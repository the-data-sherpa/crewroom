use crate::{
    CheckedProcess, canonical, conflict, create_root, durable_new, flock, lease_free, open_lease,
    read,
};
use crewroom_core::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeSet, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{Instant, sleep, timeout},
};

#[cfg(target_os = "macos")]
const LSOF: &str = "/usr/sbin/lsof";
#[cfg(not(target_os = "macos"))]
const LSOF: &str = "/usr/bin/lsof";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub pgid: i32,
    pub started: String,
    pub token: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ownership {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<ProcessIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<i32>>,
}
impl Ownership {
    fn new(state: &str, reason: Option<&str>) -> Self {
        Self {
            state: state.into(),
            reason: reason.map(str::to_owned),
            owner: None,
            members: None,
        }
    }
    fn uncertain(reason: &str) -> Self {
        Self::new("uncertain", Some(reason))
    }
}
async fn output(program: &str, args: &[&str]) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let pipe = child.stdout.take().unwrap();
    let future = async {
        let mut bytes = Vec::new();
        pipe.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
        if bytes.len() > 1024 * 1024 {
            return Err(conflict("Process inspection exceeded its output limit."));
        }
        if !child.wait().await?.success() {
            return Err(conflict("Process inspection failed."));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| conflict("Process inspection timed out."))?
}
const SURVIVES: &str = "A process survives without its inherited ownership lock.";
const EXITING: &str = "Every remaining process is exiting; ownership is not yet released.";
/// How long a settle re-inspects an uncertain ownership.
const SETTLE_WINDOW: Duration = Duration::from_secs(1);
/// How long a settle waits, in total, once every listed group member was seen
/// irrevocably exiting. A loaded scheduler can keep an exiting process listed,
/// or its lock held, for longer than [`SETTLE_WINDOW`].
const EXIT_WINDOW: Duration = Duration::from_secs(5);
/// Whether the process `pid` has exited or is irrevocably exiting.
type ExitProbe = fn(i32) -> bool;

/// Whether one task's `/proc/<pid>/task/<tid>/stat` line shows it dead or
/// past the point of no return in `do_exit` (`PF_EXITING`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn task_exiting(stat: &str) -> Option<bool> {
    const PF_EXITING: u64 = 0x4;
    // The command name may itself contain spaces and parentheses.
    let fields: Vec<_> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    let state = fields.first()?;
    let flags: u64 = fields.get(6)?.parse().ok()?;
    Some(state.starts_with(['Z', 'X']) || flags & PF_EXITING != 0)
}
/// A process with no remaining task, or whose every task is dead or flagged
/// `PF_EXITING`, can no longer run user code. Anything unreadable counts as
/// running, so an unknown process is never mistaken for an exiting one.
#[cfg(target_os = "linux")]
fn exiting(pid: i32) -> bool {
    use std::io::ErrorKind::NotFound;
    let tasks = match std::fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => tasks,
        Err(error) => return error.kind() == NotFound,
    };
    for task in tasks {
        let Ok(task) = task else { return false };
        match std::fs::read_to_string(task.path().join("stat")) {
            Ok(stat) if task_exiting(&stat) == Some(true) => {}
            Err(error) if error.kind() == NotFound => {} // This task already finished.
            _ => return false,
        }
    }
    true
}
/// Elsewhere `ps` already reports an exiting process (`E`), so only a process
/// that no longer exists counts.
#[cfg(not(target_os = "linux"))]
fn exiting(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists; nothing is delivered.
    let missing = unsafe { libc::kill(pid, 0) } != 0;
    missing && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}
