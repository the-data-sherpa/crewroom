//! Actual Unix processes and inherited leases, independent of OMP or provider access.
use crewroom_runtime::{CommandSpec, inspect_owned, prepare_owned, stop_owned};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::time::{Instant, sleep};

struct Fixture {
    _data: tempfile::TempDir,
    root: PathBuf,
    child: Child,
}
impl Fixture {
    async fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let cwd = data.path().canonicalize().unwrap();
        let root = cwd.join("owned");
        prepare_owned(
            &root,
            &CommandSpec {
                command: "/bin/sleep".into(),
                args: vec!["30".into()],
                cwd,
                env: BTreeMap::new(),
            },
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_crewroom-process"))
            .arg("exec")
            .arg(&root)
            .env_clear()
            .env("LANG", "en_US.UTF-8")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let fixture = Self {
            _data: data,
            root,
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observed = inspect_owned(&fixture.root, true).await;
            if observed.state == "live" {
                assert_eq!(observed.owner.unwrap().pid, fixture.child.id() as i32);
                break;
            }
            assert!(Instant::now() < deadline, "{observed:?}");
            sleep(Duration::from_millis(20)).await;
        }
        fixture
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only this exact fixture child exists in the group; clean it on assertion failure too.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn actual_owned_child_can_be_observed_and_stopped() {
    let mut fixture = Fixture::new().await;
    assert_eq!(stop_owned(&fixture.root).await.state, "stopped");
    assert!(!fixture.child.wait().unwrap().success());
    assert_eq!(inspect_owned(&fixture.root, true).await.state, "stopped");
}

#[tokio::test]
async fn changed_start_identity_is_never_signalled() {
    let mut fixture = Fixture::new().await;
    let path = fixture.root.join("owner.json");
    let original = std::fs::read(&path).unwrap();
    let mut owner: Value = serde_json::from_slice(&original).unwrap();
    owner["started"] = Value::String("a different process identity".into());
    std::fs::write(&path, serde_json::to_vec(&owner).unwrap()).unwrap();
    let stopped = stop_owned(&fixture.root).await;
    assert_eq!(stopped.state, "uncertain");
    assert!(stopped.reason.unwrap().contains("PID identity changed"));
    assert!(fixture.child.try_wait().unwrap().is_none());
    std::fs::write(path, original).unwrap();
    assert_eq!(stop_owned(&fixture.root).await.state, "stopped");
    fixture.child.wait().unwrap();
}
