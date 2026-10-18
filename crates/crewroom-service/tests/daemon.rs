use crewroom_service::ConnectionInfo;
use std::{path::Path, process::Command, time::Duration};

struct OwnedDaemon(u32);
impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        // SAFETY: this PID is returned by our authenticated disposable daemon.
        unsafe {
            libc::kill(self.0 as i32, libc::SIGTERM);
        }
    }
}

async fn connect(path: &Path) -> ConnectionInfo {
    for _ in 0..100 {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(info) = serde_json::from_slice::<ConnectionInfo>(&bytes)
        {
            let response = reqwest::Client::new()
                .get(format!("{}/api/health", info.origin))
                .bearer_auth(&info.token)
                .send()
                .await;
            if response.is_ok_and(|r| r.status().is_success()) {
                return info;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Detached daemon did not become healthy");
}

#[tokio::test]
async fn detached_service_survives_launcher_and_stops_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let descriptor = root.path().join("native-connection.json");
    let result = Command::new(env!("CARGO_BIN_EXE_crewroomd"))
        .arg("--background")
        .arg("--port")
        .arg("0")
        .arg("--data-dir")
        .arg(root.path())
        .arg("--roles")
        .arg(root.path().join("roles.json"))
        .status()
        .unwrap();
    assert!(result.success());
    let info = connect(&descriptor).await;
    let daemon = OwnedDaemon(info.pid);
    assert_ne!(info.pid, std::process::id());
    assert!(
        Command::new(env!("CARGO_BIN_EXE_crewroomd"))
            .arg("--data-dir")
            .arg(root.path())
            .arg("--status")
            .output()
            .unwrap()
            .status
            .success()
    );
    drop(daemon);
    for _ in 0..100 {
        if !descriptor.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Daemon did not release its descriptor after SIGTERM");
}

#[tokio::test]
async fn repository_edit_preflight_keeps_observation_responsive_and_replays_after_restart() {
    use serde_json::{Value, json};
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let checkout = root.path().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    // Give only this disposable daemon a wrapper; no global PATH or user Git configuration changes.
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let original_path = std::env::var_os("PATH").unwrap();
    let git = std::env::split_paths(&original_path)
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    symlink(git, bin.join("real-git")).unwrap();
    std::fs::write(bin.join("git"),"#!/bin/sh\ndir=${0%/*}\nif [ -f \"$dir/armed\" ]; then\n  for arg; do\n    if [ \"$arg\" = rev-parse ]; then\n      touch \"$dir/blocked\"\n      while [ -f \"$dir/armed\" ]; do sleep 0.02; done\n      break\n    fi\n  done\nfi\nexec \"$dir/real-git\" \"$@\"\n").unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let daemon_path = std::env::join_paths(
        std::iter::once(bin.clone()).chain(std::env::split_paths(&original_path)),
    )
    .unwrap();
    let start = || {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_crewroomd"))
                .env("PATH", &daemon_path)
                .args(["--background", "--port", "0", "--data-dir"])
                .arg(&data)
                .arg("--roles")
                .arg(root.path().join("roles.json"))
                .status()
                .unwrap()
                .success()
        );
    };
    let descriptor = data.join("native-connection.json");
    start();
    let info = connect(&descriptor).await;
    let daemon = OwnedDaemon(info.pid);
    async fn post(info: &ConnectionInfo, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/api{path}", info.origin))
            .bearer_auth(&info.token)
            .header("Origin", &info.origin)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    let repo: Value = post(
        &info,
        "/repositories",
        json!({"name":"Before","path":checkout,"targetBranch":"main"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let route = format!("/repositories/{}", repo["id"].as_str().unwrap());
    let input = json!({"name":"After","path":checkout,"targetBranch":"main","revision":1,"requestId":uuid::Uuid::new_v4().to_string()});
    let foreign = reqwest::Client::new()
        .post(format!("{}/api{route}", info.origin))
        .bearer_auth(&info.token)
        .header("Origin", "https://foreign.example")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 403);
    std::fs::write(bin.join("armed"), "").unwrap();
    let (pending_info, pending_route, pending_input) = (info.clone(), route.clone(), input.clone());
    let pending =
        tokio::spawn(async move { post(&pending_info, &pending_route, pending_input).await });
    for _ in 0..100 {
        if bin.join("blocked").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        bin.join("blocked").exists(),
        "The actual Git preflight must reach the latch"
    );
    let observed = reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .timeout(Duration::from_secs(1))
        .send()
        .await;
    // Release the real child before asserting so a failed responsiveness check leaves no blocked Git.
    std::fs::remove_file(bin.join("armed")).unwrap();
    let observed: Value = observed.unwrap().json().await.unwrap();
    assert_eq!(observed["repositories"][0]["name"], "Before");
    let response = pending.await.unwrap();
    assert!(response.status().is_success());
    let saved: Value = response.json().await.unwrap();
    assert_eq!(saved["revision"], 2);
    let mut stale = input.clone();
    stale["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(post(&info, &route, stale).await.status(), 409);
    drop(daemon);
    for _ in 0..100 {
        if !descriptor.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!descriptor.exists());
    std::fs::rename(&checkout, root.path().join("moved-repo")).unwrap();
    start();
    let info = connect(&descriptor).await;
    let daemon = OwnedDaemon(info.pid);
    let replay = post(&info, &route, input.clone()).await;
    assert!(replay.status().is_success());
    assert_eq!(replay.json::<Value>().await.unwrap(), saved);
    let mut changed = input;
    changed["name"] = json!("Changed replay");
    assert_eq!(post(&info, &route, changed).await.status(), 409);
    assert_eq!(
        post(&info, "/runs/fixture/continue-later", json!({}))
            .await
            .status(),
        400
    );
    drop(daemon);
    for _ in 0..100 {
        if !descriptor.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Disposable daemon did not stop");
}

#[tokio::test]
async fn creation_and_research_release_workflow_lock_and_recheck_after_git() {
    use serde_json::{Value, json};
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let checkout = std::fs::canonicalize(root.path()).unwrap().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let original_path = std::env::var_os("PATH").unwrap();
    let git = std::env::split_paths(&original_path)
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    symlink(git, bin.join("real-git")).unwrap();
    // Block the first real Git child only; competing configuration preflight must still run.
    std::fs::write(bin.join("git"),"#!/bin/sh\ndir=${0%/*}\nif [ -f \"$dir/armed\" ]; then\n  for arg; do\n    if [ \"$arg\" = rev-parse ] && mkdir \"$dir/claimed\" 2>/dev/null; then\n      touch \"$dir/blocked\"\n      while [ -f \"$dir/armed\" ]; do sleep 0.02; done\n      break\n    fi\n  done\nfi\nexec \"$dir/real-git\" \"$@\"\n").unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let daemon_path = std::env::join_paths(
        std::iter::once(bin.clone()).chain(std::env::split_paths(&original_path)),
    )
    .unwrap();
    let start = || {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_crewroomd"))
                .env("PATH", &daemon_path)
                .args(["--background", "--port", "0", "--data-dir"])
                .arg(&data)
                .arg("--roles")
                .arg(root.path().join("roles.json"))
                .status()
                .unwrap()
                .success()
        );
    };
    async fn post(
        info: &ConnectionInfo,
        path: &str,
        body: Value,
    ) -> reqwest::Result<reqwest::Response> {
        reqwest::Client::new()
            .post(format!("{}/api{path}", info.origin))
            .bearer_auth(&info.token)
            .header("Origin", &info.origin)
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
    }
    async fn wait_latch(bin: &Path) {
        for _ in 0..100 {
            if bin.join("blocked").exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let _ = std::fs::remove_file(bin.join("armed"));
        panic!("Actual Git child did not reach latch");
    }
    async fn stop(daemon: OwnedDaemon, descriptor: &Path) {
        drop(daemon);
        for _ in 0..100 {
            if !descriptor.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Disposable daemon did not stop");
    }
    let descriptor = data.join("native-connection.json");
    start();
    let info = connect(&descriptor).await;
    let daemon = OwnedDaemon(info.pid);
    let creation = json!({"name":"Before","path":checkout,"targetBranch":"main","requestId":uuid::Uuid::new_v4().to_string()});
    std::fs::write(bin.join("armed"), "").unwrap();
    let (pending_info, input) = (info.clone(), creation.clone());
    let pending = tokio::spawn(async move { post(&pending_info, "/repositories", input).await });
    wait_latch(&bin).await;
    let observed = reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .timeout(Duration::from_secs(1))
        .send()
        .await;
    let independent = post(
        &info,
        "/tasks",
        json!({"title":"Independent creation","description":""}),
    )
    .await;
    std::fs::remove_file(bin.join("armed")).unwrap();
    assert!(observed.unwrap().status().is_success());
    assert!(independent.unwrap().status().is_success());
    let response = pending.await.unwrap().unwrap();
    assert!(response.status().is_success());
    let repository: Value = response.json().await.unwrap();
    let task: Value = post(
        &info,
        "/tasks",
        json!({"title":"Research","description":"","repositoryId":repository["id"]}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let task_route = format!("/tasks/{}/refinement", task["id"].as_str().unwrap());
    let accepted = post(
        &info,
        &task_route,
        json!({"revision":1,"sourceRevision":1,"brief":"Accepted","criteria":["Inspect source"]}),
    )
    .await
    .unwrap();
    assert!(
        accepted.status().is_success(),
        "{}",
        accepted.text().await.unwrap()
    );
    let research_route = format!("/tasks/{}/research-runs", task["id"].as_str().unwrap());
    let request = json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()});
    std::fs::remove_file(bin.join("blocked")).unwrap();
    std::fs::remove_dir(bin.join("claimed")).unwrap();
    std::fs::write(bin.join("armed"), "").unwrap();
    let (pending_info, route, input) = (info.clone(), research_route.clone(), request.clone());
    let pending = tokio::spawn(async move { post(&pending_info, &route, input).await });
    wait_latch(&bin).await;
    let observed = reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .timeout(Duration::from_secs(1))
        .send()
        .await;
    let changed=post(&info,&format!("/repositories/{}",repository["id"].as_str().unwrap()),json!({"name":"After","path":checkout,"targetBranch":"main","revision":1,"requestId":uuid::Uuid::new_v4().to_string()})).await;
    std::fs::remove_file(bin.join("armed")).unwrap();
    assert!(observed.unwrap().status().is_success());
    assert!(changed.unwrap().status().is_success());
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), 409);
    let refusal: Value = response.json().await.unwrap();
    assert!(
        refusal.to_string().contains("changed during Git preflight"),
        "{refusal}"
    );
    let snapshot: Value = reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(snapshot["runs"].as_array().unwrap().is_empty());
    let response = post(&info, &research_route, request.clone()).await.unwrap();
    assert!(response.status().is_success());
    let research: Value = response.json().await.unwrap();
    assert_eq!(research["researchSource"]["repositoryRevision"], 2);
    stop(daemon, &descriptor).await;
    std::fs::rename(&checkout, root.path().join("moved")).unwrap();
    start();
    let info = connect(&descriptor).await;
    let daemon = OwnedDaemon(info.pid);
    assert_eq!(
        post(&info, "/repositories", creation.clone())
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        repository
    );
    assert_eq!(
        post(&info, &research_route, request.clone())
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        research
    );
    let mut changed = creation;
    changed["name"] = json!("Different receipt");
    assert_eq!(
        post(&info, "/repositories", changed)
            .await
            .unwrap()
            .status(),
        409
    );
    let mut changed = request;
    changed["activeLimitMs"] = json!(5000);
    assert_eq!(
        post(&info, &research_route, changed)
            .await
            .unwrap()
            .status(),
        409
    );
    stop(daemon, &descriptor).await;
}

#[tokio::test]
async fn signals_after_descriptor_publication_stop_cleanly_even_when_startup_output_blocks() {
    use std::{
        io::{Read, Write},
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::Stdio,
    };
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let root = tempfile::tempdir().unwrap();
        let descriptor = root.path().join("native-connection.json");
        // Hold the startup println after the descriptor is published. This makes
        // the old late signal-handler registration race deterministic.
        let (mut output, mut sink) = UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        loop {
            match output.write(&[b'x'; 8192]) {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        output.set_nonblocking(false).unwrap();
        let output: OwnedFd = output.into();
        let mut child = Command::new(env!("CARGO_BIN_EXE_crewroomd"))
            .args(["--port", "0", "--data-dir"])
            .arg(root.path())
            .arg("--roles")
            .arg(root.path().join("roles.json"))
            .env_remove("CREWROOM_RUNTIME_CONFIG")
            .stdout(Stdio::from(output))
            .spawn()
            .unwrap();
        let info = connect(&descriptor).await;
        assert_eq!(info.pid, child.id());
        // SAFETY: child is a live process spawned and retained by this test.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let drain = std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while sink.read(&mut buffer).is_ok_and(|n| n > 0) {}
        });
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if status.is_err() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        drain.join().unwrap();
        assert!(status.unwrap().success(), "Signal {signal} skipped cleanup");
        assert!(!descriptor.exists());
        let authority = crewroom_core::authority::Authority::acquire_for_execution(root.path());
        assert!(authority.is_ok(), "Signal cleanup retained authority");
    }
}
