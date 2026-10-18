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
async fn inspect(root: &Path, scan_holders: bool) -> Result<Ownership> {
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
    let free = lease_free(root)?;
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
    if free {
        state.reason = Some("A process survives without its inherited ownership lock.".into());
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
    inspect(root, scan_holders)
        .await
        .unwrap_or_else(|_| Ownership::uncertain("Launch ownership cannot be established."))
}
async fn settled(root: &Path, first: Ownership, window: Duration) -> Ownership {
    let mut state = first.clone();
    let deadline = Instant::now() + window;
    while state.state == "uncertain" && Instant::now() + Duration::from_millis(50) < deadline {
        sleep(Duration::from_millis(50)).await;
        match timeout(
            deadline.saturating_duration_since(Instant::now()),
            inspect_owned(root, false),
        )
        .await
        {
            Ok(next) => state = next,
            Err(_) => break,
        }
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
        Duration::from_secs(1),
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
}
