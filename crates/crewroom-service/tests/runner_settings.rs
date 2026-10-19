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
    // Approving a model is a runner change too: refused while the
    // environment selects the configuration.
    let (status, refused) = post(
        &info,
        "/runner/profile",
        json!({"harness":"codex","provider":"codex","selector":"model","approve":true,"efforts":["low"]}),
    )
    .await;
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("CREWROOM_RUNTIME_CONFIG"),
        "{refused}"
    );
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
    // A configuration without OMP providers (for example Codex only) saves
    // without inventing an empty models section.
    let mut codex_only = settings(&root);
    codex_only["advanced"] = json!({"catalog": [], "policy": {}});
    codex_only["secrets"] = json!({});
    let (status, saved) = post(&info, "/runner/settings", codex_only).await;
    assert_eq!(status, 200, "{saved}");
    let written: Value =
        serde_json::from_slice(&std::fs::read(root.join("data/runtime.json")).unwrap()).unwrap();
    assert_eq!(written.get("models"), None, "{written}");
    // A version manager's symlinked executable is refused with its real path.
    let link = root.join("codex-latest");
    std::os::unix::fs::symlink("/bin/sh", &link).unwrap();
    let mut linked = settings(&root);
    linked["fields"]["codexBinary"] = json!(link);
    linked["fields"]["codexAuthFile"] = json!(root.join("auth.json"));
    let (status, refused) = post(&info, "/runner/settings", linked).await;
    assert_eq!(status, 400, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("symbolic link"),
        "{refused}"
    );
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

/// A fixture OMP that lists one model from the `fixture` provider. It never
/// runs a model; discovery metadata only.
fn fixture_omp(root: &Path) -> std::path::PathBuf {
    let binary = root.join("omp.py");
    std::fs::write(
        &binary,
        r#"#!/usr/bin/python3
import json,sys
if '--version' in sys.argv:
 print('omp/18.4.12');sys.exit(0)
assert sys.argv[1:]==['models','--json','--no-extensions']
print(json.dumps({'models':[{'provider':'fixture','id':'model','selector':'fixture/model','name':'Example','thinking':['low','high','xhigh'],'input':['text']}]}))
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    binary
}
async fn refreshed(info: &ConnectionInfo) {
    let (status, body) = post(info, "/model-catalog/refresh", json!({})).await;
    assert_eq!(status, 200, "{body}");
    for _ in 0..200 {
        let (view, _) = get(info, "/model-catalog").await;
        if view["state"] != "refreshing" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Discovery did not finish");
}
fn offering<'a>(view: &'a Value, harness: &str) -> &'a Value {
    view["profiles"]["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["harness"] == harness)
        .and_then(|h| h["offerings"].as_array().unwrap().first())
        .unwrap_or_else(|| panic!("no {harness} offering: {}", view["profiles"]))
}

#[tokio::test(flavor = "multi_thread")]
async fn discovered_models_are_approved_explicitly_and_applied_by_a_reload() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let omp = fixture_omp(&root);
    let _daemon = spawn(&root, None);
    let info = connect(&root, None).await;
    // Nothing to approve into before a configuration exists.
    let approve = json!({"harness":"omp","provider":"fixture","selector":"model","approve":true,"efforts":["high"]});
    let (status, refused) = post(&info, "/runner/profile", approve.clone()).await;
    assert_eq!(status, 409, "{refused}");
    let mut body = settings(&root);
    body["fields"]["ompBinary"] = json!(omp);
    body["advanced"]["catalog"] = json!([]);
    let (status, saved) = post(&info, "/runner/settings", body).await;
    assert_eq!(status, 200, "{saved}");
    let info = connect(&root, Some(&info.token)).await;
    refreshed(&info).await;
    let (view, _) = get(&info, "/runner").await;
    assert_eq!(view["active"], true, "{view}");
    assert_eq!(prerequisite(&view, "profiles")["state"], "missing");
    let listed = offering(&view, "omp");
    assert_eq!(listed["selector"], "model", "{listed}");
    assert_eq!(listed["advertisedEfforts"], json!(["low", "high", "xhigh"]));
    assert_eq!(listed["profile"], Value::Null, "discovery approves nothing");
    assert_eq!(listed["approvable"], true, "{listed}");
    // Efforts outside the advertised set are refused before anything is written.
    let mut outside = approve.clone();
    outside["efforts"] = json!(["max"]);
    let (status, refused) = post(&info, "/runner/profile", outside).await;
    assert_eq!(status, 400, "{refused}");

    let (status, saved) = post(&info, "/runner/profile", approve).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["applying"], true, "{saved}");
    assert!(
        saved["notice"]
            .as_str()
            .unwrap()
            .starts_with("Execution profiles saved")
    );
    let written = std::fs::read_to_string(root.join("data/runtime.json")).unwrap();
    assert!(!written.contains(MAIL_SECRET) && !written.contains(PROVIDER_SECRET));
    assert_eq!(mode(&root.join("data/runtime.json")), 0o600);
    let info = connect(&root, Some(&info.token)).await;
    refreshed(&info).await;
    let (view, text) = get(&info, "/runner").await;
    assert!(!text.contains(MAIL_SECRET) && !text.contains(PROVIDER_SECRET));
    assert_eq!(view["active"], true, "{view}");
    assert_eq!(
        view["secrets"],
        json!({"mail":"stored","providers":{"fixture":"stored"}}),
        "secrets survive a profile change"
    );
    assert_eq!(
        view["advanced"]["catalog"],
        json!([{"provider":"fixture","id":"model","api":"openai-responses","inputs":["text"],"efforts":["high"],"tools":true}])
    );
    assert_eq!(
        offering(&view, "omp")["profile"],
        json!({"efforts":["high"],"api":"openai-responses"})
    );
    assert_eq!(prerequisite(&view, "profiles")["state"], "ready");
    // The running runner now pairs the offering with its profile; access is
    // configured, never reported as proven inference.
    let (catalog, _) = get(&info, "/model-catalog").await;
    let route = &catalog["offerings"][0]["offering"];
    assert!(route["executionProfile"].is_string(), "{catalog}");
    assert_eq!(route["capabilities"]["efforts"], json!(["high"]));
    assert_eq!(route["inference"], Value::Null);

    let remove = json!({"harness":"omp","provider":"fixture","selector":"model","approve":false});
    let (status, saved) = post(&info, "/runner/profile", remove.clone()).await;
    assert_eq!(status, 200, "{saved}");
    let info = connect(&root, Some(&info.token)).await;
    let (view, _) = get(&info, "/runner").await;
    assert_eq!(view["advanced"]["catalog"], json!([]), "{view}");
    let (status, refused) = post(&info, "/runner/profile", remove).await;
    assert_eq!(status, 404, "{refused}");
}
