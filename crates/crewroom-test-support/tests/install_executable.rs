use crewroom_test_support::install_executable;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// FOR-86/FOR-89: while sibling threads keep spawning children, every
/// installed fixture must start. A script written with `std::fs::write` in this
/// process fails here with `Text file busy` within a few hundred installs.
#[test]
fn fixture_executables_start_while_sibling_threads_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let spawners: Vec<_> = (0..4)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("/bin/true").status();
                }
            })
        })
        .collect();
    let installers: Vec<_> = (0..4)
        .map(|w| {
            let root = temp.path().to_owned();
            std::thread::spawn(move || {
                for i in 0..150 {
                    let path = root.join(format!("fixture-{w}-{i}"));
                    install_executable(&path, "#!/bin/sh\nexit 0\n");
                    let status = std::process::Command::new(&path).status();
                    assert!(
                        status.as_ref().is_ok_and(|s| s.success()),
                        "{i}: {status:?}"
                    );
                }
            })
        })
        .collect();
    let installed: Vec<_> = installers.into_iter().map(|t| t.join()).collect();
    stop.store(true, Ordering::Relaxed);
    for spawner in spawners {
        spawner.join().unwrap();
    }
    for result in installed {
        result.unwrap();
    }
}

/// The install is owner-only, leaves no staging files, and replaces an
/// executable that is still running without disturbing it.
#[test]
fn install_is_private_clean_and_replaces_a_running_executable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tool");
    let started = temp.path().join("started");
    let release = temp.path().join("release");
    install_executable(
        &path,
        format!(
            "#!/bin/sh\n: > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\necho first\n",
            started.display(),
            release.display()
        ),
    );
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "{mode:o}");
    let running = std::process::Command::new(&path)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Replace only once the shell has opened the first script.
    while !started.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    install_executable(&path, "#!/bin/sh\necho second\n");
    std::fs::write(&release, "").unwrap();
    let first = running.wait_with_output().unwrap();
    assert!(first.status.success(), "{first:?}");
    assert_eq!(String::from_utf8_lossy(&first.stdout), "first\n");
    let second = std::process::Command::new(&path).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&second.stdout), "second\n");
    let mut names: Vec<_> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["release", "started", "tool"]);
}
