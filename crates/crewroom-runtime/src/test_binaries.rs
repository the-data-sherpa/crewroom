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
/// The fresh-worktree preparation step (`scripts/prepare_worktree.py`) that also runs [`BUILD`].
const PREPARE_SCRIPT: &str = "scripts/prepare_worktree.py";
const PREPARE_FLAG: &str = "--build-helpers";

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
        panic!("{}", missing_helpers_message(profile_dir, reason));
    }
    helpers
}

fn missing_helpers_message(profile_dir: &Path, reason: &str) -> String {
    format!(
        "Runtime helper binaries are missing from {} and could not be built ({reason}). Run `{BUILD}` with the same CARGO_TARGET_DIR, or `python3 {PREPARE_SCRIPT} {PREPARE_FLAG}`.",
        profile_dir.display()
    )
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
    fn missing_helpers_message_names_the_real_preparation_step() {
        let message = missing_helpers_message(Path::new("/x/debug"), "no cargo");
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let script = workspace.join(PREPARE_SCRIPT);
        let source = std::fs::read_to_string(&script)
            .unwrap_or_else(|e| panic!("{} named in the message: {e}", script.display()));
        assert!(message.contains(&format!("`python3 {PREPARE_SCRIPT} {PREPARE_FLAG}`")));
        assert!(message.contains(&format!("`{BUILD}`")));
        assert!(
            source.contains(&format!("'{PREPARE_FLAG}'")),
            "{PREPARE_FLAG} is a flag of the script"
        );
        // The script's helper build runs the same command the message names.
        let argv = BUILD
            .split(' ')
            .map(|arg| format!("'{arg}'"))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(source.contains(&argv), "script builds with `{BUILD}`");
    }

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
