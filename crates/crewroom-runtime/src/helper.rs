//! A separate helper retains the operation lease and drains output after controller death.
use crate::{
    CheckedProcess, CommandSpec, canonical, conflict, durable_new, flock, lease_free, open_lease,
    read,
};
use crewroom_core::Result;
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::OpenOptionsExt,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

pub(crate) struct DispatchGuard {
    _file: File,
}
impl DispatchGuard {
    pub(crate) fn acquire(descriptor: &CheckedProcess) -> Result<Self> {
        canonical(&descriptor.controller_root)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(descriptor.controller_root.join("dispatch.lock"))?;
        flock(&file, libc::LOCK_SH)?;
        if serde_json::from_slice::<serde_json::Value>(&read(
            &descriptor.controller_root.join("epoch.json"),
        )?)? != json!({"epoch":descriptor.epoch})
        {
            return Err(conflict(
                "Controller epoch no longer owns checked dispatch.",
            ));
        }
        if lease_free(&descriptor.controller_root)? {
            return Err(conflict("Controller lock is no longer held."));
        }
        Ok(Self { _file: file })
    }
}
pub(crate) fn inherit_lease(command: &mut Command, lease: &File) {
    let fd = lease.as_raw_fd();
    // SAFETY: pre_exec only performs async-signal-safe fcntl. The caller retains lease
    // through spawn/exec and all other descriptors retain their CLOEXEC flags.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
fn owner(root: &Path, token: &str, checked: Option<&CheckedProcess>) -> Result<()> {
    let output = Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &std::process::id().to_string()])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        return Err(conflict("Cannot establish process start identity."));
    }
    let started = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if started.is_empty() {
        return Err(conflict("Missing process start identity."));
    }
    // SAFETY: getpgrp has no preconditions.
    let pgid = unsafe { libc::getpgrp() };
    if pgid != std::process::id() as i32 {
        return Err(conflict("Process helper must own its process group."));
    }
    let mut value = json!({"pid":std::process::id(),"pgid":pgid,"started":started,"token":token});
    if let Some(descriptor) = checked {
        value["specSha256"] = json!(descriptor.spec_sha256);
        value["epoch"] = json!(descriptor.epoch);
    }
    publish_owner(root, &serde_json::to_vec(&value)?)
}
/// `owner.json` is either absent or complete. A helper killed while writing
/// leaves only the partial file, so inspection never reads a torn receipt and
/// an absent receipt still proves the command was not executed.
fn publish_owner(root: &Path, bytes: &[u8]) -> Result<()> {
    let partial = root.join("owner.json.partial");
    durable_new(&partial, bytes)?;
    // A hard link fails if a receipt already exists, unlike rename.
    std::fs::hard_link(&partial, root.join("owner.json"))?;
    std::fs::remove_file(&partial)?;
    File::open(root)?.sync_all()?;
    Ok(())
}
fn command(spec: &CommandSpec, lease: &File) -> Command {
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env);
    inherit_lease(&mut command, lease);
    command
}
pub fn execute_owned(root: &Path) -> Result<()> {
    let lease = open_lease(root)?;
    if !flock(&lease, libc::LOCK_EX | libc::LOCK_NB)? {
        return Err(conflict("Process lease already held."));
    }
    let value: serde_json::Value = serde_json::from_slice(&read(&root.join("launch.json"))?)?;
    let spec: CommandSpec = serde_json::from_value(value.clone())?;
    let token = value["token"]
        .as_str()
        .ok_or_else(|| conflict("Missing process token."))?;
    owner(root, token, None)?;
    Err(command(&spec, &lease).exec().into())
}
/// Checked admission followed by in-place exec preserves the host's PID and
/// terminal. The dispatch lock closes on successful exec; only the process
/// ownership lease survives. Repository commands use captured execution below.
pub fn execute_checked_owned(descriptor: &CheckedProcess) -> Result<()> {
    descriptor.validate()?;
    let _dispatch = DispatchGuard::acquire(descriptor)?;
    canonical(&descriptor.root)?;
    let lease = open_lease(&descriptor.root)?;
    if !flock(&lease, libc::LOCK_EX | libc::LOCK_NB)? {
        return Err(conflict("Process lease already held."));
    }
    let spec: CommandSpec = serde_json::from_value(descriptor.manifest()?)?;
    if descriptor.root.join("seal.json").try_exists()?
        || descriptor.root.join("owner.json").try_exists()?
    {
        return Err(conflict("Checked process was already launched or sealed."));
    }
    owner(&descriptor.root, &descriptor.token, Some(descriptor))?;
    Err(command(&spec, &lease).exec().into())
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Capture {
    retained_bytes: usize,
    truncated: bool,
}
fn capture(mut pipe: impl Read, mut output: File, overflow: &Path) -> Result<(Capture, bool)> {
    const CAP: usize = 128 * 1024;
    let mut info = Capture {
        retained_bytes: 0,
        truncated: false,
    };
    let mut failed = false;
    let mut chunk = [0u8; 65536];
    loop {
        let n = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        let keep = n.min(CAP - info.retained_bytes);
        if n > keep && !info.truncated {
            info.truncated = true;
            // Negative evidence only: bounded callers can stop promptly instead
            // of waiting for final capture metadata. Never a success/exit receipt.
            if durable_new(overflow, b"").is_err() {
                failed = true;
            }
        }
        let mut prefix = &chunk[..keep];
        while !prefix.is_empty() && !failed {
            match output.write(prefix) {
                Ok(0) => failed = true,
                Ok(n) => {
                    info.retained_bytes += n;
                    prefix = &prefix[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (),
                Err(_) => failed = true,
            }
        }
        // Keep draining even after a retained log fails; repository writes remain unlimited.
    }
    if output.sync_all().is_err() {
        failed = true;
    }
    Ok((info, failed))
}
/// Returns only a live-parent exit observation; no durable success/exit receipt is written.
pub fn execute_checked(descriptor: &CheckedProcess) -> Result<ExitStatus> {
    descriptor.validate()?;
    let dispatch = DispatchGuard::acquire(descriptor)?;
    canonical(&descriptor.root)?;
    let lease = open_lease(&descriptor.root)?;
    if !flock(&lease, libc::LOCK_EX | libc::LOCK_NB)? {
        return Err(conflict("Process lease already held."));
    }
    let spec: CommandSpec = serde_json::from_value(descriptor.manifest()?)?;
    if descriptor.root.join("seal.json").try_exists()? {
        return Err(conflict("Checked process was sealed."));
    }
    if descriptor.root.join("owner.json").try_exists()? {
        return Err(conflict("Checked process was already launched."));
    }
    owner(&descriptor.root, &descriptor.token, Some(descriptor))?;
    let open = |name| -> Result<File> {
        Ok(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(descriptor.root.join(name))?)
    };
    let stdout = open("stdout.log")?;
    let stderr = open("stderr.log")?;
    let mut child = command(&spec, &lease)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(dispatch); // Only the operation lease reaches the child. Takeover need not wait for Git.
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let captures = std::thread::scope(|scope| {
        let a = scope.spawn(|| capture(out, stdout, &descriptor.root.join("stdout.truncated")));
        let b = scope.spawn(|| capture(err, stderr, &descriptor.root.join("stderr.truncated")));
        (a.join(), b.join())
    });
    let status = child.wait()?;
    let (out, failed_out) = captures
        .0
        .map_err(|_| conflict("Output capture worker failed."))??;
    let (err, failed_err) = captures
        .1
        .map_err(|_| conflict("Output capture worker failed."))??;
    if failed_out || failed_err {
        return Err(conflict(
            "Checked output capture failed; command outcome requires reconciliation.",
        ));
    }
    // Only fully persisted captures accompany the helper's forwarded child exit.
    // A helper failure also exits nonzero, so a partial capture must not let a
    // live parent mistake that failure for the command's own numeric exit.
    durable_new(
        &descriptor.root.join("capture.json"),
        &serde_json::to_vec(&json!({"stdout":out,"stderr":err}))?,
    )?;
    Ok(status)
}
/// Preserve child signals for the live parent rather than inventing a normal exit code.
pub fn exit_like(status: ExitStatus) -> ! {
    if let Some(signal) = status.signal() {
        // SAFETY: restoring the default disposition and raising the observed child signal
        // terminates only this helper; no other process is addressed.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }
    std::process::exit(status.code().unwrap_or(1));
}
