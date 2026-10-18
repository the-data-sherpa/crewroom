//! Native process ownership. Workflow admission and command containment live above this boundary.
pub mod container_command;
mod delivery_flight;
pub mod evidence;
pub mod helper;
pub mod host;
pub mod host_journal;
pub mod host_protocol;
mod inspect;
pub mod jira_delivery;
pub mod jira_sync;
pub mod mail;
pub mod messaging;
pub mod model_catalog;
pub mod model_policy;
pub mod observation;
pub mod pane;
mod provider_body;
pub mod provider_gateway;
pub mod provider_input;
pub mod provider_usage;
pub mod providers;
pub mod publication;
pub mod publication_git;
pub mod publication_remote;
pub mod repository;
pub mod rpc;
pub mod rpc_frame;
pub mod runner;
mod spec;
pub mod supervisor;
pub mod target_refresh;
use crewroom_core::{Error, Result};
pub use inspect::{
    Ownership, ProcessIdentity, inspect_checked, inspect_owned, inspect_settled, seal_checked,
    stop_owned,
};
pub use spec::validate_launch;
pub use spec::{
    CheckedProcess, CheckedSpec, CommandSpec, describe_checked, prepare_checked, prepare_owned,
};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::Path,
};

pub(crate) fn conflict(message: &str) -> Error {
    Error::Conflict(message.into())
}
pub(crate) fn canonical(path: &Path) -> Result<()> {
    if !path.is_absolute() || std::fs::canonicalize(path)?.as_os_str() != path.as_os_str() {
        return Err(conflict("Process directory is not canonical."));
    }
    Ok(())
}
pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}
pub(crate) fn open_lease(root: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join("lease.lock"))?)
}
pub(crate) fn flock(file: &File, operation: i32) -> Result<bool> {
    loop {
        // SAFETY: the borrowed File keeps the descriptor valid; flock does not retain it.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(false);
        }
        return Err(error.into());
    }
}
pub(crate) fn lease_free(root: &Path) -> Result<bool> {
    flock(&open_lease(root)?, libc::LOCK_SH | libc::LOCK_NB)
}
pub(crate) fn durable_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(
        path.parent()
            .ok_or_else(|| conflict("Missing parent directory."))?,
    )?
    .sync_all()?;
    Ok(())
}
pub(crate) fn create_root(root: &Path) -> Result<()> {
    canonical(
        root.parent()
            .ok_or_else(|| conflict("Missing process parent."))?,
    )?;
    match std::fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => File::open(root.parent().unwrap())?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    canonical(root)
}