async fn inspect(root: &Path, scan_holders: bool, exiting: ExitProbe) -> Result<Ownership> {
    if !root.join("launch.json").try_exists()? {
        return Ok(Ownership::new("stopped", None));
    }
    let intent: serde_json::Value = serde_json::from_slice(&read(&root.join("launch.json"))?)?;
    let owner: ProcessIdentity = serde_json::from_slice(&read(&root.join("owner.json"))?)?;
    if intent["token"] != owner.token || owner.pid < 2 || owner.pid != owner.pgid {
        return Ok(Ownership::uncertain(
            "Process identity does not match its launch intent.",
        ));
    }
    // Read the lease before listing processes. Under load the listing can take
    // long enough for a member to exit after it is listed; a lease read after
    // the listing would then pair a stale member with a lock it has released.
    let free = lease_free(root)?;
    let table = output("/bin/ps", &["-axo", "pid=,pgid=,stat=,lstart="]).await?;
    let processes: Vec<_> = table
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 4
                || fields[2].starts_with('Z')
                || fields[2].chars().skip(1).any(|c| c == 'E')
            {
                return None;
            }
            Some((
                fields[0].parse::<i32>().ok()?,
                fields[1].parse::<i32>().ok()?,
                fields[3..].join(" "),
            ))
        })
        .collect();
    let current = processes.iter().find(|p| p.0 == owner.pid);
    let members: Vec<_> = processes
        .iter()
        .filter(|p| p.1 == owner.pgid)
        .map(|p| p.0)
        .collect();
    let mut state = Ownership {
        state: "uncertain".into(),
        reason: None,
        owner: Some(owner.clone()),
        members: Some(members.clone()),
    };
    if current.is_none() && members.is_empty() && free {
        state.state = "stopped".into();
        return Ok(state);
    }
    if current.is_some_and(|p| {
        p.1 != owner.pgid
            || p.2
                != owner
                    .started
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
    }) {
        state.reason = Some("PID identity changed; refusing to signal it.".into());
        return Ok(state);
    }
    // A member that the kernel shows as exiting (or already gone) can no longer
    // run user code and drops whatever it still holds, the lock included. On
    // Linux it releases its files in `do_exit` before it becomes a zombie, so
    // it can stay listed, with or without the lock, after its leader exits.
    // Waiting for it is not proof of stop: only a later inspection that finds
    // the group empty and the lock free reports `stopped`.
    if !members.is_empty() && members.iter().all(|&pid| exiting(pid)) {
        state.reason = Some(EXITING.into());
        return Ok(state);
    }
    if free {
        state.reason = Some(SURVIVES.into());
        return Ok(state);
    }
    let path = root.join("lease.lock");
    let path = path
        .to_str()
        .ok_or_else(|| conflict("Invalid process path."))?;
    let holders = async |scoped: bool| {
        let pid = owner.pid.to_string();
        let args = if scoped {
            vec!["-t", "-a", "-p", &pid, path]
        } else {
            vec!["-t", path]
        };
        output(LSOF, &args)
            .await
            .unwrap_or_default()
            .lines()
            .filter_map(|s| s.parse::<i32>().ok())
            .collect::<Vec<_>>()
    };
    if current.is_some() && holders(true).await.contains(&owner.pid) {
        state.state = "live".into();
        return Ok(state);
    }
    let mut members: BTreeSet<_> = members.into_iter().collect();
    if scan_holders {
        members.extend(holders(false).await);
    }
    state.members = Some(members.into_iter().collect());
    state.reason = Some("The leader exited; surviving lock holders require reconciliation.".into());
    Ok(state)
}
pub async fn inspect_owned(root: &Path, scan_holders: bool) -> Ownership {
    inspect_probed(root, scan_holders, exiting).await
}
async fn inspect_probed(root: &Path, scan_holders: bool, exiting: ExitProbe) -> Ownership {
    inspect(root, scan_holders, exiting)
        .await
        .unwrap_or_else(|_| Ownership::uncertain("Launch ownership cannot be established."))
}
/// Re-inspect an uncertain ownership for `window`, or for [`EXIT_WINDOW`] in
/// all once an inspection found only exiting members. Only a
/// completed fresh inspection ever reports `stopped`; waiting never does.
async fn settled(root: &Path, first: Ownership, window: Duration, exiting: ExitProbe) -> Ownership {
    let mut state = first.clone();
    let start = Instant::now();
    let mut deadline = start + window;
    loop {
        // Once a member was seen exiting, keep the longer window: its later
        // observations (lock still held while the group is already empty,
        // say) are the same exit, not a new uncertainty.
        if state.reason.as_deref() == Some(EXITING) {
            deadline = deadline.max(start + EXIT_WINDOW);
        }
        if state.state != "uncertain" || Instant::now() + Duration::from_millis(50) >= deadline {
            break;
        }
        sleep(Duration::from_millis(50)).await;
        // The window bounds when a re-inspection may start, not how long it
        // may take: a loaded machine can spend most of a second listing
        // processes, and cancelling that listing would leave the stale first
        // observation as the answer. Each inspection is bounded by its own
        // command timeouts.
        state = inspect_probed(root, false, exiting).await;
    }
    if state.state == "uncertain" {
        state.members = Some(
            state
                .members
                .iter()
                .flatten()
                .chain(first.members.iter().flatten())
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        );
    }
    state
}
pub async fn inspect_settled(root: &Path) -> Ownership {
    settled(
        root,
        inspect_owned(root, true).await,
        SETTLE_WINDOW,
        exiting,
    )
    .await
}
/// Whether a helper that its caller already signalled and reaped as process
/// group leader `pgid` stopped before it could launch anything. The helper
/// writes `owner.json` durably before it execs the command, so an absent
/// receipt means the command never started. The free lease and the empty
/// process group rule out anything the helper forked on the way.
pub(crate) async fn stopped_before_owner(root: &Path, pgid: i32) -> bool {
    let check = async {
        if root.join("owner.json").try_exists()? {
            return Ok(false);
        }
        // A member killed with the group (such as the helper's own `ps`) can
        // still be listed, and still hold the inherited lease, for a moment.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let table = output("/bin/ps", &["-axo", "pgid=,stat="]).await?;
            let empty = !table.lines().any(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                fields.len() == 2
                    && fields[0].parse::<i32>() == Ok(pgid)
                    && !fields[1].starts_with('Z')
            });
            let stopped = empty && lease_free(root)?;
            if stopped || Instant::now() >= deadline {
                return Ok::<_, crewroom_core::Error>(stopped);
            }
            sleep(Duration::from_millis(50)).await;
        }
    };
    check.await.unwrap_or(false)
}
pub async fn stop_owned(root: &Path) -> Ownership {
    let mut state = inspect_settled(root).await;
    if state.state != "live" {
        return state;
    }
    // SAFETY: signal only a leader/group whose start identity and inherited kernel
    // lock were just observed. Uncertain/foreign identities are never signalled.
    if unsafe { libc::kill(-state.owner.as_ref().unwrap().pgid, libc::SIGTERM) } != 0 {
        return inspect_settled(root).await;
    }
    let deadline = Instant::now() + Duration::from_millis(600);
    while Instant::now() < deadline {
        sleep(Duration::from_millis(30)).await;
        state = inspect_owned(root, true).await;
        if state.state == "stopped" {
            return state;
        }
    }
    if state.state == "live" {
        // SAFETY: use only the latest verified live identity, never the original stale PID.
        unsafe { libc::kill(-state.owner.as_ref().unwrap().pgid, libc::SIGKILL) };
    }
    sleep(Duration::from_millis(50)).await;
    inspect_settled(root).await
}
fn only_unprepared_files(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let name = entry?.file_name();
        if name != "lease.lock" && name != "seal.json" {
            return Err(conflict("Unprepared root has other files."));
        }
    }
    Ok(())
}
pub async fn inspect_checked(descriptor: &CheckedProcess) -> Ownership {
    let validate = || -> Result<Option<Ownership>> {
        descriptor.validate()?;
        canonical(&descriptor.root)?;
        let unprepared = !descriptor.root.join("launch.json").try_exists()?;
        if unprepared {
            only_unprepared_files(&descriptor.root)?;
        } else {
            descriptor.manifest()?;
        }
        let owner_path = descriptor.root.join("owner.json");
        if descriptor.root.join("seal.json").try_exists()? {
            let seal: serde_json::Value =
                serde_json::from_slice(&read(&descriptor.root.join("seal.json"))?)?;
            if seal != json!({"token":descriptor.token,"specSha256":descriptor.spec_sha256}) {
                return Err(conflict("Seal mismatch."));
            }
            if !owner_path.try_exists()? {
                return Ok(Some(if lease_free(&descriptor.root)? {
                    Ownership::new(
                        "stopped",
                        Some(if unprepared {
                            "Admitted launch sealed before manifest creation."
                        } else {
                            "Prepared launch durably sealed before execution."
                        }),
                    )
                } else {
                    Ownership::uncertain("Sealed lease still has a holder.")
                }));
            }
        }
        if unprepared {
            return Err(conflict("Manifest-less process is not sealed."));
        }
        if owner_path.try_exists()? {
            let owner: serde_json::Value = serde_json::from_slice(&read(&owner_path)?)?;
            if owner["specSha256"] != descriptor.spec_sha256 || owner["epoch"] != descriptor.epoch {
                return Err(conflict("Owner mismatch."));
            }
        }
        Ok(None)
    };
    match validate() {
        Ok(Some(state)) => state,
        Ok(None) => inspect_settled(&descriptor.root).await,
        Err(_) => Ownership::uncertain("Checked process identity cannot be established."),
    }
}
pub async fn seal_checked(descriptor: &CheckedProcess) -> Ownership {
    let validate_files = || -> Result<()> {
        if descriptor.root.join("launch.json").try_exists()? {
            descriptor.manifest()?;
        } else {
            only_unprepared_files(&descriptor.root)?;
        }
        Ok(())
    };
    let prepare = || -> Result<()> {
        descriptor.validate()?;
        canonical(&descriptor.controller_root)?;
        create_root(&descriptor.root)?;
        validate_files()
    };
    if prepare().is_err() {
        return Ownership::uncertain("Checked process manifest does not match its descriptor.");
    }
    let seal = || -> Result<()> {
        let lease = open_lease(&descriptor.root)?;
        if !flock(&lease, libc::LOCK_EX | libc::LOCK_NB)? {
            return Err(conflict("Process lease already held."));
        }
        validate_files()?;
        let expected = json!({"token":descriptor.token,"specSha256":descriptor.spec_sha256});
        let path = descriptor.root.join("seal.json");
        if path.try_exists()? {
            if serde_json::from_slice::<serde_json::Value>(&read(&path)?)? != expected {
                return Err(conflict("Seal mismatch."));
            }
        } else {
            durable_new(&path, &serde_json::to_vec(&expected)?)?;
        }
        Ok(())
    };
    let _ = seal(); // A refused seal never proves stop; re-inspect the original descriptor.
    inspect_checked(descriptor).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn prepared() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        durable_new(&root.path().join("launch.json"), br#"{"token":"t"}"#).unwrap();
        root
    }
    fn reaped_group() -> i32 {
        let mut child = std::process::Command::new("/bin/true")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        pid
    }

    #[tokio::test]
    async fn a_helper_reaped_before_its_owner_receipt_launched_nothing() {
        let root = prepared();
        assert!(stopped_before_owner(root.path(), reaped_group()).await);
        // Killed while writing its receipt: only the unpublished file exists.
        durable_new(&root.path().join("owner.json.partial"), b"{\"pi").unwrap();
        assert!(stopped_before_owner(root.path(), reaped_group()).await);
    }

    #[tokio::test]
    async fn an_owner_receipt_a_held_lease_or_a_live_member_needs_inspection() {
        let root = prepared();
        let lease = open_lease(root.path()).unwrap();
        assert!(flock(&lease, libc::LOCK_EX | libc::LOCK_NB).unwrap());
        assert!(!stopped_before_owner(root.path(), reaped_group()).await);
        drop(lease);

        let mut member = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let live = member.id() as i32;
        let survived = stopped_before_owner(root.path(), live).await;
        member.kill().unwrap();
        member.wait().unwrap();
        assert!(!survived);

        durable_new(&root.path().join("owner.json"), b"{}").unwrap();
        assert!(!stopped_before_owner(root.path(), reaped_group()).await);
    }

    /// A launch whose recorded owner is a real `sleep` group leader that does
    /// not hold the (free) lease: what a runner looks like after it released
    /// its files, whether it is exiting or survives.
    fn unlocked_owner() -> (tempfile::TempDir, std::process::Child) {
        let root = prepared();
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let started = std::process::Command::new("/bin/ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .env("LC_ALL", "C")
            .output()
            .unwrap()
            .stdout;
        let owner = json!({
            "pid": pid,
            "pgid": pid,
            "started": String::from_utf8(started).unwrap().trim(),
            "token": "t",
        });
        durable_new(
            &root.path().join("owner.json"),
            &serde_json::to_vec(&owner).unwrap(),
        )
        .unwrap();
        assert!(lease_free(root.path()).unwrap());
        (root, child)
    }

    #[tokio::test]
    async fn a_member_exiting_after_its_lock_release_settles_stopped_past_the_window() {
        let (root, mut child) = unlocked_owner();
        // The kernel shows the member past PF_EXITING; it disappears only after
        // the ordinary settle window has passed, as under heavy load.
        let reaper = std::thread::spawn(move || {
            std::thread::sleep(SETTLE_WINDOW + Duration::from_millis(500));
            child.kill().unwrap();
            child.wait().unwrap();
        });
        let first = inspect_probed(root.path(), true, |_| true).await;
        assert_eq!(first.state, "uncertain");
        assert_eq!(first.reason.as_deref(), Some(EXITING));
        let start = Instant::now();
        let state = settled(root.path(), first, SETTLE_WINDOW, |_| true).await;
        reaper.join().unwrap();
        assert_eq!(state.state, "stopped", "{state:?}");
        assert!(start.elapsed() > SETTLE_WINDOW);
    }

    #[tokio::test]
    async fn an_exit_seen_holding_the_lock_keeps_the_longer_window_until_release() {
        let (root, mut child) = unlocked_owner();
        // Stand in for the exiting member's lock, released only after the
        // member itself is gone: the group is empty while the lock is held.
        let lease = open_lease(root.path()).unwrap();
        assert!(flock(&lease, libc::LOCK_EX | libc::LOCK_NB).unwrap());
        let release = std::thread::spawn(move || {
            std::thread::sleep(SETTLE_WINDOW + Duration::from_millis(300));
            child.kill().unwrap();
            child.wait().unwrap();
            std::thread::sleep(Duration::from_millis(600));
            drop(lease);
        });
        let first = inspect_probed(root.path(), true, |_| true).await;
        assert_eq!(first.reason.as_deref(), Some(EXITING), "{first:?}");
        let state = settled(root.path(), first, SETTLE_WINDOW, |_| true).await;
        release.join().unwrap();
        assert_eq!(state.state, "stopped", "{state:?}");
    }

    #[tokio::test]
    async fn a_survivor_without_its_lock_stays_uncertain_and_is_never_signalled() {
        let (root, mut child) = unlocked_owner();
        let start = Instant::now();
        let state = stop_owned(root.path()).await;
        let elapsed = start.elapsed();
        let alive = child.try_wait().unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(state.state, "uncertain");
        assert_eq!(state.reason.as_deref(), Some(SURVIVES));
        assert!(alive, "an uncertain survivor was signalled");
        // A running survivor does not earn the longer exit window.
        assert!(elapsed < EXIT_WINDOW, "{elapsed:?}");
    }

    #[test]
    fn task_stat_flags_and_state_decide_exiting() {
        let stat = |state: &str, flags: u64| {
            format!("42 (a (b) c) {state} 1 42 42 0 -1 {flags} 0 0 0 0 1 2 0 0 20 0 1 0 9")
        };
        assert_eq!(task_exiting(&stat("R", 0x0040_0100)), Some(false));
        assert_eq!(task_exiting(&stat("S", 0x0040_0104)), Some(true));
        assert_eq!(task_exiting(&stat("D", 0x4)), Some(true));
        assert_eq!(task_exiting(&stat("Z", 0x0040_0100)), Some(true));
        assert_eq!(task_exiting(&stat("X", 0)), Some(true));
        assert_eq!(task_exiting("42 (truncated"), None);
        assert_eq!(task_exiting("42 (sleep) S 1 42"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_kernel_shows_a_running_process_as_not_exiting_and_a_dead_one_as_exiting() {
        let mut running = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let live = exiting(running.id() as i32);
        running.kill().unwrap();
        running.wait().unwrap();
        assert!(!live);
        assert!(!exiting(std::process::id() as i32));

        let mut zombie = std::process::Command::new("/bin/true").spawn().unwrap();
        let pid = zombie.id() as i32;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap()
            .contains(") Z ")
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let dead = exiting(pid);
        zombie.wait().unwrap();
        assert!(dead);
        assert!(exiting(reaped_group()));
    }
}
