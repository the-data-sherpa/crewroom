use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, Response},
};
use crewroom_runtime::container_command::{ContainerCommand, LogFrames};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
const ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const IMAGE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
#[derive(Default)]
struct Mock {
    found: Option<Value>,
    requests: Vec<String>,
    logs: Vec<u8>,
    fault: Option<(String, Fault)>,
}
#[derive(Clone)]
enum Fault {
    Gate(Arc<Notify>),
    Lost(bool),
    Status(u16),
}
struct Engine {
    root: tempfile::TempDir,
    socket: PathBuf,
    state: Arc<Mutex<Mock>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.server.abort();
    }
}
fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![1, 0, 0, 0];
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}
impl Engine {
    async fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("cr-native-engine-")
            .tempdir()
            .unwrap();
        let socket = root.path().join("s");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(Mock {
            logs: frame("actual résumé 🦀".as_bytes()),
            ..Default::default()
        }));
        let app = Router::new().fallback(serve).with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            root,
            socket,
            state,
            server,
        }
    }
    fn job(&self, name: &str) -> ContainerCommand {
        ContainerCommand::new(self.root.path().join(name), &self.socket).unwrap()
    }
    fn spec(&self) -> Value {
        json!({"image":IMAGE,"workspace":self.root.path(),"command":"/bin/true","args":[],"id":"retained-command"})
    }
    fn fault(&self, phase: &str, fault: Fault) {
        self.state.lock().unwrap().fault = Some((phase.into(), fault));
    }
    fn count(&self, op: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|s| s.as_str() == op)
            .count()
    }
    async fn reached(&self, op: &str) {
        let end = tokio::time::Instant::now() + Duration::from_secs(3);
        while self.count(op) == 0 {
            assert!(tokio::time::Instant::now() < end, "{op} not reached");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
async fn serve(State(state): State<Arc<Mutex<Mock>>>, request: Request<Body>) -> Response<Body> {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let raw = to_bytes(request.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let op = if path == "/containers/create" {
        "create"
    } else if path.ends_with("/start") {
        "start"
    } else if path.ends_with("/kill") {
        "kill"
    } else if path.ends_with("/logs") {
        "logs"
    } else if path.starts_with("/images/") {
        "image"
    } else if method == "DELETE" {
        "delete"
    } else {
        "inspect"
    };
    let (status, bytes, fault) = {
        let mut s = state.lock().unwrap();
        s.requests.push(op.into());
        let fault = if s.fault.as_ref().is_some_and(|(p, _)| p == op) {
            s.fault.take().map(|(_, f)| f)
        } else {
            None
        };
        let apply = !matches!(fault, Some(Fault::Lost(false)) | Some(Fault::Status(_)));
        let mut status = 200;
        let mut value = Value::Null;
        let mut bytes = None;
        if apply {
            match op {
                "create" => {
                    let mut host = body["HostConfig"].clone();
                    for k in ["PidMode", "UsernsMode", "UTSMode"] {
                        host[k] = json!("");
                    }
                    host["IpcMode"] = json!("private");
                    host["Privileged"] = json!(false);
                    let mount = &host["Mounts"][0];
                    let actual = json!([{"Type":"bind","Source":mount["Source"],"Destination":"/workspace","RW":mount["ReadOnly"]!=true}]);
                    s.found = Some(
                        json!({"Id":ID,"Image":body["Image"],"Config":{"Labels":body["Labels"],"User":body["User"],"Tty":false,"NetworkDisabled":true,"Cmd":body["Cmd"],"WorkingDir":"/workspace"},"HostConfig":host,"Mounts":actual,"State":{"Running":false,"Restarting":false,"Pid":0,"Status":"created","ExitCode":0}}),
                    );
                    status = 201;
                    value = json!({"Id":ID});
                }
                "start" => {
                    if let Some(v) = s.found.as_mut() {
                        let running = v["Config"]["Cmd"][0] != "/bin/true";
                        v["State"] = json!({"Running":running,"Restarting":false,"Pid":if running{100}else{0},"Status":if running{"running"}else{"exited"},"ExitCode":0});
                        status = 204;
                    } else {
                        status = 404;
                    }
                }
                "kill" => {
                    if let Some(v) = s.found.as_mut() {
                        v["State"] = json!({"Running":false,"Restarting":false,"Pid":0,"Status":"exited","ExitCode":137});
                        status = 204;
                    } else {
                        status = 404;
                    }
                }
                "delete" => {
                    s.found = None;
                    status = 204;
                }
                "image" => value = json!({"Os":"linux","Architecture":"arm64"}),
                "logs" => bytes = Some(s.logs.clone()),
                _ => {
                    if let Some(found) = s.found.as_ref() {
                        value = found.clone();
                    } else {
                        status = 404;
                    }
                }
            }
        }
        if let Some(Fault::Status(code)) = fault {
            status = code;
        }
        let bytes = bytes.unwrap_or_else(|| {
            if value.is_null() {
                vec![]
            } else {
                serde_json::to_vec(&value).unwrap()
            }
        });
        (status, bytes, fault)
    };
    match fault {
        Some(Fault::Gate(g)) => g.notified().await,
        Some(Fault::Lost(_)) => tokio::time::sleep(Duration::from_secs(6)).await,
        _ => (),
    }
    Response::builder()
        .status(status)
        .body(Body::from(bytes))
        .unwrap()
}
#[test]
fn docker_log_framing_is_incremental_bounded_and_rejects_truncation() {
    let raw = [frame(b"one"), frame(&[]), frame("🦀\n".as_bytes())].concat();
    for chunk in 1..=raw.len() {
        let mut reader = LogFrames::default();
        let mut out = Vec::new();
        for part in raw.chunks(chunk) {
            for frame in reader.push(part).unwrap() {
                out.extend(frame);
            }
        }
        reader.finish().unwrap();
        assert_eq!(out, "one🦀\n".as_bytes());
    }
    for n in [1, 7, 9, 10] {
        let mut reader = LogFrames::default();
        reader.push(&raw[..n]).unwrap();
        assert!(reader.finish().is_err());
    }
    for bad in [
        vec![3, 0, 0, 0, 0, 0, 0, 0],
        vec![1, 1, 0, 0, 0, 0, 0, 0],
        vec![1, 0, 0, 0, 0, 64, 0, 1],
    ] {
        assert!(LogFrames::default().push(&bad).is_err());
    }
}
#[tokio::test]
async fn cancelled_start_and_concurrent_stop_preserve_the_launch_barrier() {
    let e = Engine::new().await;
    let job = e.job("job");
    let gate = Arc::new(Notify::new());
    e.fault("create", Fault::Gate(gate.clone()));
    let clone = job.clone();
    let spec = e.spec();
    let starter = tokio::spawn(async move { clone.start(spec).await });
    e.reached("create").await;
    assert_eq!(job.inspect().await["state"], "uncertain");
    assert!(
        job.start(e.spec())
            .await
            .unwrap_err()
            .to_string()
            .contains("durable intent")
    );
    starter.abort();
    let _ = starter.await;
    let clone = job.clone();
    let stopper = tokio::spawn(async move { clone.stop().await });
    let end = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let v: Value =
            serde_json::from_slice(&std::fs::read(job.root.join("container.json")).unwrap())
                .unwrap();
        if v["stopRequested"] == true {
            break;
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!stopper.is_finished());
    gate.notify_one();
    assert_eq!(stopper.await.unwrap().unwrap()["state"], "stopped");
    assert_eq!(e.count("start"), 0);
    assert_eq!(e.count("create"), 1);
    let receipt = job.receipt().await.unwrap();
    assert_eq!(receipt["started"], false);
    assert_eq!(receipt["completed"], false);
    job.remove().await.unwrap();
    assert!(e.state.lock().unwrap().found.is_none());
}
#[tokio::test]
async fn simultaneous_starts_have_one_durable_owner() {
    let e = Engine::new().await;
    let job = e.job("job");
    let (a, b) = tokio::join!(job.start(e.spec()), job.start(e.spec()));
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(e.count("create"), 1);
    assert_eq!(e.count("start"), 1);
    job.remove().await.unwrap();
}
#[tokio::test]
async fn lost_create_and_delete_replies_reconcile_without_reexecution() {
    let e = Engine::new().await;
    let job = e.job("job");
    e.fault("create", Fault::Lost(true));
    assert!(job.start(e.spec()).await.is_err());
    let reopened = e.job("job");
    assert_eq!(reopened.inspect().await["state"], "stopped");
    assert!(reopened.start(e.spec()).await.is_err());
    let receipt = reopened.receipt().await.unwrap();
    assert_eq!(receipt["started"], false);
    e.fault("delete", Fault::Lost(true));
    assert!(reopened.remove().await.is_err());
    assert_eq!(e.job("job").inspect().await["state"], "stopped");
    assert_eq!(e.job("job").receipt().await.unwrap(), receipt);
    assert_eq!(e.count("create"), 1);
    assert_eq!(e.count("start"), 0);
    assert_eq!(e.count("delete"), 1);
}
#[tokio::test]
async fn lost_start_stays_uncertain_until_engine_execution_is_observed() {
    for applied in [false, true] {
        let e = Engine::new().await;
        let job = e.job("job");
        e.fault("start", Fault::Lost(applied));
        assert!(job.start(e.spec()).await.is_err());
        let state = job.inspect().await;
        assert_eq!(
            state["state"],
            if applied { "stopped" } else { "uncertain" }
        );
        if applied {
            let receipt = job.receipt().await.unwrap();
            assert_eq!(receipt["started"], false);
            assert_eq!(receipt["completed"], false);
            job.remove().await.unwrap();
        } else {
            assert!(job.receipt().await.is_err());
            assert_eq!(job.stop().await.unwrap()["state"], "uncertain");
            job.remove().await.unwrap();
            assert_eq!(e.count("delete"), 0);
        }
        assert_eq!(e.count("start"), 1);
    }
}
#[tokio::test]
async fn changed_containment_and_copied_or_linked_journals_cannot_authorize_cleanup() {
    let e = Engine::new().await;
    let job = e.job("job");
    let mut spec = e.spec();
    spec["command"] = json!("/bin/sleep");
    job.start(spec).await.unwrap();
    let original = e.state.lock().unwrap().found.clone().unwrap();
    for (path, value) in [
        ("/Config/Labels/io.crewroom.command", json!("foreign")),
        ("/Image", json!("sha256:bad")),
        ("/HostConfig/NetworkMode", json!("host")),
        ("/HostConfig/Privileged", json!(true)),
        ("/HostConfig/ReadonlyRootfs", json!(false)),
        ("/HostConfig/Memory", json!(1)),
        ("/HostConfig/Tmpfs/~1tmp", json!("rw,exec")),
        ("/HostConfig/CapDrop", json!([])),
        ("/HostConfig/SecurityOpt", json!([])),
        ("/HostConfig/PidsLimit", json!(0)),
        ("/HostConfig/NanoCpus", json!(0)),
        ("/HostConfig/RestartPolicy/Name", json!("always")),
        ("/HostConfig/Mounts/0/Source", json!("/foreign")),
        ("/HostConfig/Mounts/0/ReadOnly", json!("false")),
        ("/Mounts/0/Source", json!("/foreign")),
        ("/Config/Tty", json!(true)),
        ("/Config/NetworkDisabled", json!(false)),
        ("/Config/Cmd", json!(["/foreign"])),
        ("/Config/WorkingDir", json!("/foreign")),
        ("/Config/User", json!("other:other")),
        ("/HostConfig/PidMode", json!("host")),
        ("/HostConfig/IpcMode", json!("host")),
    ] {
        let mut altered = original.clone();
        *altered.pointer_mut(path).unwrap() = value;
        e.state.lock().unwrap().found = Some(altered);
        assert_eq!(job.inspect().await["state"], "uncertain", "{path}");
        assert_eq!(job.stop().await.unwrap()["state"], "uncertain", "{path}");
        job.remove().await.unwrap();
        assert_eq!(e.count("kill"), 0);
        assert_eq!(e.count("delete"), 0);
    }
    e.state.lock().unwrap().found = Some(original);
    let copied = e.job("copied");
    std::fs::create_dir(&copied.root).unwrap();
    std::fs::copy(
        job.root.join("container.json"),
        copied.root.join("container.json"),
    )
    .unwrap();
    assert_eq!(copied.stop().await.unwrap()["state"], "uncertain");
    let linked = e.job("linked");
    std::os::unix::fs::symlink(&job.root, &linked.root).unwrap();
    assert_eq!(linked.stop().await.unwrap()["state"], "uncertain");
    assert_eq!(job.stop().await.unwrap()["state"], "stopped");
    job.remove().await.unwrap();
}
#[tokio::test]
async fn final_logs_must_be_complete_and_bounded_before_removal() {
    let e = Engine::new().await;
    let job = e.job("job");
    job.start(e.spec()).await.unwrap();
    for logs in [
        vec![1],
        frame(b"incomplete")[..10].to_vec(),
        vec![0; 4 * 1024 * 1024 + 1],
    ] {
        e.state.lock().unwrap().logs = logs;
        assert!(job.receipt().await.is_err());
        assert!(job.remove().await.is_err());
        assert_eq!(e.count("delete"), 0);
    }
    e.state.lock().unwrap().logs = frame(b"complete");
    let receipt = job.receipt().await.unwrap();
    job.remove().await.unwrap();
    assert_eq!(e.job("job").receipt().await.unwrap(), receipt);
    assert_eq!(receipt["completed"], true);
}
#[tokio::test]
async fn redirected_engine_replies_do_not_confirm_start() {
    let e = Engine::new().await;
    let job = e.job("job");
    e.fault("start", Fault::Status(302));
    assert!(job.start(e.spec()).await.is_err());
    assert_eq!(job.inspect().await["state"], "uncertain");
    assert!(job.receipt().await.is_err());
    assert_eq!(e.count("start"), 1);
}
#[tokio::test]
async fn failed_or_cancelled_live_observation_does_not_stop_the_command() {
    let e = Engine::new().await;
    let job = e.job("job");
    let mut spec = e.spec();
    spec["command"] = json!("/bin/sleep");
    job.start(spec).await.unwrap();
    assert!(
        job.follow(
            |_| Err(crewroom_core::Error::Conflict("Observer failure".into())),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(job.inspect().await["state"], "running");
    let cancel = CancellationToken::new();
    cancel.cancel();
    job.follow(|_| panic!("Cancelled observer emitted output"), cancel)
        .await
        .unwrap();
    assert_eq!(job.inspect().await["state"], "running");
    assert_eq!(e.count("kill"), 0);
    job.stop().await.unwrap();
    job.remove().await.unwrap();
}

#[tokio::test]
async fn active_start_reply_is_a_barrier_even_when_the_engine_already_runs() {
    let e = Engine::new().await;
    let job = e.job("job");
    let gate = Arc::new(Notify::new());
    e.fault("start", Fault::Gate(gate.clone()));
    let clone = job.clone();
    let mut spec = e.spec();
    spec["command"] = json!("/bin/sleep");
    let starter = tokio::spawn(async move { clone.start(spec).await });
    e.reached("start").await;
    assert_eq!(job.inspect().await["state"], "uncertain");
    starter.abort();
    let _ = starter.await;
    let clone = job.clone();
    let stopper = tokio::spawn(async move { clone.stop().await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!stopper.is_finished());
    assert_eq!(e.count("kill"), 0);
    gate.notify_one();
    assert_eq!(stopper.await.unwrap().unwrap()["state"], "stopped");
    assert_eq!(e.count("kill"), 1);
    assert_eq!(e.count("start"), 1);
    let receipt = job.receipt().await.unwrap();
    assert_eq!(receipt["started"], true);
    assert_eq!(receipt["completed"], false);
    job.remove().await.unwrap();
}
#[tokio::test]
async fn completion_rechecks_engine_state_after_final_logs() {
    let e = Engine::new().await;
    let job = e.job("job");
    job.start(e.spec()).await.unwrap();
    let original = e.state.lock().unwrap().found.clone().unwrap();
    let gate = Arc::new(Notify::new());
    e.fault("logs", Fault::Gate(gate.clone()));
    let clone = job.clone();
    let reading = tokio::spawn(async move { clone.receipt().await });
    e.reached("logs").await;
    e.state.lock().unwrap().found.as_mut().unwrap()["State"]["Running"] = json!(true);
    gate.notify_one();
    assert!(reading.await.unwrap().is_err());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(job.root.join("container.json")).unwrap()).unwrap();
    assert!(saved["receipt"].is_null());
    job.remove().await.unwrap();
    assert_eq!(e.count("delete"), 0);
    e.state.lock().unwrap().found = Some(original);
    job.remove().await.unwrap();
}

#[tokio::test]
async fn invalid_specs_are_refused_before_durable_admission() {
    let e = Engine::new().await;
    let cases = [
        ("image", json!("python:latest")),
        ("image", json!(format!("sha256:{}", "A".repeat(64)))),
        ("workspace", json!("relative")),
        ("command", json!("relative")),
        ("args", json!([1])),
        ("args", Value::Null),
        ("timeoutMs", json!(99)),
        ("timeoutMs", json!(100.5)),
        ("timeoutMs", json!(3_600_001)),
        ("timeoutMs", Value::Null),
        ("timeoutMs", json!("100")),
        ("memoryMiB", json!(127)),
        ("memoryMiB", json!(8193)),
        ("memoryMiB", json!(128.1)),
        ("memoryMiB", Value::Null),
        ("tmpExecutable", json!("true")),
        ("readOnlyWorkspace", Value::Null),
    ];
    for (i, (key, value)) in cases.into_iter().enumerate() {
        let job = e.job(&format!("bad-{i}"));
        let mut spec = e.spec();
        spec[key] = value;
        assert!(job.start(spec).await.is_err(), "{key}");
        assert!(!job.root.join("container.json").exists());
    }
    assert_eq!(e.count("create"), 0);
    let job = e.job("valid");
    let mut spec = e.spec();
    spec["memoryMiB"] = json!(128.0);
    spec["timeoutMs"] = json!(100.0);
    job.start(spec).await.unwrap();
    assert_eq!(
        job.receipt().await.unwrap()["spec"]["id"],
        "retained-command"
    );
    job.remove().await.unwrap();
}
#[tokio::test]
async fn corrupt_durable_flags_do_not_become_stopped_or_completed_proof() {
    let e = Engine::new().await;
    let job = e.job("job");
    job.start(e.spec()).await.unwrap();
    let path = job.root.join("container.json");
    let original = std::fs::read(&path).unwrap();
    for (key, value) in [
        ("root", Value::Null),
        ("token", json!("foreign")),
        ("startRequested", json!("true")),
        ("startConfirmed", json!("false")),
        ("stopRequested", Value::Null),
        ("timedOut", json!(1)),
        ("startedAt", json!(-1)),
        ("completed", json!({"exitCode":"0"})),
        ("removed", json!(true)),
        ("removalRequested", json!(true)),
        ("receipt", json!({"completed":true})),
    ] {
        let mut altered: Value = serde_json::from_slice(&original).unwrap();
        altered[key] = value;
        std::fs::write(&path, serde_json::to_vec(&altered).unwrap()).unwrap();
        assert_eq!(job.inspect().await["state"], "uncertain", "{key}");
        assert_eq!(job.stop().await.unwrap()["state"], "uncertain", "{key}");
        assert!(job.receipt().await.is_err());
        job.remove().await.unwrap();
        assert_eq!(e.count("delete"), 0);
    }
    std::fs::write(&path, original).unwrap();
    let receipt = job.receipt().await.unwrap();
    assert_eq!(receipt["completed"], true);
    job.remove().await.unwrap();
    assert_eq!(job.receipt().await.unwrap(), receipt);
}

#[tokio::test]
async fn malformed_engine_liveness_never_becomes_stopped_proof() {
    let e = Engine::new().await;
    let job = e.job("job");
    job.start(e.spec()).await.unwrap();
    let original = e.state.lock().unwrap().found.clone().unwrap();
    for field in ["Running", "Restarting"] {
        for value in [
            json!("true"),
            json!("false"),
            json!(0),
            Value::Null,
            json!({}),
        ] {
            let mut altered = original.clone();
            altered["State"][field] = value;
            e.state.lock().unwrap().found = Some(altered);
            assert_eq!(job.inspect().await["state"], "uncertain", "{field}");
            assert_eq!(job.stop().await.unwrap()["state"], "uncertain");
            assert!(job.receipt().await.is_err());
            job.remove().await.unwrap();
            assert_eq!(e.count("delete"), 0);
        }
    }
    e.state.lock().unwrap().found = Some(original);
    job.remove().await.unwrap();
}
