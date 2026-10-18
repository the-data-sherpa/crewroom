//! Test-only location of the runtime helper binaries (FOR-59).
//!
//! `crewroomd` resolves `crewroom-process` and `crewroom-host` beside itself, but
//! Cargo builds only the tested package's own binaries. Tests in other packages
//! call [`ensure`] with the binary directory they run from so a fresh build
//! directory works without a manual `cargo build -p crewroom-runtime --bins`.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

/// The two helper binaries installed beside `crewroomd`.
#[derive(Clone, Debug)]
pub struct HelperBinaries {
    pub process: PathBuf,
    pub host: PathBuf,
}

const BUILD: &str = "cargo build --locked -p crewroom-runtime --bins";

/// The profile directory (for example `target/debug`) that holds the running test executable.
pub fn profile_dir_of_current_test() -> PathBuf {
    let exe = std::env::current_exe().expect("current test executable");
    // Integration tests and unit tests run from `<profile>/deps/<name>-<hash>`.
    exe.parent()
        .and_then(Path::parent)
        .expect("test executable inside a Cargo profile directory")
        .to_path_buf()
}

/// Build (or, when Cargo is unavailable, locate) the helpers in `profile_dir` once per process.
///
/// Building is a no-op when the helpers are current, so stale helpers from an older
/// revision are refreshed rather than silently reused.
pub fn ensure(profile_dir: &Path) -> HelperBinaries {
    static BUILT: OnceLock<Result<(), String>> = OnceLock::new();
    let helpers = HelperBinaries {
        process: profile_dir.join("crewroom-process"),
        host: profile_dir.join("crewroom-host"),
    };
    let outcome = BUILT.get_or_init(|| build(profile_dir));
    if let Err(reason) = outcome
        && !(helpers.process.is_file() && helpers.host.is_file())
    {
        panic!(
            "Runtime helper binaries are missing from {} and could not be built ({reason}). Run `{BUILD}` with the same CARGO_TARGET_DIR, or `python3 scripts/prepare-worktree.py --build-helpers`.",
            profile_dir.display()
        );
    }
    helpers
}

fn build(profile_dir: &Path) -> Result<(), String> {
    let (target_dir, triple) = target_layout(profile_dir)?;
    let profile = match profile_dir.file_name().and_then(|n| n.to_str()) {
        Some("debug") => "dev".to_owned(),
        Some(name) => name.to_owned(),
        None => return Err("unnamed profile directory".into()),
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--locked",
            "--quiet",
            "--bins",
            "-p",
            "crewroom-runtime",
        ])
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .args(["--profile", &profile])
        .env("CARGO_TARGET_DIR", &target_dir);
    if let Some(triple) = triple {
        command.args(["--target", &triple]);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Split `<target>/<profile>` or `<target>/<triple>/<profile>` into the target directory and triple.
fn target_layout(profile_dir: &Path) -> Result<(PathBuf, Option<String>), String> {
    let parent = profile_dir
        .parent()
        .ok_or("profile directory has no target directory")?;
    if parent.join("CACHEDIR.TAG").is_file() {
        return Ok((parent.to_path_buf(), None));
    }
    let root = parent
        .parent()
        .filter(|root| root.join("CACHEDIR.TAG").is_file())
        .ok_or("profile directory is not inside a Cargo target directory")?;
    let triple = parent
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("unnamed target triple directory")?;
    Ok((root.to_path_buf(), Some(triple.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_cross_target_layouts_are_recognised() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("CACHEDIR.TAG"), "").unwrap();
        let (target, triple) = target_layout(&root.path().join("debug")).unwrap();
        assert_eq!((target.as_path(), triple), (root.path(), None));
        let (target, triple) =
            target_layout(&root.path().join("x86_64-unknown-linux-gnu/release")).unwrap();
        assert_eq!(target, root.path());
        assert_eq!(triple.as_deref(), Some("x86_64-unknown-linux-gnu"));
        let elsewhere = tempfile::tempdir().unwrap();
        assert!(target_layout(&elsewhere.path().join("a/debug")).is_err());
    }
}
