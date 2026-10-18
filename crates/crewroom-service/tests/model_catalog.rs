use crewroom_service::{ConnectionInfo, Service, ServiceOptions};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    time::Duration,
};

async fn post(info: &ConnectionInfo, path: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/api/{path}", info.origin))
        .bearer_auth(&info.token)
        .header("Origin", &info.origin)
        .json(&body)
        .send()
        .await
        .unwrap()
}
async fn get(info: &ConnectionInfo, path: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{}/api/{path}", info.origin))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
fn options(root: &Path) -> ServiceOptions {
    let runtime = root.join("runtime");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&runtime)
        .unwrap();
    let binary = root.join("omp.py");
    if !binary.exists() {
        std::fs::write(&binary,r#"#!/usr/bin/python3
import json,sys,time,pathlib,os
if '--version' in sys.argv:
 print('omp/18.4.12');sys.exit(0)
assert sys.argv[1:]==['models','--json','--no-extensions']
root=pathlib.Path(__file__).parent
scenario=json.loads((root/'scenario.json').read_text()) if (root/'scenario.json').exists() else {}
(root/'discovery-pid').write_text(str(os.getpid()))
time.sleep(scenario.get('delay',.5))
if scenario.get('fail'):
 print('credential-canary account-canary');sys.exit(1)
if scenario.get('empty'):
 print('{"models":[]}');sys.exit(0)
print(json.dumps({'models':[{'provider':'fixture','id':'model','selector':'fixture/model','name':'Example','thinking':['low','high','xhigh'],'input':['text'],'apiKey':'credential-canary'}],'account':'account-canary'}))
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let target = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let helper = target.join("crewroom-process");
    assert!(
        helper.is_file(),
        "Build the production helper first: cargo build --locked -p crewroom-runtime --bins"
    );
    ServiceOptions {
        runtime: Some(
            json!({"root":runtime,"socket":runtime.join("tmux.sock"),"ompBinary":binary,
        "mail":{"atcDisabled":true,"retentionSeconds":600,"projectRoot":root.join("mail"),"url":"http://127.0.0.1:4123/mcp/","token":"credential-canary"},
        "catalog":[{"provider":"fixture","id":"model","api":"openai-responses","tools":true,"inputs":["text"],"efforts":["low","high","xhigh"]}],
        "policy":{"default":{"model":"fixture/model","effort":"high"}},"models":{"providers":{"fixture":{"baseUrl":"http://127.0.0.1:5555","apiKey":"credential-canary"}}}}),
        ),
        helper,
        host: target.join("crewroom-host"),
        tmux: "/usr/bin/tmux".into(),
        jira: None,
        publication: vec![],
        publication_unavailable: vec![],
        environment: json!({"PATH":"/usr/bin:/bin","HOME":root}),
        github: None,
    }
}

#[tokio::test]
async fn authenticated_refresh_and_preview_are_responsive_credential_free_and_restart_safe() {
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
    let setup = options(root.path());
    let service = Service::start_with_options(root.path(), &roles, 0, setup)
        .await
        .unwrap();
    let info = &service.info;
    for path in ["model-catalog/refresh", "model-preview"] {
        assert_eq!(
            reqwest::Client::new()
                .post(format!("{}/api/{path}", info.origin))
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            reqwest::Client::new()
                .post(format!("{}/api/{path}", info.origin))
                .bearer_auth(&info.token)
                .header("Origin", "https://invalid.example")
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        post(
            info,
            "model-catalog/refresh",
            json!({"apiKey":"credential-canary"})
        )
        .await
        .status(),
        400
    );
    let first: Value = post(info, "model-catalog/refresh", json!({}))
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let repeated: Value = post(info, "model-catalog/refresh", json!({}))
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["generation"], repeated["generation"]);
    tokio::time::timeout(Duration::from_secs(2), get(info, "workspace"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while get(info, "model-catalog").await["state"] == "refreshing" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let catalog = get(info, "model-catalog").await;
    assert_eq!(catalog["state"], "partial");
    assert_eq!(catalog["offerings"][0]["fresh"], true);
    assert!(!catalog.to_string().contains("credential-canary"));
    assert!(!catalog.to_string().contains("account-canary"));
    let workspace = get(info, "workspace").await;
    assert!(workspace["runs"].as_array().unwrap().is_empty());
    let selection = catalog["offerings"][0]["offering"]["key"].clone();
    let mut jobs = workspace["modelJobs"].clone();
    for job in ["code", "judgment", "review", "fast"] {
        jobs["jobs"][job] = selection.clone();
    }
    assert!(
        post(info, "model-jobs", jobs.clone())
            .await
            .status()
            .is_success()
    );
    assert_eq!(post(info, "model-jobs", jobs).await.status(), 409);
    let role = workspace["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap()
        .clone();
    let preview: Value = post(info, "model-preview", json!({"role":role}))
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["model"]["resolved"]["adapter"], "omp");
    assert_eq!(preview["model"]["selection"]["intendedModel"], selection);
    assert!(!preview.to_string().contains("credential-canary"));
    let mut changed = get(info, "workspace").await["modelJobs"].clone();
    changed["budget"] = json!("small");
    post(info, "model-jobs", changed)
        .await
        .error_for_status()
        .unwrap();
    let newer: Value = post(info, "model-preview", json!({"role":role}))
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(
        preview["model"]["selection"]["previewRevision"],
        newer["model"]["selection"]["previewRevision"]
    );
    service.close().await.unwrap();
    // The catalog itself survives independently of a UI connection or workflow.
    let stored: Value = serde_json::from_slice(
        &std::fs::read(root.path().join("runtime/model-catalog/catalog.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["revision"], catalog["revision"]);
    assert_eq!(stored["harnesses"][0]["offerings"][0]["key"], selection);
    let reopened = Service::start_with_options(root.path(), &roles, 0, options(root.path()))
        .await
        .unwrap();
    let restored = get(&reopened.info, "workspace").await;
    assert_eq!(restored["modelDiscovery"]["revision"], catalog["revision"]);
    assert_eq!(restored["modelJobs"]["jobs"]["code"], selection);
    assert_eq!(restored["modelDiscovery"]["offerings"][0]["fresh"], true);
    assert!(restored["runs"].as_array().unwrap().is_empty());
    reopened.close().await.unwrap();
}

async fn refresh(info: &ConnectionInfo) -> Value {
    post(info, "model-catalog/refresh", json!({}))
        .await
        .error_for_status()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = get(info, "model-catalog").await;
            if view["state"] != "refreshing" {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn scenario(root: &Path, value: Value) {
    std::fs::write(
        root.join("scenario.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn lost_refresh_retry_concurrent_saves_drift_and_disappearance_never_admit_a_substitute() {
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
    let service = Service::start_with_options(root.path(), &roles, 0, options(root.path()))
        .await
        .unwrap();
    let info = &service.info;
    let initial = refresh(info).await;
    let workspace = get(info, "workspace").await;
    let mut jobs = workspace["modelJobs"].clone();
    let key = initial["offerings"][0]["offering"]["key"].clone();
    for name in ["code", "judgment", "review", "fast"] {
        jobs["jobs"][name] = key.clone();
    }
    let (left, right, refresh_response) = tokio::join!(
        post(info, "model-jobs", jobs.clone()),
        post(info, "model-jobs", jobs),
        post(info, "model-catalog/refresh", json!({}))
    );
    let mut statuses = [left.status().as_u16(), right.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    // Ignore the first response body, then retry the same operation.
    drop(refresh_response);
    let retried: Value = post(info, "model-catalog/refresh", json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        retried["generation"],
        get(info, "model-catalog").await["generation"]
    );
    let ready = refresh(info).await;
    let role = workspace["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "implementer")
        .unwrap();
    let preview: Value = post(info, "model-preview", json!({"role":role}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(preview["model"]["selection"]["intendedModel"], key);
    assert_eq!(
        preview["model"]["selection"]["catalogRevision"],
        ready["revision"]
    );

    // A valid executable changing after discovery invalidates the old receipt.
    let binary = root.path().join("omp.py");
    let original = std::fs::read(&binary).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&binary)
        .unwrap()
        .write_all(b"\n# changed after discovery\n")
        .unwrap();
    let stale = get(info, "model-catalog").await;
    assert_eq!(stale["offerings"][0]["fresh"], false);
    let blocked: Value = post(info, "model-preview", json!({"role":role}))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        blocked["error"]
            .as_str()
            .unwrap()
            .contains("Refresh this harness")
    );
    assert!(blocked["model"].is_null());
    std::fs::write(&binary, original).unwrap();
    refresh(info).await;

    scenario(root.path(), json!({"fail":true}));
    let failed = refresh(info).await;
    assert_eq!(failed["state"], "error");
    assert_eq!(failed["offerings"][0]["offering"]["key"], key);
    assert_eq!(failed["offerings"][0]["fresh"], false);
    assert!(!failed.to_string().contains("credential-canary"));
    assert!(!failed.to_string().contains("account-canary"));

    scenario(root.path(), json!({"empty":true}));
    let removed = refresh(info).await;
    assert!(removed["offerings"].as_array().unwrap().is_empty());
    let retained = get(info, "workspace").await;
    assert_eq!(retained["modelJobs"]["jobs"]["code"], key);
    assert!(retained["roleCatalog"]["implementer"]["modelError"].is_string());
    let task: Value = post(
        info,
        "tasks",
        json!({"title":"Missing offering","description":"fixture"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let attempted = post(
        info,
        &format!("tasks/{}/refinement-runs", task["id"].as_str().unwrap()),
        json!({"purpose":"explore","revision":1}),
    )
    .await;
    assert_eq!(attempted.status(), 409);
    let after = get(info, "workspace").await;
    for run in after["runs"].as_array().unwrap() {
        let runtime = get(
            info,
            &format!("runs/{}/runtime", run["id"].as_str().unwrap()),
        )
        .await;
        assert!(runtime["assignments"].as_array().unwrap().is_empty());
    }
    scenario(root.path(), json!({}));
    let recovered = refresh(info).await;
    assert_eq!(recovered["offerings"][0]["fresh"], true);
    let mut budget = get(info, "workspace").await["modelJobs"].clone();
    budget["budget"] = json!("medium");
    post(info, "model-jobs", budget)
        .await
        .error_for_status()
        .unwrap();
    let unsupported: Value = post(info, "model-preview", json!({"role":role}))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        unsupported["error"]
            .as_str()
            .unwrap()
            .contains("Effort medium")
    );
    service.close().await.unwrap();
    let mut changed = options(root.path());
    changed.runtime.as_mut().unwrap()["catalog"][0]["tools"] = json!(false);
    let reopened = Service::start_with_options(root.path(), &roles, 0, changed)
        .await
        .unwrap();
    let drift = get(&reopened.info, "model-catalog").await;
    assert_eq!(drift["offerings"][0]["fresh"], false);
    assert_eq!(drift["harnesses"][0]["fresh"], false);
    refresh(&reopened.info).await;
    let mut role = role.clone();
    role["effort"] = json!("high");
    let missing_tools: Value = post(&reopened.info, "model-preview", json!({"role":role}))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        missing_tools["error"]
            .as_str()
            .unwrap()
            .contains("Required tool capability")
    );
    assert_eq!(
        get(&reopened.info, "workspace").await["modelJobs"]["jobs"]["code"],
        key
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn legacy_history_and_stop_remain_independent_of_refresh_and_shutdown_cancels_discovery() {
    use crewroom_core::{roles::Roles, store::Store};
    let root = tempfile::tempdir().unwrap();
    let roles_path = root.path().join("roles.json");
    let roles = Roles::new(&roles_path).read().unwrap();
    let coordinator = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let models = [
        json!({"requested":{"model":"codex/legacy","effort":"default"},"resolved":{"provider":"codex","id":"legacy","selector":"codex/legacy","api":"openai-responses","effort":"high"},"fallbackFrom":{"selector":"old/original"}}),
        json!({"resolved":{"adapter":"codex","provider":"codex","id":"native-old","selector":"codex/native-old","api":"codex-app-server","effort":"low"}}),
        json!({"resolved":{"adapter":"claude-code","provider":"claude-code","id":"sonnet","selector":"claude-code/sonnet","api":"claude-code-stream","resolvedModel":"claude-old-observed","effort":"low"}}),
    ];
    let mut saved = Vec::new();
    for (index, model) in models.iter().enumerate() {
        let task = store.create_task(&json!({"title":format!("Pre-MH4 history {index}"),"description":"Seeded migration evidence"})).unwrap();
        let run = store
            .queue_refinement(
                task["id"].as_str().unwrap(),
                &json!({"revision":1,"purpose":"explore"}),
            )
            .unwrap();
        let run_id = run["id"].as_str().unwrap();
        let admitted = store.admit_assignment(run_id,&json!({"coordinator":true,"name":"Historical coordinator","role":coordinator,"model":model,"input":"Seeded, not executed"})).unwrap();
        let attempt_id = admitted["attempt"]["id"].as_str().unwrap();
        store.claim_attempt(attempt_id).unwrap();
        let mut identity = json!({"hostPid":2_000_000_001_u32,"hostUrl":"http://127.0.0.1:49999","pane":"%999","sessionId":format!("retained-{index}")});
        if index == 0 {
            identity["ompPid"] = json!(2_000_000_002_u32);
        } else {
            identity["runnerPid"] = json!(2_000_000_002_u32);
            identity["runnerKind"] = model["resolved"]["adapter"].clone();
            identity["observedModel"] = if index == 1 {
                json!("native-old")
            } else {
                json!("claude-old-observed")
            };
        }
        store
            .record_attempt_identity(attempt_id, &identity)
            .unwrap();
        let message = store
            .prepare_message(
                attempt_id,
                "uncertain-mail",
                &json!({"to":admitted["assignment"]["id"],"body":"Seeded unresolved Mail effect"}),
            )
            .unwrap();
        store
            .mark_message_uncertain(message["id"].as_str().unwrap())
            .unwrap();
        let steering = store
            .steer(
                run_id,
                &json!({"requestId":"uncertain-steering","body":"Seeded unconfirmed input"}),
            )
            .unwrap();
        store
            .dispatch_steering(
                attempt_id,
                &[steering["id"].as_str().unwrap().to_owned()],
                "seeded-input",
            )
            .unwrap();
        store
            .record_ownership(
                attempt_id,
                true,
                Some("Seeded stopped history; no physical processes"),
            )
            .unwrap();
        store.set_desired_state(run_id, "paused").unwrap();
        store.begin_pause(run_id, None).unwrap();
        store.finish_pause(run_id).unwrap();
        saved.push((
            run_id.to_owned(),
            admitted["assignment"].clone(),
            identity["sessionId"].clone(),
        ));
    }
    drop(store);
    let service = Service::start_with_options(root.path(), &roles_path, 0, options(root.path()))
        .await
        .unwrap();
    let info = &service.info;
    refresh(info).await;
    let before = get(info, &format!("runs/{}/runtime", saved[0].0)).await;
    std::fs::remove_file(root.path().join("discovery-pid")).unwrap();
    scenario(root.path(), json!({"delay":30}));
    post(info, "model-catalog/refresh", json!({}))
        .await
        .error_for_status()
        .unwrap();
    let paused = tokio::time::timeout(
        Duration::from_secs(2),
        post(info, &format!("runs/{}/pause", saved[0].0), json!({})),
    )
    .await
    .unwrap();
    assert!(paused.status().is_success());
    let during = get(info, &format!("runs/{}/runtime", saved[0].0)).await;
    assert_eq!(during["assignments"], before["assignments"]);
    // There is deliberately no process receipt for seeded history. Recovery
    // must retain its uncertainty, including when a catalog worker is busy.
    assert_eq!(during["attempts"][0]["ownership"], "uncertain");
    assert_eq!(during["attempts"][0]["sessionId"], saved[0].2);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.path().join("discovery-pid").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(root.path().join("discovery-pid"))
        .unwrap()
        .parse()
        .unwrap();
    if let Err(error) = tokio::time::timeout(Duration::from_secs(5), service.close())
        .await
        .unwrap()
    {
        assert!(
            error.to_string().to_lowercase().contains("ownership"),
            "{error}"
        );
    }
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "discovery child survived shutdown"
    );
    let reopened = Service::start_with_options(root.path(), &roles_path, 0, options(root.path()))
        .await
        .unwrap();
    let mut jobs = get(&reopened.info, "workspace").await["modelJobs"].clone();
    jobs["jobs"]["code"] = json!("offering:omp:fixture:model");
    post(&reopened.info, "model-jobs", jobs)
        .await
        .error_for_status()
        .unwrap();
    scenario(root.path(), json!({"fail":true}));
    refresh(&reopened.info).await;
    for (run, assignment, session) in &saved {
        let view = get(&reopened.info, &format!("runs/{run}/runtime")).await;
        assert_eq!(view["assignments"], json!([assignment]));
        assert_eq!(view["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(view["attempts"][0]["model"], assignment["model"]);
        assert_eq!(view["attempts"][0]["sessionId"], *session);
        assert_eq!(view["attempts"][0]["ownership"], "uncertain");
        assert_eq!(view["messages"].as_array().unwrap().len(), 1);
        assert_eq!(view["messages"][0]["disposition"], "uncertain");
        let messages = get(&reopened.info, "workspace").await["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["runId"] == *run)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["delivery"], "dispatched");
        let _resume = post(&reopened.info, &format!("runs/{run}/resume"), json!({})).await;
        let held = get(&reopened.info, &format!("runs/{run}/runtime")).await;
        assert_eq!(held["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(held["messages"], view["messages"]);
        let after = get(&reopened.info, "workspace").await["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["runId"] == *run)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(after, messages);
        assert_ne!(held["run"]["status"], "running");
    }
    if let Err(error) = reopened.close().await {
        assert!(
            error.to_string().to_lowercase().contains("ownership"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn unsupported_publish_remote_is_reported_as_an_unavailable_publication_provider() {
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
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
            std::process::Command::new("git")
                .current_dir(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let mut setup = options(root.path());
    setup.publication_unavailable = vec![
        crewroom_runtime::providers::remote::UnavailablePublication::unsupported(
            checkout.clone(),
            "github.com",
        ),
    ];
    let service = Service::start_with_options(root.path(), &roles, 0, setup)
        .await
        .unwrap();
    let info = &service.info;
    let created = post(
        info,
        "repositories",
        json!({"name":"Fixture","path":checkout,"targetBranch":"main"}),
    )
    .await;
    assert!(created.status().is_success(), "{created:?}");
    let workspace = get(info, "workspace").await;
    let repo = &workspace["repositories"][0];
    assert_eq!(repo["publicationAvailable"], false);
    assert_eq!(repo["publicationProvider"]["state"], "unavailable");
    assert_eq!(repo["publicationProvider"]["host"], "github.com");
    assert!(
        repo["publicationProvider"]["reason"]
            .as_str()
            .unwrap()
            .contains("no supported publication provider")
    );
    assert_eq!(workspace["capabilities"]["publication"], false);
    service.close().await.unwrap();
}

/// FOR-38: the operator's Linux service ran without runner configuration, so
/// the workspace had no catalog and the pickers silently became text fields.
#[tokio::test]
async fn unconfigured_runner_still_lists_installed_models_and_names_each_cause() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::DirBuilder::new().mode(0o700).create(&bin).unwrap();
    let codex = bin.join("codex");
    std::fs::write(&codex, r#"#!/usr/bin/python3
import json,sys
if '--version' in sys.argv:
 print('codex-cli 0.163.0');sys.exit(0)
assert sys.argv[1:4]==['app-server','--listen','stdio://']
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 result={'data':[{'model':'fixture-model','displayName':'Fixture model','inputModalities':['text'],'supportedReasoningEfforts':[{'reasoningEffort':'low'}],'apiKey':'credential-canary'}],'nextCursor':None} if v['method']=='model/list' else {}
 print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut setup = options(root.path());
    setup.runtime = None;
    setup.environment = json!({"PATH":bin,"HOME":root.path()});
    let roles = root.path().join("roles.json");
    let service = Service::start_with_options(root.path(), &roles, 0, setup)
        .await
        .unwrap();
    let info = &service.info;
    let workspace = get(info, "workspace").await;
    let catalog = &workspace["modelDiscovery"];
    assert_eq!(catalog["runner"]["configured"], false);
    assert!(
        catalog["runner"]["remedy"]
            .as_str()
            .unwrap()
            .contains("CREWROOM_RUNTIME_CONFIG")
    );
    assert_eq!(catalog["selection"], "intent");
    let harness = |view: &Value, name: &str| {
        view["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["harness"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(harness(catalog, "codex")["configured"], true);
    for (name, executable) in [("omp", "`omp`"), ("claude-code", "`claude`")] {
        let entry = harness(catalog, name);
        assert_eq!(entry["configured"], false);
        assert!(
            entry["remedy"].as_str().unwrap().contains(executable),
            "{entry}"
        );
    }
    let view = refresh(info).await;
    assert_eq!(view["selection"], "intent");
    let codex_view = harness(&view, "codex");
    assert_eq!(codex_view["outcome"], "success", "{codex_view}");
    assert!(
        codex_view["executionNote"]
            .as_str()
            .unwrap()
            .contains("codex-cli 0.163.0 lists models"),
        "{codex_view}"
    );
    assert_eq!(
        harness(&view, "claude-code")["error"],
        "Harness is not configured."
    );
    let key = "offering:codex:codex:fixture-model";
    let choices = view["choices"].as_array().unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0]["key"], key);
    assert_eq!(choices[0]["eligibility"]["low"]["eligible"], false);
    assert!(!view.to_string().contains("credential-canary"));
    // Saving records intent; legacy free text and catalog keys stay verbatim.
    let mut jobs = get(info, "workspace").await["modelJobs"].clone();
    jobs["jobs"] =
        json!({"code":key,"judgment":"legacy/free-text","review":key,"fast":"legacy/free-text"});
    jobs["budget"] = json!("small");
    let saved: Value = post(info, "model-jobs", jobs)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["jobs"]["code"], key);
    assert_eq!(saved["jobs"]["judgment"], "legacy/free-text");
    let workspace = get(info, "workspace").await;
    assert_eq!(workspace["modelJobs"]["jobs"], saved["jobs"]);
    assert_eq!(workspace["modelDiscovery"]["choices"], view["choices"]);
    service.close().await.unwrap();
    // The persisted catalog survives restart in the private data directory.
    let mut setup = options(root.path());
    setup.runtime = None;
    setup.environment = json!({"PATH":bin,"HOME":root.path()});
    let reopened = Service::start_with_options(root.path(), &roles, 0, setup)
        .await
        .unwrap();
    let restored = get(&reopened.info, "workspace").await;
    assert_eq!(restored["modelDiscovery"]["choices"][0]["key"], key);
    reopened.close().await.unwrap();
}
