//! The Rust controller shares the accepted service's OS lock, not just its filename.
use crate::{Error, Result};
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

pub struct Authority {
    lease: File,
    dispatch: File,
    root: PathBuf,
    epoch: String,
}
fn open_lock(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}
fn try_lock(file: &File, mode: i32) -> Result<bool> {
    loop {
        // SAFETY: fd belongs to a live File; flock does not retain it.
        if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } == 0 {
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
fn lock(file: &File, mode: i32) -> Result<()> {
    if !try_lock(file, mode)? {
        return Err(Error::Conflict(
            "Another controller owns this workbench.".into(),
        ));
    }
    Ok(())
}
impl Authority {
    pub fn acquire(data: &Path) -> Result<Self> {
        Self::acquire_mode(data, false)
    }
    /// Runtime holders share dispatch with checked helpers; the service stays fenced
    /// until its runner integration is admitted.
    pub fn acquire_for_execution(data: &Path) -> Result<Self> {
        Self::acquire_mode(data, true)
    }
    fn acquire_mode(data: &Path, checked: bool) -> Result<Self> {
        std::fs::create_dir_all(data)?;
        let data = std::fs::canonicalize(data)?;
        let metadata = std::fs::metadata(&data)?;
        // SAFETY: geteuid has no preconditions.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(Error::Conflict("Workspace belongs to another user.".into()));
        }
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700))?;
        let root = data.join("controller");
        if std::fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(Error::Conflict(
                "Controller directory cannot be a symlink.".into(),
            ));
        }
        std::fs::create_dir_all(&root)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        let dispatch = open_lock(&root.join("dispatch.lock"))?;
        let lease = open_lock(&root.join("lease.lock"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);
        while !try_lock(&dispatch, libc::LOCK_EX)? {
            lock(&lease, libc::LOCK_SH)?;
            // SAFETY: a valid owned descriptor; release our probe, never another holder.
            if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_UN) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Conflict("Controller dispatch lock remains held while controller lease is free; inspect checked dispatch helper and retry.".into()));
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        lock(&lease, libc::LOCK_EX)?;
        let epoch = uuid::Uuid::new_v4().to_string();
        write_private(
            &root.join("epoch.json"),
            serde_json::to_string(&serde_json::json!({"epoch":epoch}))?.as_bytes(),
        )?;
        if checked {
            lock(&dispatch, libc::LOCK_SH)?;
        }
        Ok(Self {
            lease,
            dispatch,
            root,
            epoch,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
    pub fn assert_current(&self) -> Result<()> {
        if std::fs::read(self.root.join("epoch.json"))?
            != serde_json::to_vec(&serde_json::json!({"epoch":self.epoch}))?
        {
            return Err(Error::Conflict(
                "Controller authority was lost. Saved work is retained.".into(),
            ));
        }
        for (file, name) in [
            (&self.lease, "lease.lock"),
            (&self.dispatch, "dispatch.lock"),
        ] {
            let saved = file.metadata()?;
            let current = std::fs::symlink_metadata(self.root.join(name))?;
            if current.file_type().is_symlink()
                || saved.ino() != current.ino()
                || saved.dev() != current.dev()
            {
                return Err(Error::Conflict(
                    "Controller authority was lost. Saved work is retained.".into(),
                ));
            }
        }
        Ok(())
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("Private file needs a parent directory.".into()))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_service_fences_dispatch_but_runtime_holder_shares_it() {
        let data = tempfile::tempdir().unwrap();
        let service = Authority::acquire(data.path()).unwrap();
        let probe = open_lock(&service.root().join("dispatch.lock")).unwrap();
        assert!(!try_lock(&probe, libc::LOCK_SH).unwrap());
        let first_epoch = service.epoch().to_owned();
        drop(service);
        let runtime = Authority::acquire_for_execution(data.path()).unwrap();
        assert_ne!(runtime.epoch(), first_epoch);
        assert!(try_lock(&probe, libc::LOCK_SH).unwrap());
        assert!(Authority::acquire_for_execution(data.path()).is_err());
        runtime.assert_current().unwrap();
        assert_eq!(
            std::fs::metadata(runtime.root().join("epoch.json"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn replaced_epoch_revokes_authority_without_changing_its_locks() {
        let data = tempfile::tempdir().unwrap();
        let holder = Authority::acquire_for_execution(data.path()).unwrap();
        write_private(&holder.root().join("epoch.json"), br#"{"epoch":"foreign"}"#).unwrap();
        assert!(holder.assert_current().is_err());
        assert!(Authority::acquire(data.path()).is_err());
    }
    #[test]
    fn one_authority_and_lock_replacement_fence() {
        let root = tempfile::tempdir().unwrap();
        let first = Authority::acquire(root.path()).unwrap();
        assert!(Authority::acquire(root.path()).is_err());
        first.assert_current().unwrap();
        std::fs::rename(
            root.path().join("controller/lease.lock"),
            root.path().join("old-lock"),
        )
        .unwrap();
        std::fs::write(root.path().join("controller/lease.lock"), "").unwrap();
        assert!(first.assert_current().is_err());
        drop(first);
        Authority::acquire(root.path()).unwrap();
    }
}
