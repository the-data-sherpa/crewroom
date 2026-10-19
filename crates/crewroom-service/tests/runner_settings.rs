//! Settings → Runner (FOR-83) against the real `crewroomd` binary with an
//! isolated data folder: create the runner configuration through the API,
//! confirm it is written owner-only with secrets only in the encrypted store,
//! and that the daemon reloads in place. Simulated: the Mail endpoint is an
//! unused loopback port and no agent is launched.
use crewroom_service::{ConnectionInfo, Service};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command},
    time::Duration,
};

const MAIL_SECRET: &str = "synthetic-runner-mail-token";
const PROVIDER_SECRET: &str = "synthetic-runner-provider-key";

fn crewroomd() -> &'static Path {
    let daemon = Path::new(env!("CARGO_BIN_EXE_crewroomd"));
    crewroom_runtime::test_binaries::ensure(daemon.parent().unwrap());
    daemon
}
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        // SAFETY: this PID belongs to the disposable child this test spawned.
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let _ = self.0.wait();
    }
}
fn spawn(root: &Path, runtime: Option<&Path>) -> Daemon {
    let mut command = Command::new(crewroomd());
    command
        .args(["--port", "0", "--data-dir"])
        .arg(root.join("data"))
        .arg("--roles")
        .arg(root.join("roles.json"))
        .env_remove("CREWROOM_RUNTIME_CONFIG")
        .stdout(std::process::Stdio::null());
    if let Some(runtime) = runtime {
        command.env("CREWROOM_RUNTIME_CONFIG", runtime);
    }
    Daemon(command.spawn().unwrap())
}
/// Waits for a healthy daemon whose token differs from `previous`.
async fn connect(root: &Path, previous: Option<&str>) -> ConnectionInfo {
    let path = root.join("data/native-connection.json");
    for _ in 0..400 {
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(info) = serde_json::from_slice::<ConnectionInfo>(&bytes)
            && previous != Some(info.token.as_str())
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
    panic!("Daemon did not become healthy");
}
async fn get(info: &ConnectionInfo, path: &str) -> (Value, String) {
    let text = reqwest::Client::new()
        .get(format!("{}/api{path}", info.origin))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    (serde_json::from_str(&text).unwrap(), text)
}
async fn post(info: &ConnectionInfo, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/api{path}", info.origin))
        .bearer_auth(&info.token)
        .header("Origin", &info.origin)
        .json(&body)
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}
fn prerequisite<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["prerequisites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("missing prerequisite {id}: {view}"))
}
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
fn settings(root: &Path) -> Value {
    json!({
        "fields": {
            "root": root.join("hosts"),
            "mailUrl": "http://127.0.0.1:9/mcp/",
            "mailProjectRoot": root,
        },
        "advanced": {
            "catalog": [{"provider":"fixture","id":"model","api":"openai-responses","inputs":["text"],"efforts":["low"],"tools":true}],
            "policy": {},
            "models": {"providers": {"fixture": {"baseUrl": "http://127.0.0.1:9", "api": "openai-responses", "apiKey": PROVIDER_SECRET}}},
        },
        "secrets": {"mail": MAIL_SECRET},
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn runner_settings_are_saved_privately_and_applied_by_an_in_place_reload() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let _daemon = spawn(&root, None);
    let info = connect(&root, None).await;
    let (view, _) = get(&info, "/runner").await;
    assert_eq!(view["source"]["kind"], "none", "{view}");
    assert_eq!(view["active"], false);
    assert_eq!(view["editable"], true);
    assert_eq!(view["restart"]["available"], true, "{view}");
    for id in ["runtime-root", "mail", "docker", "profiles"] {
        assert_eq!(prerequisite(&view, id)["state"], "missing", "{id}: {view}");
        assert!(prerequisite(&view, id)["remedy"].is_string(), "{id}");
    }
    for p in view["prerequisites"].as_array().unwrap() {
        assert!(
            ["ready", "missing", "failing"].contains(&p["state"].as_str().unwrap()),
            "{p}"
        );
    }
    let (summary, _) = get(&info, "/workspace").await;
    assert_eq!(
        summary["runner"],
        json!({"active":false,"source":"none","error":null})
    );

    // Validation runs before anything is written.
    let mut incomplete = settings(&root);
    incomplete["secrets"] = json!({});
    let (status, refused) = post(&info, "/runner/settings", incomplete).await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["error"], "Enter the Rust Agent Mail token.");
    let mut misplaced = settings(&root);
    misplaced["advanced"]["root"] = json!("/elsewhere");
    let (status, refused) = post(&info, "/runner/settings", misplaced).await;
    assert_eq!(status, 400, "{refused}");
    assert!(!root.join("data/runtime.json").exists());

    // Save: written owner-only, secrets encrypted, then applied by a reload.
    let (status, saved) = post(&info, "/runner/settings", settings(&root)).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["applying"], true, "{saved}");
    let file = root.join("data/runtime.json");
    assert_eq!(mode(&file), 0o600);
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(!written.contains(MAIL_SECRET) && !written.contains(PROVIDER_SECRET));
    assert!(written.contains("$crewroomSecret"));
    let credentials = std::fs::read_to_string(root.join("data/credentials.json")).unwrap();
    assert!(!credentials.contains(MAIL_SECRET) && !credentials.contains(PROVIDER_SECRET));
    let reloaded = connect(&root, Some(&info.token)).await;
    assert_eq!(reloaded.origin, info.origin, "the reload keeps the port");
    let (health, _) = get(&reloaded, "/health").await;
    assert_eq!(health["executionAvailable"], true, "{health}");
    let (view, text) = get(&reloaded, "/runner").await;
    assert!(!text.contains(MAIL_SECRET) && !text.contains(PROVIDER_SECRET));
    assert_eq!(view["source"]["kind"], "settings", "{view}");
    assert_eq!(view["active"], true);
    assert_eq!(view["pending"], false, "{view}");
    assert_eq!(view["error"], Value::Null, "{view}");
    assert_eq!(
        view["secrets"],
        json!({"mail":"stored","providers":{"fixture":"stored"}})
    );
    assert_eq!(prerequisite(&view, "runtime-root")["state"], "ready");
    assert_eq!(prerequisite(&view, "profiles")["state"], "ready");
    assert_eq!(prerequisite(&view, "omp-providers")["state"], "ready");
    // Nothing listens on the configured Mail port: shown as failing, not ready.
    assert_eq!(prerequisite(&view, "mail")["state"], "failing", "{view}");
    assert_eq!(
        view["advanced"]["models"]["providers"]["fixture"].get("apiKey"),
        None
    );
    let (summary, _) = get(&reloaded, "/workspace").await;
    assert_eq!(summary["runner"]["active"], true);
    assert_eq!(
        summary["capabilities"]["execution"], true,
        "{}",
        summary["capabilities"]
    );

    // Edit from the projected form without re-entering secrets: kept.
    let mut edit = json!({"fields": view["form"], "advanced": view["advanced"]});
    edit["advanced"]["policy"] = json!({"equivalence": []});
    let (status, saved) = post(&reloaded, "/runner/settings", edit).await;
    assert_eq!(status, 200, "{saved}");
    let again = connect(&root, Some(&reloaded.token)).await;
    let (view, _) = get(&again, "/runner").await;
    assert_eq!(view["active"], true, "{view}");
    assert_eq!(view["advanced"]["policy"], json!({"equivalence": []}));
    assert_eq!(
        view["secrets"],
        json!({"mail":"stored","providers":{"fixture":"stored"}})
    );

    // A broken saved file leaves the service reachable with the reason shown.
    std::fs::write(&file, "{not json").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (status, applied) = post(&again, "/runner/apply", json!({})).await;
    assert_eq!(status, 200, "{applied}");
    let broken = connect(&root, Some(&again.token)).await;
    let (view, _) = get(&broken, "/runner").await;
    assert_eq!(view["active"], false, "{view}");
    assert!(
        view["error"].as_str().unwrap().contains("not valid JSON"),
        "{view}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn environment_configuration_stays_active_and_read_only() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let hosts = root.join("hosts");
    let runtime = root.join("runtime-env.json");
    std::fs::write(
        &runtime,
        json!({"root":hosts,"socket":hosts.join("tmux.sock"),"mail":{"url":"http://127.0.0.1:9/mcp/","token":"synthetic-inline-mail","projectRoot":root,"retentionSeconds":86400,"atcDisabled":true},"catalog":[],"policy":{}}).to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _daemon = spawn(&root, Some(&runtime));
    let info = connect(&root, None).await;
    let (view, text) = get(&info, "/runner").await;
    assert!(!text.contains("synthetic-inline-mail"));
    assert_eq!(view["source"]["kind"], "environment", "{view}");
    assert_eq!(view["active"], true);
    assert_eq!(view["editable"], false);
    assert_eq!(view["secrets"]["mail"], "inline");
    assert!(
        view["editReason"]
            .as_str()
            .unwrap()
            .contains("CREWROOM_RUNTIME_CONFIG")
    );
    let (status, refused) = post(&info, "/runner/settings", settings(&root)).await;
    assert_eq!(status, 409, "{refused}");
    assert!(!root.join("data/runtime.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_embedded_service_saves_but_states_that_a_restart_is_needed() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let service = Service::start(&root.join("data"), &root.join("roles.json"), 0)
        .await
        .unwrap();
    let info = service.info.clone();
    let (view, _) = get(&info, "/runner").await;
    assert_eq!(view["restart"]["available"], false);
    assert!(
        view["restart"]["reason"]
            .as_str()
            .unwrap()
            .contains("Stop and start")
    );
    let (status, refused) = post(&info, "/runner/apply", json!({})).await;
    assert_eq!(status, 409, "{refused}");
    let (status, saved) = post(&info, "/runner/settings", settings(&root)).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["applying"], false);
    assert!(saved["notice"].as_str().unwrap().contains("stop and start"));
    let (view, _) = get(&info, "/runner").await;
    assert_eq!(view["source"]["kind"], "settings");
    assert_eq!(view["pending"], true, "saved but not applied: {view}");
    assert_eq!(view["active"], false);
    // Cross-origin mutations are refused like every other mutation.
    let response = reqwest::Client::new()
        .post(format!("{}/api/runner/apply", info.origin))
        .bearer_auth(&info.token)
        .header("Origin", "http://example.invalid")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    service.close().await.unwrap();
}
