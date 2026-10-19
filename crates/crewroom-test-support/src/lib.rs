//! Test-only helpers shared by the Crewroom crates' test suites (FOR-89).
//!
//! Nothing here is linked into a product binary: every crate takes this crate
//! as a `[dev-dependencies]` entry only.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Installs `content` at `path` as an owner-only executable (0700) without
/// this process ever holding a writer to the file it will exec (FOR-86, FOR-89).
///
/// Each spawn by a sibling test thread copies every open descriptor, including
/// `O_CLOEXEC` ones, into the child until the child execs. If one of those
/// copies is a writer of a script, exec of that script fails with
/// `Text file busy` (ETXTBSY). `std::fs::write` followed by exec therefore fails
/// intermittently in any multi-threaded test binary.
///
/// The content is staged under a different name, a separate `cp` process is
/// the only writer of the executable, and the finished file is renamed onto
/// `path`. The rename also makes replacement atomic: `path` never names a
/// half-written file, and a process still running the previous file keeps it.
pub fn install_executable(path: impl AsRef<Path>, content: impl AsRef<[u8]>) {
    let path = path.as_ref();
    let staged = sibling(path, "source");
    let next = sibling(path, "next");
    std::fs::write(&staged, content).unwrap();
    let copied = std::process::Command::new("cp")
        .arg("--")
        .arg(&staged)
        .arg(&next)
        .status()
        .unwrap();
    assert!(copied.success(), "cp {}: {copied}", next.display());
    std::fs::remove_file(&staged).unwrap();
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(&next, path).unwrap();
}

/// A unique hidden name next to `path`, so concurrent installs never collide.
fn sibling(path: &Path, kind: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .unwrap_or_else(|| panic!("{} has no file name", path.display()));
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{}.{kind}-{}-{unique}",
        name.to_string_lossy(),
        std::process::id()
    ))
}
