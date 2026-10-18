use crewroom_service::{ConnectionInfo, Service};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{os::unix::fs::MetadataExt, time::Duration};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

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

#[tokio::test]
async fn dropped_or_cancelled_service_owners_drain_and_release_authority() {
    use crewroom_core::authority::Authority;
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
    let service = Service::start(root.path(), &roles, 0).await.unwrap();
    let descriptor = root.path().join("native-connection.json");
    assert!(descriptor.exists());
    drop(service);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !descriptor.exists() && Authority::acquire(root.path()).is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for _ in 0..8 {
        let (data, roles) = (root.path().to_owned(), roles.clone());
        let starting = tokio::spawn(async move { Service::start(&data, &roles, 0).await });
        tokio::task::yield_now().await;
        starting.abort();
        if let Ok(Ok(service)) = starting.await {
            drop(service);
        }
        // Drive the registered startup before checking that its abandoned owner closes.
        tokio::time::sleep(Duration::from_millis(30)).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !descriptor.exists() && Authority::acquire(root.path()).is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    Service::start(root.path(), &roles, 0)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}
#[tokio::test]
async fn authenticated_loopback_boundary_and_push_then_reopen() {
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
    let server = Service::start(root.path(), &roles, 0).await.unwrap();
    let client = reqwest::Client::new();
    let url = format!("{}/api/workspace", server.info.origin);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 403);
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&server.info.token)
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&server.info.token)
            .header("Host", "evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&server.info.token)
            .header("sec-fetch-site", "cross-site")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(format!("{}/api/tasks", server.info.origin))
            .bearer_auth(&server.info.token)
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        std::fs::metadata(root.path().join("native-connection.json"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
    assert!(Service::start(root.path(), &roles, 0).await.is_err());
    let mut request = format!(
        "{}/api/live",
        server.info.origin.replacen("http://", "ws://", 1)
    )
    .into_client_request()
    .unwrap();
    assert!(
        tokio_tungstenite::connect_async(request.clone())
            .await
            .is_err()
    );
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", server.info.token).parse().unwrap(),
    );
    let (mut live, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    live.send(Message::Text(
        json!({"op":"subscribe","topic":"workspace"})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let initial = tokio::time::timeout(Duration::from_secs(3), live.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let initial: Value = serde_json::from_str(initial.to_text().unwrap()).unwrap();
    assert_eq!(initial["data"]["tasks"], json!([]));
    let task: Value = post(
        &server.info,
        "/tasks",
        json!({"title":"Native ticket","description":"Retained request"}),
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(task["id"].is_string());
    let pushed = tokio::time::timeout(Duration::from_secs(3), live.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(pushed.to_text().unwrap()).unwrap()["data"]["tasks"][0]["title"],
        "Native ticket"
    );
    let id = task["id"].as_str().unwrap();
    assert!(post(&server.info,&format!("/tasks/{id}/refinement"),json!({"revision":1,"sourceRevision":1,"brief":"Native persistence","criteria":["Survives reopen"]})).await.status().is_success());
    let old_token = server.info.token.clone();
    // An open observer must not pin the authority during shutdown.
    tokio::time::timeout(Duration::from_secs(6), server.close())
        .await
        .unwrap()
        .unwrap();
    drop(live);
    let reopened = Service::start(root.path(), &roles, 0).await.unwrap();
    assert_ne!(reopened.info.token, old_token);
    let saved: Value = client
        .get(format!("{}/api/workspace", reopened.info.origin))
        .bearer_auth(&reopened.info.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["tasks"][0]["brief"], "Native persistence");
    assert_eq!(saved["requirementRevisions"][0]["revision"], 2);
    assert_eq!(saved["connections"]["runner"], false);
    assert_eq!(
        post(&reopened.info, "/runs/not-running/start", json!({}))
            .await
            .status(),
        409
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn captured_launch_survives_service_restart_without_granting_execution() {
    let root = tempfile::tempdir().unwrap();
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
    let roles = root.path().join("roles.json");
    let server = Service::start(root.path(), &roles, 0).await.unwrap();
    let repo: Value = post(
        &server.info,
        "/repositories",
        json!({"name":"Fixture","path":checkout,"targetBranch":"main"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let task: Value = post(
        &server.info,
        "/tasks",
        json!({"title":"Planned launch","description":"","repositoryId":repo["id"]}),
    )
    .await
    .json()
    .await
    .unwrap();
    let task_id = task["id"].as_str().unwrap();
    assert!(post(&server.info,&format!("/tasks/{task_id}/refinement"),json!({"revision":1,"sourceRevision":1,"brief":"Retain the plan","criteria":["No process starts"]})).await.status().is_success());
    let source: Value = post(
        &server.info,
        &format!("/tasks/{task_id}/plan-source"),
        json!({"revision":2}),
    )
    .await
    .json()
    .await
    .unwrap();
    let plan: Value = post(&server.info, &format!("/tasks/{task_id}/implementation-plans"), json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Retain launch","scope":["Capture intent"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"capture","name":"Capture","dependsOn":[],"outputs":["Saved launch"],"criteria":[0],"validation":["Restart fixture"]}]}})).await.json().await.unwrap();
    assert!(post(&server.info, &format!("/implementation-plans/{}/decision", plan["id"].as_str().unwrap()), json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"decision":"accept"})).await.status().is_success());
    let run:Value=post(&server.info,&format!("/tasks/{task_id}/runs"),json!({"revision":2,"launch":{"team":[{"name":"Coordinator","roleId":"coordinator","coordinator":true,"input":"Observe"}]}})).await.json().await.unwrap();
    assert_eq!(run["status"], "awaiting_runner");
    assert_eq!(run["launch"]["repository"]["mode"], "new");
    let run_id = run["id"].as_str().unwrap();
    let edit = json!({"requestId":uuid::Uuid::new_v4().to_string(),"launch":{"team":[{"name":"Coordinator","roleId":"coordinator","coordinator":true,"input":"Updated saved plan"}]}});
    let saved: Value = post(
        &server.info,
        &format!("/queue/{run_id}/launch"),
        edit.clone(),
    )
    .await
    .json()
    .await
    .unwrap();
    for action in ["start", "resume"] {
        assert_eq!(
            post(&server.info, &format!("/runs/{run_id}/{action}"), json!({}))
                .await
                .status(),
            409
        );
    }
    server.close().await.unwrap();
    let reopened = Service::start(root.path(), &roles, 0).await.unwrap();
    let replay: Value = post(&reopened.info, &format!("/queue/{run_id}/launch"), edit)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(replay, saved);
    let runtime: Value = reqwest::Client::new()
        .get(format!(
            "{}/api/runs/{run_id}/runtime",
            reopened.info.origin
        ))
        .bearer_auth(&reopened.info.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(runtime["assignments"], json!([]));
    assert_eq!(runtime["attempts"], json!([]));
    assert_eq!(runtime["run"]["id"], run_id);
    assert_eq!(runtime["teamProposals"], json!([]));
    assert_eq!(runtime["commands"], json!([]));
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn learning_decisions_and_startup_reconciliation_keep_one_catalog_revision() {
    use crewroom_core::{roles::Roles, store::Store};
    let root = tempfile::tempdir().unwrap();
    let role_path = root.path().join("roles.json");
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let task = store
        .create_task(&json!({"title":"Learning API","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    // Trusted proposal fixture: coordinator authentication and execution are N3.
    let input = json!({"runId":run["id"],"attemptId":"fixture-attempt","toolCallId":"first","target":{"kind":"role","roleId":"reviewer"},"baseVersion":1,"change":"Retain API approval","rationale":"Recorded correction","evidence":[{"kind":"steering","id":"fixture-evidence"}]});
    let proposal = store.propose_learning(&input).unwrap();
    drop(store);
    let server = Service::start(root.path(), &role_path, 0).await.unwrap();
    let route = format!("/learning/{}/decision", proposal["id"].as_str().unwrap());
    let decision = json!({"requestId":uuid::Uuid::new_v4().to_string(),"decision":"approve","reason":"Evidence reviewed"});
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{}/api{route}", server.info.origin))
            .bearer_auth(&server.info.token)
            .header("Origin", "https://foreign.example")
            .json(&decision)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(!role_path.exists());
    let response = post(&server.info, &route, decision.clone()).await;
    assert!(response.status().is_success());
    let approved: Value = response.json().await.unwrap();
    assert_eq!(approved["decision"]["appliedVersion"], 2);
    assert_eq!(
        post(&server.info, "/learning", input.clone())
            .await
            .status(),
        400
    );
    server.close().await.unwrap();
    let reopened = Service::start(root.path(), &role_path, 0).await.unwrap();
    let replay: Value = post(&reopened.info, &route, decision)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(replay, approved);
    reopened.close().await.unwrap();
    let store = Store::open(&path).unwrap();
    let mut next = input;
    next["toolCallId"] = json!("second");
    next["baseVersion"] = json!(2);
    next["change"] = json!("Reconciled on startup");
    let pending = store.propose_learning(&next).unwrap();
    store.decide_learning(pending["id"].as_str().unwrap(),&json!({"status":"approved","requestId":uuid::Uuid::new_v4().to_string(),"reason":"Approved before interruption","pendingRoleSave":true})).unwrap();
    drop(store);
    for _ in 0..2 {
        let reopened = Service::start(root.path(), &role_path, 0).await.unwrap();
        let reviewer = Roles::new(&role_path)
            .read()
            .unwrap()
            .into_iter()
            .find(|r| r["id"] == "reviewer")
            .unwrap();
        assert_eq!(reviewer["revision"], 3);
        assert_eq!(reviewer["instructions"], "Reconciled on startup");
        let state: Value = client
            .get(format!("{}/api/workspace", reopened.info.origin))
            .bearer_auth(&reopened.info.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let saved = state["learning"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == pending["id"])
            .unwrap();
        assert_eq!(saved["decision"]["appliedVersion"], 3);
        assert!(saved["decision"].get("pendingRoleSave").is_none());
        assert_eq!(
            post(&reopened.info, "/runs/fixture/start", json!({}))
                .await
                .status(),
            409
        );
        reopened.close().await.unwrap();
    }
}

#[tokio::test]
async fn refinement_decision_persists_linked_history_without_rewriting_the_running_brief() {
    use crewroom_core::store::Store;
    let root = tempfile::tempdir().unwrap();
    let role_path = root.path().join("roles.json");
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let task = store
        .create_task(&json!({"title":"Refine through API","description":"Original idea"}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"grill"}),
        )
        .unwrap();
    let role = crewroom_core::roles::Roles::new(&role_path)
        .read()
        .unwrap()
        .remove(0);
    let admitted=store.admit_assignment(run["id"].as_str().unwrap(),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Inspect scope"})).unwrap();
    let attempt = admitted["attempt"]["id"].as_str().unwrap();
    let run_id = run["id"].as_str().unwrap();
    store.claim_attempt(attempt).unwrap();
    // Explicit stored identity fixture; no actual host is started or inspected.
    store.record_attempt_identity(attempt,&json!({"hostPid":123,"ompPid":124,"sessionId":"fixture","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
    store.set_desired_state(run_id, "running").unwrap();
    store.start_execution(run_id).unwrap();
    let proposal=store.propose_refinement(attempt,"proposal",&json!({"brief":"Proposed direction","criteria":["Keep history"],"decisions":["Retain original source"],"unresolvedQuestions":["Any constraint?"]})).unwrap();
    drop(store);
    let server = Service::start(root.path(), &role_path, 0).await.unwrap();
    let route = format!(
        "/refinement-proposals/{}/decision",
        proposal["id"].as_str().unwrap()
    );
    let input = json!({"decision":"accept","requestId":uuid::Uuid::new_v4().to_string(),"brief":"Operator amended direction"});
    let response = post(&server.info, &route, input.clone()).await;
    assert!(response.status().is_success());
    let accepted: Value = response.json().await.unwrap();
    assert_eq!(accepted["decision"]["revision"], 2);
    server.close().await.unwrap();
    let reopened = Service::start(root.path(), &role_path, 0).await.unwrap();
    let replay: Value = post(&reopened.info, &route, input.clone())
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(replay, accepted);
    let mut changed = input;
    changed["brief"] = json!("Changed replay");
    assert_eq!(post(&reopened.info, &route, changed).await.status(), 409);
    let state: Value = reqwest::Client::new()
        .get(format!("{}/api/workspace", reopened.info.origin))
        .bearer_auth(&reopened.info.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["tasks"][0]["brief"], "Operator amended direction");
    assert_eq!(state["runs"][0]["taskRevision"], 1);
    assert_eq!(state["runs"][0]["taskSnapshot"]["brief"], "");
    assert_eq!(state["notes"].as_array().unwrap().len(), 2);
    assert!(
        state["notes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["revision"] == 2)
    );
    assert_eq!(
        state["requirementRevisions"][0]["proposalId"],
        proposal["id"]
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn role_preview_matches_admitted_snapshots_and_catalog_counts_recorded_runs() {
    use crewroom_core::{roles::Roles, store::Store};
    let root = tempfile::tempdir().unwrap();
    let role_path = root.path().join("roles.json");
    let service = Service::start(root.path(), &role_path, 0).await.unwrap();
    let task: Value = post(
        &service.info,
        "/tasks",
        json!({"title":"Preview acceptance","description":"Saved source"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let role = Roles::new(&role_path)
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap();
    let response = post(
        &service.info,
        "/role-preview",
        json!({"role":role,"taskId":task["id"]}),
    )
    .await;
    assert!(response.status().is_success());
    let preview: Value = response.json().await.unwrap();
    let run: Value = post(
        &service.info,
        &format!("/tasks/{}/refinement-runs", task["id"].as_str().unwrap()),
        json!({"revision":1,"purpose":"explore"}),
    )
    .await
    .json()
    .await
    .unwrap();
    service.close().await.unwrap();
    {
        let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
        let mut input = preview["assignment"].clone();
        input["model"] = json!({"resolved":{"selector":"fixture/model","effort":"low"}});
        let admitted = store
            .admit_assignment(run["id"].as_str().unwrap(), &input)
            .unwrap();
        assert_eq!(admitted["assignment"]["role"], preview["role"]);
        assert_eq!(
            admitted["assignment"]["brief"],
            preview["assignment"]["brief"]
        );
        assert_eq!(run["taskSnapshot"], preview["task"]);
    }
    let service = Service::start(root.path(), &role_path, 0).await.unwrap();
    let workspace: Value = reqwest::Client::new()
        .get(format!("{}/api/workspace", service.info.origin))
        .bearer_auth(&service.info.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(workspace["roleCatalog"]["coordinator"]["runs"], 1);
    assert!(
        workspace["roleCatalog"]["coordinator"]["lockReason"]
            .as_str()
            .unwrap()
            .contains("must stay enabled")
    );
    assert!(
        workspace["roleCatalog"]["coordinator"]["modelError"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
    assert_eq!(
        workspace["roles"].as_array().unwrap().len(),
        Roles::new(&role_path).read().unwrap().len()
    );
    service.close().await.unwrap();
}
