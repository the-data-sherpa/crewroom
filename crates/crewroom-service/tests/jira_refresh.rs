//! A project's confirmed Jira site drives that project's Jira refresh
//! (FOR-53), against a simulated Jira API (loopback fixture, not a live Jira
//! site). Proves that each project's own filters reach Jira, that projects on
//! another site or without a connection are reported not connected, that
//! rejected and unreachable refreshes are shown per project with earlier
//! tickets kept, that confirming GitHub Issues stops a project's Jira refresh,
//! and that tokens never enter a projection. Live Jira refresh was not
//! executed (no Jira token on the verification machine).
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use crewroom_runtime::providers::{Credential, Credentials, jira::JiraClient};
use crewroom_service::{ConnectionInfo, Service, ServiceOptions};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "synthetic-jira-refresh-token";
const SITE: &str = "https://fixture.atlassian.net";
const OTHER: &str = "https://other-fixture.atlassian.net";

#[derive(Clone, Default)]
struct Fixture {
    /// `ok`, `rejected` (HTTP 401) or `error` (HTTP 503).
    mode: Arc<Mutex<&'static str>>,
    queries: Arc<Mutex<Vec<String>>>,
}
fn issue(id: &str, title: &str) -> Value {
    json!({"id":id,"key":format!("SIM-{id}"),"fields":{"summary":title,"description":"Source requirement","updated":"2026-10-18T10:00:00Z","status":{"id":"1","name":"To Do"}}})
}
async fn search(
    State(fixture): State<Fixture>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    assert_eq!(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    let mode = *fixture.mode.lock().unwrap();
    match mode {
        "rejected" => return (StatusCode::UNAUTHORIZED, "private fixture detail").into_response(),
        "error" => {
            return (StatusCode::SERVICE_UNAVAILABLE, "private fixture detail").into_response();
        }
        _ => {}
    }
    let jql = query.get("jql").cloned().unwrap_or_default();
    fixture.queries.lock().unwrap().push(jql.clone());
    let issues = if jql.contains(r#"project = "CR""#) {
        vec![issue("2", "Ranked first"), issue("1", "Ranked second")]
    } else if jql.contains("labels = beta") {
        vec![issue("3", "Beta work")]
    } else {
        vec![]
    };
    Json(json!({"isLast":true,"issues":issues})).into_response()
}
async fn simulated_jira() -> (String, Fixture) {
    let fixture = Fixture::default();
    *fixture.mode.lock().unwrap() = "ok";
    let app = Router::new()
        .route("/rest/api/3/search/jql", get(search))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, fixture)
}
/// A loopback origin with nothing listening, so a refresh cannot reach Jira.
async fn unreachable_origin() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    origin
}
fn credentials() -> Credentials {
    Arc::new(|| Box::pin(async { Ok(Credential::Bearer(TOKEN.into())) }))
}
/// A Jira connection for SITE with no workspace-level filter; every project
/// brings its own.
fn options(origin: Option<&str>) -> ServiceOptions {
    let source = json!({"siteUrl":SITE,"cloudId":"00000000-0000-4000-8000-000000000022","name":"Fixture site"});
    ServiceOptions {
        runtime: None,
        helper: "/usr/bin/false".into(),
        host: "/usr/bin/false".into(),
        tmux: "/usr/bin/false".into(),
        jira: origin.map(|o| JiraClient::new(&source, credentials(), Some(o)).unwrap()),
        publication: vec![],
        publication_unavailable: vec![],
        environment: json!({}),
        runner: Default::default(),
        github: None,
    }
}
async fn start(root: &Path, options: ServiceOptions) -> Service {
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    Service::start_with_options(&data, &root.join("roles.json"), 0, options)
        .await
        .unwrap()
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
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    assert!(!text.contains(TOKEN), "response carried the token");
    assert!(!text.contains("private fixture detail"));
    (status, serde_json::from_str(&text).unwrap())
}
async fn workspace(info: &ConnectionInfo) -> Value {
    let text = reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!text.contains(TOKEN), "workspace carried the token");
    serde_json::from_str(&text).unwrap()
}
/// A project on a Git checkout with a synthetic remote (never contacted).
async fn project(info: &ConnectionInfo, root: &Path, name: &str) -> String {
    let path = std::fs::canonicalize(root).unwrap().join(name);
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
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let (status, saved) = post(
        info,
        "/repositories",
        json!({"name":name,"path":path,"targetBranch":"main","checks":[]}),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    saved["id"].as_str().unwrap().to_owned()
}
async fn confirm(info: &ConnectionInfo, id: &str, body: Value) {
    let (status, saved) = post(info, &format!("/repositories/{id}/ticket-source"), body).await;
    assert_eq!(status, 200, "{saved}");
}
async fn jira_project(info: &ConnectionInfo, root: &Path, name: &str, site: &str) -> String {
    let id = project(info, root, name).await;
    confirm(info, &id, json!({"tickets":"jira","site":site})).await;
    id
}
fn refresh_of(value: &Value, id: &str) -> Value {
    value["jiraRefresh"]["projects"]
        .as_array()
        .and_then(|p| p.iter().find(|p| p["repositoryId"] == id))
        .cloned()
        .unwrap_or(Value::Null)
}
fn task(view: &Value, key: &str) -> Value {
    view["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == key)
        .cloned()
        .unwrap_or_else(|| panic!("{key} missing"))
}

#[tokio::test]
async fn each_project_refreshes_its_confirmed_jira_site_with_its_own_filters() {
    let root = tempfile::tempdir().unwrap();
    let (origin, fixture) = simulated_jira().await;
    let service = start(root.path(), options(Some(&origin))).await;
    let info = service.info.clone();
    let alpha = jira_project(&info, root.path(), "alpha", SITE).await;
    let beta = jira_project(&info, root.path(), "beta", SITE).await;
    let gamma = jira_project(&info, root.path(), "gamma", OTHER).await;

    // A confirmed site has no filters until the operator saves them.
    let view = workspace(&info).await;
    assert_eq!(refresh_of(&view, &alpha)["availability"], "needs_filters");
    let (status, refused) = post(
        &info,
        &format!("/repositories/{alpha}/jira/sync"),
        json!({}),
    )
    .await;
    assert_eq!(status, 409, "{refused}");
    assert!(fixture.queries.lock().unwrap().is_empty());
    let (status, invalid) = post(
        &info,
        &format!("/repositories/{alpha}/jira/filters"),
        json!({"jql":"project = CR ORDER BY created"}),
    )
    .await;
    assert_eq!(status, 400, "{invalid}");
    for (id, filters) in [
        (&alpha, json!({"projectKey":"cr","assignee":"operator-1"})),
        (&beta, json!({"jql":"labels = beta"})),
        (&gamma, json!({"projectKey":"GA"})),
    ] {
        let (status, saved) =
            post(&info, &format!("/repositories/{id}/jira/filters"), filters).await;
        assert_eq!(status, 200, "{saved}");
    }

    // Refreshing every project queries the connected site once per project,
    // each with its own filters; the project on another site is not queried.
    let (status, refreshed) = post(&info, "/jira/sync", json!({})).await;
    assert_eq!(status, 200, "{refreshed}");
    let mut expected = vec![alpha.clone(), beta.clone()];
    expected.sort();
    let mut actual: Vec<String> = serde_json::from_value(refreshed["refreshed"].clone()).unwrap();
    actual.sort();
    assert_eq!(actual, expected);
    let mut queries = fixture.queries.lock().unwrap().clone();
    queries.sort();
    assert_eq!(
        queries,
        [
            "(labels = beta) ORDER BY Rank ASC",
            r#"project = "CR" AND assignee = "operator-1" ORDER BY Rank ASC"#,
        ]
    );
    let view = workspace(&info).await;
    let a = refresh_of(&view, &alpha);
    assert_eq!(a["availability"], "connected");
    assert_eq!(a["issueCount"], 2);
    assert_eq!(refresh_of(&view, &beta)["issueCount"], 1);
    // Tickets first imported by a project's refresh target that project; the
    // Jira status is the source status only and nothing is queued.
    assert_eq!(task(&view, "SIM-1")["repositoryId"], alpha.as_str());
    assert_eq!(task(&view, "SIM-3")["repositoryId"], beta.as_str());
    assert_eq!(task(&view, "SIM-1")["sourceStatus"], "To Do");
    assert!(view["runs"].as_array().unwrap().is_empty());
    assert_eq!(view["connections"]["jira"], true);
    assert_eq!(
        view["jira"]["source"],
        json!({"siteUrl":SITE,"name":"Fixture site"})
    );

    // The project on another site is shown not connected and never queried.
    let g = refresh_of(&view, &gamma);
    assert_eq!(g["availability"], "not_connected");
    assert!(g["connectionNote"].as_str().unwrap().contains(SITE), "{g}");
    let before = fixture.queries.lock().unwrap().len();
    let (status, failed) = post(
        &info,
        &format!("/repositories/{gamma}/jira/sync"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "{failed}");
    let g = refresh_of(&failed, &gamma);
    assert_eq!(g["reason"], "not_connected");
    assert_eq!(fixture.queries.lock().unwrap().len(), before);

    // Rejected and failing refreshes are shown per project; tickets stay.
    *fixture.mode.lock().unwrap() = "rejected";
    let (_, rejected) = post(
        &info,
        &format!("/repositories/{alpha}/jira/sync"),
        json!({}),
    )
    .await;
    let r = refresh_of(&rejected, &alpha);
    assert_eq!(r["availability"], "rejected", "{r}");
    assert!(r["error"].as_str().unwrap().contains("HTTP 401"));
    assert_eq!(r["lastSyncedAt"], a["lastSyncedAt"]);
    assert_eq!(refresh_of(&rejected, &beta)["availability"], "connected");
    *fixture.mode.lock().unwrap() = "error";
    let (_, failing) = post(
        &info,
        &format!("/repositories/{alpha}/jira/sync"),
        json!({}),
    )
    .await;
    assert_eq!(refresh_of(&failing, &alpha)["reason"], "unavailable");
    let view = workspace(&info).await;
    assert_eq!(task(&view, "SIM-2")["title"], "Ranked first");

    // Confirming GitHub Issues stops beta's Jira refresh; its tickets keep their source.
    *fixture.mode.lock().unwrap() = "ok";
    confirm(
        &info,
        &beta,
        json!({"tickets":"github","repository":"fixture/issues"}),
    )
    .await;
    fixture.queries.lock().unwrap().clear();
    let (_, refreshed) = post(&info, "/jira/sync", json!({})).await;
    assert_eq!(refreshed["refreshed"], json!([alpha]));
    assert!(refresh_of(&refreshed, &beta).is_null());
    assert!(
        fixture
            .queries
            .lock()
            .unwrap()
            .iter()
            .all(|q| !q.contains("beta"))
    );
    let view = workspace(&info).await;
    let kept = task(&view, "SIM-3");
    assert_eq!(kept["source"], "jira");
    assert_eq!(kept["sourceSite"], SITE);
    service.close().await.unwrap();
}

#[tokio::test]
async fn unreachable_and_unconnected_jira_are_reported_per_project() {
    // Unreachable: the connection's origin has nothing listening.
    let root = tempfile::tempdir().unwrap();
    let origin = unreachable_origin().await;
    let service = start(root.path(), options(Some(&origin))).await;
    let info = service.info.clone();
    let id = jira_project(&info, root.path(), "alpha", SITE).await;
    post(
        &info,
        &format!("/repositories/{id}/jira/filters"),
        json!({"projectKey":"CR"}),
    )
    .await;
    let (status, failed) = post(&info, &format!("/repositories/{id}/jira/sync"), json!({})).await;
    assert_eq!(status, 200, "{failed}");
    let status = refresh_of(&failed, &id);
    assert_eq!(status["availability"], "unreachable", "{status}");
    assert!(
        status["error"]
            .as_str()
            .unwrap()
            .contains("could not be reached")
    );
    assert!(
        workspace(&info).await["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    service.close().await.unwrap();

    // Not connected: no Jira connection on this machine.
    let root = tempfile::tempdir().unwrap();
    let service = start(root.path(), options(None)).await;
    let info = service.info.clone();
    let id = jira_project(&info, root.path(), "alpha", SITE).await;
    post(
        &info,
        &format!("/repositories/{id}/jira/filters"),
        json!({"projectKey":"CR"}),
    )
    .await;
    let view = workspace(&info).await;
    assert_eq!(refresh_of(&view, &id)["availability"], "not_connected");
    assert_eq!(view["jiraRefresh"]["connection"]["state"], "not_configured");
    assert_eq!(view["capabilities"]["liveJira"], false);
    let (status, _) = post(&info, "/jira/sync", json!({})).await;
    assert_eq!(status, 409);
    let (_, failed) = post(&info, &format!("/repositories/{id}/jira/sync"), json!({})).await;
    let status = refresh_of(&failed, &id);
    assert_eq!(status["reason"], "not_connected");
    assert!(status["error"].as_str().unwrap().contains("not connected"));
    service.close().await.unwrap();
}
