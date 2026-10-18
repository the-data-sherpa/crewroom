//! Source paths are traversed through directory descriptors without following links.
use crate::conflict;
use crewroom_core::Result;
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
pub(crate) fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
pub(crate) fn name(value: &str) -> Result<()> {
    if value.is_empty()
        || Path::new(value).is_absolute()
        || value.contains(['\\', '\0'])
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.eq_ignore_ascii_case(".git"))
    {
        return Err(conflict("File path is outside repository scope."));
    }
    Ok(())
}
fn directory(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}
fn parent(root: &Path, path: &str, create: bool) -> Result<(File, CString)> {
    name(path)?;
    let mut parts = path.split('/').peekable();
    let mut dir = directory(root)?;
    while let Some(part) = parts.next() {
        let c = CString::new(part).unwrap();
        if parts.peek().is_none() {
            return Ok((dir, c));
        }
        if create {
            // SAFETY: dir and the NUL-terminated component live across this call.
            let result = unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o755) };
            if result != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().into());
            }
            if result == 0 {
                dir.sync_all()?;
            }
        }
        // SAFETY: openat returns a new owned descriptor; no path component is followed through a symlink.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fd was freshly returned by openat and has exactly one owner.
        dir = unsafe { File::from_raw_fd(fd) };
    }
    unreachable!()
}
fn open(root: &Path, path: &str) -> Result<File> {
    let (dir, base) = parent(root, path, false)?;
    // SAFETY: valid borrowed directory and C string; successful result transfers fd ownership.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            base.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: a successful openat returns an unused owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() > 1 {
        return Err(conflict(
            "Repository file path must not follow links or special files.",
        ));
    }
    Ok(file)
}
pub(crate) fn read_text(root: &Path, path: &str, limit: u64) -> Result<String> {
    let file = open(root, path)?;
    if file.metadata()?.len() > limit {
        return Err(conflict("File exceeds the text tool limit."));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(conflict("File exceeds the text tool limit."));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
pub(crate) fn regular_bytes(root: &Path, path: &str, limit: u64) -> Result<(Vec<u8>, u32)> {
    let file = open(root, path)?;
    let metadata = file.metadata()?;
    if metadata.len() > limit {
        return Err(conflict("Repository source exceeds its observation limit."));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(conflict("Repository source exceeds its observation limit."));
    }
    Ok((bytes, metadata.mode()))
}
pub(crate) fn scoped(root: &Path, value: &str) -> Result<PathBuf> {
    name(value)?;
    let mut current = root.to_owned();
    for part in value.split('/') {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(m)
                if m.file_type().is_symlink()
                    || current == root.join(value) && (!m.is_file() || m.nlink() > 1) =>
            {
                return Err(conflict(
                    "Repository file path must not follow links or special files.",
                ));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(current)
}
/// Caller holds the current store/ownership guard. There is no await between its
/// last authorization, expected-content comparison and atomic replacement.
pub(crate) fn replace(
    root: &Path,
    temporary_root: &Path,
    path: &str,
    expected: Option<&str>,
    content: &str,
) -> Result<String> {
    scoped(root, path)?;
    let (before, mode) = match regular_bytes(root, path, 50 * 1024 * 1024) {
        // read_file returns the retained UTF-8 replacement-decoded text hash.
        // Manifests separately hash raw bytes; the edit precondition must match
        // the text the caller actually read.
        Ok((bytes, mode)) => (
            Some(hash(String::from_utf8_lossy(&bytes).as_bytes())),
            mode & 0o777,
        ),
        Err(crewroom_core::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            (None, 0o644)
        }
        Err(e) => return Err(e),
    };
    if before.as_deref() != expected {
        return Err(conflict("File changed since it was read."));
    }
    let (parent, base) = parent(root, path, true)?;
    let temporary = temporary_root.join(format!(".edit-{}", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    let from = CString::new(temporary.as_os_str().as_encoded_bytes())
        .map_err(|_| conflict("Invalid edit temporary path."))?;
    // SAFETY: both names and the destination descriptor are valid for this call.
    if unsafe {
        libc::renameat(
            libc::AT_FDCWD,
            from.as_ptr(),
            parent.as_raw_fd(),
            base.as_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    parent.sync_all()?;
    directory(temporary_root)?.sync_all()?;
    Ok(hash(content))
}
