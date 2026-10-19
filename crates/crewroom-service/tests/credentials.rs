//! Stored-token route against a simulated Jira provider (loopback fixture, not
//! live Jira). Proves the settings save path, encryption at rest, environment
//! precedence reporting, unreadable-key handling and whole-state restore.
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use crewroom_runtime::providers::{config, jira::JiraClient, secrets};
use crewroom_service::{ConnectionInfo, Service, ServiceOptions};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "synthetic-stored-jira-token";

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
async fn workspace(info: &ConnectionInfo) -> String {
    reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

type Seen = Arc<Mutex<Vec<String>>>;
async fn search(State(seen): State<Seen>, headers: HeaderMap) -> Json<Value> {
    seen.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
    let issue = |id: &str, title: &str| json!({"id":id,"key":format!("SIM-{id}"),"fields":{"summary":title,"description":"Imported description","updated":"2026-10-03T10:00:00Z","status":{"id":"1","name":"To Do"}}});
    Json(
        json!({"isLast":true,"issues":[issue("2","First Jira task"),issue("1","Second Jira task")]}),
    )
}
async fn simulated_jira() -> (String, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .route("/rest/api/3/search/jql", get(search))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, seen)
}
/// A project on a Git checkout with a synthetic bitbucket.org remote (never
/// contacted) whose confirmed ticket source is the fixture's Jira site, so its
/// refresh adopts the workspace-level filter in jira.json (FOR-53).
async fn jira_project(info: &ConnectionInfo, root: &Path) -> String {
    let path = std::fs::canonicalize(root).unwrap().join("jira-project");
    std::fs::create_dir(&path).unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
        &[
            "remote",
            "add",
            "origin",
            "git@bitbucket.org:fixture/repo.git",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let (status, saved) = post(
        info,
        "/repositories",
        json!({"name":"Jira project","path":path,"targetBranch":"main","checks":[]}),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    let id = saved["id"].as_str().unwrap().to_owned();
    let (status, confirmed) = post(
        info,
        &format!("/repositories/{id}/ticket-source"),
        json!({"tickets":"jira","site":"https://fixture.atlassian.net"}),
    )
    .await;
    assert_eq!(status, 200, "{confirmed}");
    id
}
/// One project's Jira refresh status in a response or workspace.
fn refresh_of(value: &Value, id: &str) -> Value {
    value["jiraRefresh"]["projects"]
        .as_array()
        .and_then(|p| p.iter().find(|p| p["repositoryId"] == id))
        .cloned()
        .unwrap_or(Value::Null)
}
/// The production loader and store, with the provider origin pointed at the fixture.
fn options(data: &Path, origin: &str) -> ServiceOptions {
    let connection = config::load_jira_connection_stored(
        &data.join("jira.json"),
        Arc::new(|| None),
        config::stored_tokens(secrets::SecretStore::at(data), secrets::Integration::Jira),
    )
    .unwrap()
    .unwrap();
    ServiceOptions {
        runtime: None,
        helper: "/usr/bin/false".into(),
        host: "/usr/bin/false".into(),
        tmux: "/usr/bin/false".into(),
        jira: Some(
            JiraClient::new(&connection.source, connection.credentials, Some(origin)).unwrap(),
        ),
        publication: vec![],
        publication_unavailable: vec![],
        environment: json!({}),
        runner: Default::default(),
        github: None,
    }
}

#[tokio::test]
async fn jira_import_on_linux_uses_a_token_saved_through_settings() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let roles = root.path().join("roles.json");
    std::fs::write(
        data.join("jira.json"),
        json!({"siteUrl":"https://fixture.atlassian.net","cloudId":"00000000-0000-4000-8000-000000000022","filterId":"12345","assigneeAccountId":"operator-1","name":"Assigned work","accountId":"operator-1"}).to_string(),
    )
    .unwrap();
    let (origin, seen) = simulated_jira().await;

    let service = Service::start_with_options(&data, &roles, 0, options(&data, &origin))
        .await
        .unwrap();
    let info = service.info.clone();
    // First run created the private install key.
    let key = std::fs::read(data.join(secrets::SECRET_FILE)).unwrap();
    assert_eq!(key.len(), 32);
    assert_eq!(
        std::fs::metadata(data.join(secrets::SECRET_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let view: Value = serde_json::from_str(&workspace(&info).await).unwrap();
    assert_eq!(view["credentials"]["jira"]["source"], "none");
    assert_eq!(view["credentials"]["bitbucket"]["source"], "none");

    // Refresh is per project: the confirmed site adopts jira.json's filter.
    let project = jira_project(&info, root.path()).await;
    // Without a token the refresh fails before any request reaches the provider.
    let (_, failed) = post(&info, "/jira/sync", json!({})).await;
    assert!(seen.lock().unwrap().is_empty(), "{failed}");
    assert_eq!(
        refresh_of(&failed, &project)["availability"],
        "not_connected",
        "{failed}"
    );

    // Settings saves the token; the response and workspace carry only status.
    let (status, saved) = post(&info, "/credentials/jira", json!({"token": TOKEN})).await;
    assert_eq!(status, 200, "{saved}");
    assert!(!saved.to_string().contains(TOKEN));
    assert_eq!(saved["credentials"]["jira"]["source"], "stored");
    let raw = std::fs::read_to_string(data.join(secrets::CREDENTIALS_FILE)).unwrap();
    assert!(!raw.contains(TOKEN));
    assert!(raw.contains("$crewroom$v1$"));
    for (name, bad) in [
        ("unknown field", json!({"token": TOKEN, "extra": true})),
        ("empty token", json!({"token": ""})),
    ] {
        let (status, body) = post(&info, "/credentials/jira", bad).await;
        assert_eq!(status, 400, "{name}: {body}");
        assert!(!body.to_string().contains(TOKEN));
    }
    let (status, _) = post(&info, "/credentials/unknown", json!({"token": TOKEN})).await;
    assert_eq!(status, 404);

    // Refresh imports through the stored token without a restart.
    let (status, synced) = post(&info, "/jira/sync", json!({})).await;
    assert_eq!(status, 200, "{synced}");
    assert_eq!(refresh_of(&synced, &project)["state"], "connected");
    assert_eq!(
        seen.lock().unwrap().last().unwrap(),
        &format!("Bearer {TOKEN}")
    );
    let body = workspace(&info).await;
    assert!(!body.contains(TOKEN));
    let view: Value = serde_json::from_str(&body).unwrap();
    let keys: Vec<_> = view["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys, ["SIM-2", "SIM-1"]);
    service.close().await.unwrap();

    // Whole-state backup of the stopped data directory, then same-path restore.
    let backup = root.path().join("backup");
    copy_tree(&data, &backup);
    std::fs::rename(&data, root.path().join("preserved-before-restore")).unwrap();
    copy_tree(&backup, &data);
    let restored = Service::start_with_options(&data, &roles, 0, options(&data, &origin))
        .await
        .unwrap();
    assert_eq!(std::fs::read(data.join(secrets::SECRET_FILE)).unwrap(), key);
    let view: Value = serde_json::from_str(&workspace(&restored.info).await).unwrap();
    assert_eq!(view["credentials"]["jira"]["source"], "stored");
    let before = seen.lock().unwrap().len();
    let (status, synced) = post(&restored.info, "/jira/sync", json!({})).await;
    assert_eq!(status, 200, "{synced}");
    assert_eq!(seen.lock().unwrap().len(), before + 1);
    assert_eq!(
        seen.lock().unwrap().last().unwrap(),
        &format!("Bearer {TOKEN}")
    );
    restored.close().await.unwrap();

    // Restoring configuration without crewroom.secret: no crash, no new key,
    // the integration reads "Credentials unreadable — reconnect".
    std::fs::remove_file(data.join(secrets::SECRET_FILE)).unwrap();
    let service = Service::start_with_options(&data, &roles, 0, options(&data, &origin))
        .await
        .unwrap();
    assert!(!data.join(secrets::SECRET_FILE).exists());
    let view: Value = serde_json::from_str(&workspace(&service.info).await).unwrap();
    assert_eq!(view["credentials"]["jira"]["source"], "unreadable");
    assert_eq!(view["credentials"]["jira"]["label"], secrets::UNREADABLE);
    let before = seen.lock().unwrap().len();
    let (_, failed) = post(&service.info, "/jira/sync", json!({})).await;
    assert_eq!(seen.lock().unwrap().len(), before, "{failed}");
    let view: Value = serde_json::from_str(&workspace(&service.info).await).unwrap();
    let status = refresh_of(&view, &project);
    assert!(
        status["error"]
            .as_str()
            .unwrap_or("")
            .contains(secrets::UNREADABLE),
        "{status}"
    );
    assert_eq!(status["availability"], "not_connected");
    // An ordinary save will not orphan values; an explicit reconnect replaces them.
    let (status, refused) = post(&service.info, "/credentials/jira", json!({"token": TOKEN})).await;
    assert_eq!(status, 409, "{refused}");
    assert!(!data.join(secrets::SECRET_FILE).exists());
    let (status, saved) = post(
        &service.info,
        "/credentials/jira",
        json!({"token": TOKEN, "replaceUnreadable": true}),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["credentials"]["jira"]["source"], "stored");
    let (status, synced) = post(&service.info, "/jira/sync", json!({})).await;
    assert_eq!(status, 200, "{synced}");
    assert_eq!(seen.lock().unwrap().len(), before + 1);
    // Clearing removes only the stored value.
    let (status, cleared) = post(&service.info, "/credentials/jira/clear", json!({})).await;
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared["credentials"]["jira"]["source"], "none");
    service.close().await.unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir(to).unwrap();
    std::fs::set_permissions(to, std::fs::metadata(from).unwrap().permissions()).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).unwrap();
            std::fs::set_permissions(&target, entry.metadata().unwrap().permissions()).unwrap();
        }
    }
}
