//! GitHub Issues as a ticket source (FOR-45) against a simulated GitHub API
//! (loopback fixture, not github.com). Proves saved repository filters reach
//! GitHub, pagination, pull-request exclusion, identity matching on re-sync,
//! re-acceptance on title/body change with refinements kept, GitHub open/closed
//! kept as source status only, and truthful unavailable states. Live github.com
//! evidence is recorded separately in docs/testing.
use axum::{
    Json, Router,
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use crewroom_runtime::providers::github::GitHubClient;
use crewroom_service::{GitHubOptions, Service, ServiceOptions};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "ghp_synthetic0issues0token";

#[derive(Clone, Default)]
struct Fixture {
    origin: Arc<Mutex<String>>,
    revoked: Arc<Mutex<bool>>,
    rate_limited: Arc<Mutex<bool>>,
    /// A secondary rate limit: 403 with GitHub's message and no limit headers.
    secondary_limited: Arc<Mutex<bool>>,
    /// Issues closed since the fixture started (#3 is always closed).
    closed: Arc<Mutex<HashSet<u64>>>,
    /// Issues GitHub no longer serves, with the status their endpoint answers.
    gone: Arc<Mutex<HashMap<u64, StatusCode>>>,
    first_title: Arc<Mutex<String>>,
    queries: Arc<Mutex<Vec<HashMap<String, String>>>>,
}
fn authorized(fixture: &Fixture, headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {TOKEN}"))
        && !*fixture.revoked.lock().unwrap()
}
fn bad_credentials() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"message":"Bad credentials"})),
    )
        .into_response()
}
fn state_of(fixture: &Fixture, number: u64) -> &'static str {
    if number == 3 || fixture.closed.lock().unwrap().contains(&number) {
        "closed"
    } else {
        "open"
    }
}
fn issue(fixture: &Fixture, number: u64, state: &str) -> Value {
    let title = if number == 1 {
        fixture.first_title.lock().unwrap().clone()
    } else {
        format!("Synthetic issue {number}")
    };
    json!({
        "id": 9000 + number,
        "number": number,
        "title": title,
        "body": format!("Synthetic body {number}"),
        "state": state,
        "html_url": format!("https://github.com/fixture/issues/issues/{number}"),
        "updated_at": "2026-10-18T08:00:00Z",
        "labels": [{"name":"crewroom"}],
    })
}
async fn user(State(fixture): State<Fixture>, headers: HeaderMap) -> Response {
    if !authorized(&fixture, &headers) {
        return bad_credentials();
    }
    Json(json!({"login":"octo-fixture","id":1})).into_response()
}
async fn issues(
    State(fixture): State<Fixture>,
    UrlPath((owner, name)): UrlPath<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&fixture, &headers) {
        return bad_credentials();
    }
    if *fixture.rate_limited.lock().unwrap() {
        let mut response = (
            StatusCode::FORBIDDEN,
            Json(json!({"message":"API rate limit exceeded for user ID 1."})),
        )
            .into_response();
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", "0".parse().unwrap());
        response
            .headers_mut()
            .insert("x-ratelimit-reset", "1792310400".parse().unwrap());
        return response;
    }
    if *fixture.secondary_limited.lock().unwrap() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."})),
        )
            .into_response();
    }
    match (owner.as_str(), name.as_str()) {
        ("fixture", "issues") => {}
        ("fixture", "disabled") => {
            return (
                StatusCode::GONE,
                Json(json!({"message":"Issues are disabled for this repo"})),
            )
                .into_response();
        }
        _ => {
            return (StatusCode::NOT_FOUND, Json(json!({"message":"Not Found"}))).into_response();
        }
    }
    fixture.queries.lock().unwrap().push(query.clone());
    let origin = fixture.origin.lock().unwrap().clone();
    // GitHub's state filter: open (the default), closed or all. Issues it no
    // longer serves are not listed.
    let wanted = query.get("state").map(String::as_str).unwrap_or("open");
    let listed = |numbers: &[u64]| -> Vec<Value> {
        numbers
            .iter()
            .filter(|n| !fixture.gone.lock().unwrap().contains_key(n))
            .map(|&n| issue(&fixture, n, state_of(&fixture, n)))
            .filter(|i| wanted == "all" || i["state"] == wanted)
            .collect()
    };
    if query.get("page").map(String::as_str) == Some("2") {
        // Second page: one closed issue and nothing after it.
        return Json(json!(listed(&[3]))).into_response();
    }
    let pull = json!({"id":9100,"number":2,"title":"A pull request","body":"","state":"open","html_url":"https://github.com/fixture/issues/pull/2","updated_at":"2026-10-18T08:00:00Z","pull_request":{"url":"x"}});
    let mut page = listed(&[1, 4]);
    page.insert(page.len().min(1), pull);
    let mut response = Json(json!(page)).into_response();
    response.headers_mut().insert(
        "link",
        format!(
            "<{origin}/repos/fixture/issues/issues?state={wanted}&per_page=100&page=2>; rel=\"next\", <{origin}/repos/fixture/issues/issues?page=2>; rel=\"last\""
        )
        .parse()
        .unwrap(),
    );
    response
}
async fn single(
    State(fixture): State<Fixture>,
    UrlPath((_, _, number)): UrlPath<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&fixture, &headers) {
        return bad_credentials();
    }
    if let Some(status) = fixture.gone.lock().unwrap().get(&number).copied() {
        let mut response = (status, Json(json!({"message":"gone"}))).into_response();
        if status.is_redirection() {
            response.headers_mut().insert(
                "location",
                "https://api.github.com/repositories/2/issues/1"
                    .parse()
                    .unwrap(),
            );
        }
        return response;
    }
    Json(issue(&fixture, number, state_of(&fixture, number))).into_response()
}
async fn milestones(State(fixture): State<Fixture>, headers: HeaderMap) -> Response {
    if !authorized(&fixture, &headers) {
        return bad_credentials();
    }
    Json(json!([{"number":7,"title":"Sprint 1"},{"number":8,"title":"Sprint 2"}])).into_response()
}
async fn simulated_github() -> (String, Fixture) {
    let fixture = Fixture::default();
    *fixture.first_title.lock().unwrap() = "Synthetic issue 1".into();
    let app = Router::new()
        .route("/user", get(user))
        .route("/repos/{owner}/{name}/issues", get(issues))
        .route("/repos/{owner}/{name}/issues/{number}", get(single))
        .route("/repos/{owner}/{name}/milestones", get(milestones))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    *fixture.origin.lock().unwrap() = origin.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, fixture)
}
fn options(origin: &str) -> ServiceOptions {
    ServiceOptions {
        runtime: None,
        helper: "/usr/bin/false".into(),
        host: "/usr/bin/false".into(),
        tmux: "/usr/bin/false".into(),
        jira: None,
        publication: vec![],
        publication_unavailable: vec![],
        environment: json!({}),
        github: Some(GitHubOptions {
            client: GitHubClient::new(Some(origin)).unwrap(),
            environment: Arc::new(|| None),
        }),
    }
}
async fn post(info: &crewroom_service::ConnectionInfo, path: &str, body: Value) -> (u16, Value) {
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
    assert!(!text.contains(TOKEN), "response echoed the token: {path}");
    (status, serde_json::from_str(&text).unwrap())
}
async fn workspace(info: &crewroom_service::ConnectionInfo) -> Value {
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
fn github_tasks(view: &Value) -> Vec<Value> {
    view["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["source"] == "github")
        .cloned()
        .collect()
}

#[tokio::test]
async fn github_issues_import_resync_and_report_unavailable_states() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let roles = root.path().join("roles.json");
    let (origin, fixture) = simulated_github().await;
    let service = Service::start_with_options(&data, &roles, 0, options(&origin))
        .await
        .unwrap();
    let info = service.info.clone();

    assert_eq!(
        workspace(&info).await["githubIssues"]["state"],
        "not_configured"
    );
    let (status, _) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(status, 409, "sync without a saved source is refused");
    for bad in [
        json!({"repository":"not a repo"}),
        json!({"repository":"fixture/issues","state":"merged"}),
        json!({"repository":"fixture/issues","token":TOKEN}),
    ] {
        let (status, _) = post(&info, "/github/issues/source", bad).await;
        assert_eq!(status, 400);
    }

    // Project settings: repository plus label, assignee, milestone and state filters.
    let (status, saved) = post(
        &info,
        "/github/issues/source",
        json!({"repository":"fixture/issues","labels":"crewroom","assignee":"octo-fixture","milestone":"Sprint 1","state":"all"}),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["githubIssues"]["state"], "not_synced");
    assert_eq!(
        saved["githubIssues"]["filters"],
        "state all · labels crewroom · assignee octo-fixture · milestone Sprint 1"
    );
    assert_eq!(saved["githubIssues"]["connection"], "disconnected");

    // Missing connection: recorded truthfully, nothing imported.
    let (status, missing) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(missing["githubIssues"]["state"], "unavailable");
    assert_eq!(missing["githubIssues"]["reason"], "not_connected");
    assert!(github_tasks(&workspace(&info).await).is_empty());

    let (status, _) = post(&info, "/github/connect", json!({"token": TOKEN})).await;
    assert_eq!(status, 200);
    let (status, synced) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(status, 200, "{synced}");
    assert_eq!(synced["githubIssues"]["state"], "connected", "{synced}");
    assert_eq!(synced["githubIssues"]["issueCount"], 3);
    // Filters reached GitHub; the milestone title was resolved to its number.
    let first = fixture.queries.lock().unwrap()[0].clone();
    assert_eq!(first["labels"], "crewroom");
    assert_eq!(first["assignee"], "octo-fixture");
    assert_eq!(first["milestone"], "7");
    assert_eq!(first["state"], "all");
    assert_eq!(first["per_page"], "100");
    assert_eq!(fixture.queries.lock().unwrap().len(), 2, "both pages read");

    let view = workspace(&info).await;
    let tasks = github_tasks(&view);
    let keys: Vec<_> = tasks.iter().map(|t| t["key"].clone()).collect();
    assert_eq!(keys.len(), 3, "the pull request is excluded");
    for key in ["#1", "#3", "#4"] {
        assert!(keys.contains(&json!(key)), "{key} imported");
    }
    let one = tasks.iter().find(|t| t["key"] == "#1").unwrap().clone();
    assert_eq!(one["sourceStatus"], "open");
    assert_eq!(one["sourceSite"], "https://github.com/fixture/issues");
    assert_eq!(one["sourceId"], "9001");
    assert_eq!(
        one["sourceUrl"],
        "https://github.com/fixture/issues/issues/1"
    );
    assert_eq!(one["project"], "fixture/issues");
    assert_eq!(one["sourceUpdatedAt"], "2026-10-18T08:00:00Z");
    let three = tasks.iter().find(|t| t["key"] == "#3").unwrap();
    assert_eq!(three["sourceStatus"], "closed");
    // GitHub state is a source fact, not workflow state or agent activity.
    assert!(view["runs"].as_array().unwrap().is_empty());

    // The operator refines and accepts the brief; a re-sync with a changed
    // title updates the same ticket and asks for re-acceptance.
    let id = one["id"].as_str().unwrap().to_owned();
    let (status, refined) = post(
        &info,
        &format!("/tasks/{id}/refinement"),
        json!({"revision":1,"sourceRevision":1,"brief":"Operator brief","criteria":["Synthetic criterion"]}),
    )
    .await;
    assert_eq!(status, 200, "{refined}");
    *fixture.first_title.lock().unwrap() = "Synthetic issue 1, revised".into();
    let (status, _) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(status, 200);
    let tasks = github_tasks(&workspace(&info).await);
    assert_eq!(tasks.len(), 3, "re-sync matched on source identity");
    let revised = tasks.iter().find(|t| t["id"] == json!(id)).unwrap();
    assert_eq!(revised["title"], "Synthetic issue 1, revised");
    assert_eq!(revised["sourceRevision"], 2);
    assert_eq!(revised["needsReacceptance"], true);
    assert_eq!(revised["brief"], "Operator brief");
    let last_synced = workspace(&info).await["githubIssues"]["lastSyncedAt"].clone();
    assert!(last_synced.is_string());

    // Rate limited: reported with GitHub's reset time; earlier tickets kept.
    *fixture.rate_limited.lock().unwrap() = true;
    let (_, limited) = post(&info, "/github/issues/sync", json!({})).await;
    let limited = &limited["githubIssues"];
    assert_eq!(limited["state"], "unavailable");
    assert_eq!(limited["reason"], "rate_limited");
    let error = limited["error"].as_str().unwrap();
    assert!(error.contains("rate-limited"), "{error}");
    assert!(error.contains("resets at 2026-10-18T08:00:00"), "{error}");
    assert_eq!(limited["lastSyncedAt"], last_synced);
    assert_eq!(github_tasks(&workspace(&info).await).len(), 3);
    *fixture.rate_limited.lock().unwrap() = false;

    // Not installed / no access, and issues disabled.
    for (repository, reason, phrase) in [
        ("fixture/private", "no_access", "not installed"),
        ("fixture/disabled", "issues_disabled", "Issues are disabled"),
    ] {
        let (status, _) = post(
            &info,
            "/github/issues/source",
            json!({"repository": repository}),
        )
        .await;
        assert_eq!(status, 200);
        let (_, failed) = post(&info, "/github/issues/sync", json!({})).await;
        assert_eq!(failed["githubIssues"]["reason"], reason, "{failed}");
        assert!(
            failed["githubIssues"]["error"]
                .as_str()
                .unwrap()
                .contains(phrase),
            "{failed}"
        );
    }
    // A revoked token is reported as rejected.
    post(
        &info,
        "/github/issues/source",
        json!({"repository":"fixture/issues"}),
    )
    .await;
    *fixture.revoked.lock().unwrap() = true;
    let (_, rejected) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(rejected["githubIssues"]["reason"], "rejected");
    // Changing the source never removes earlier tickets or their refinements.
    let kept = github_tasks(&workspace(&info).await);
    assert_eq!(kept.len(), 3);
    drop(service);
}

fn task_by_key(view: &Value, key: &str) -> Value {
    github_tasks(view)
        .into_iter()
        .find(|t| t["key"] == key)
        .unwrap_or_else(|| panic!("{key} imported"))
}

#[tokio::test]
async fn github_issues_refresh_tickets_that_leave_the_filter_and_survive_gone_issues() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let roles = root.path().join("roles.json");
    let (origin, fixture) = simulated_github().await;
    let service = Service::start_with_options(&data, &roles, 0, options(&origin))
        .await
        .unwrap();
    let info = service.info.clone();
    // The default filter: open issues only.
    let (status, saved) = post(
        &info,
        "/github/issues/source",
        json!({"repository":"fixture/issues"}),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["githubIssues"]["filters"], "state open");
    post(&info, "/github/connect", json!({"token": TOKEN})).await;
    let (_, synced) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(synced["githubIssues"]["issueCount"], 2, "{synced}");
    let view = workspace(&info).await;
    assert_eq!(github_tasks(&view).len(), 2, "closed #3 is not imported");
    assert_eq!(task_by_key(&view, "#1")["sourceStatus"], "open");

    // Closing #1 on GitHub drops it from the open listing; the re-sync still
    // records that it is closed instead of leaving it shown as open.
    fixture.closed.lock().unwrap().insert(1);
    let (_, synced) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(synced["githubIssues"]["state"], "connected", "{synced}");
    assert_eq!(synced["githubIssues"]["issueCount"], 1);
    let view = workspace(&info).await;
    assert_eq!(task_by_key(&view, "#1")["sourceStatus"], "closed");
    assert_eq!(task_by_key(&view, "#4")["sourceStatus"], "open");
    // Reopened, it matches again.
    fixture.closed.lock().unwrap().remove(&1);
    post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(
        task_by_key(&workspace(&info).await, "#1")["sourceStatus"],
        "open"
    );

    // A deleted issue (410) and a transferred one (301) no longer fail the
    // whole sync or claim the repository is disabled or unreachable; they are
    // named with the sync result and their tickets keep the last status.
    fixture.gone.lock().unwrap().insert(4, StatusCode::GONE);
    fixture
        .gone
        .lock()
        .unwrap()
        .insert(1, StatusCode::MOVED_PERMANENTLY);
    let (_, synced) = post(&info, "/github/issues/sync", json!({})).await;
    let issues = &synced["githubIssues"];
    assert_eq!(issues["state"], "connected", "{synced}");
    assert_eq!(issues["issueCount"], 0);
    let label = issues["label"].as_str().unwrap();
    assert!(label.contains("no longer serves #1, #4"), "{label}");
    assert!(!label.contains("disabled"), "{label}");
    assert_eq!(issues["unreadable"].as_array().unwrap().len(), 2);
    // Still unreadable on the next sync, then readable again once restored.
    let (_, again) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(again["githubIssues"]["unreadable"], issues["unreadable"]);
    fixture.gone.lock().unwrap().clear();
    let (_, restored) = post(&info, "/github/issues/sync", json!({})).await;
    assert_eq!(restored["githubIssues"]["issueCount"], 2, "{restored}");
    assert_eq!(restored["githubIssues"]["unreadable"], json!([]));

    // A secondary rate limit without rate-limit headers is reported as a rate
    // limit, not as missing access.
    *fixture.secondary_limited.lock().unwrap() = true;
    let (_, limited) = post(&info, "/github/issues/sync", json!({})).await;
    let limited = &limited["githubIssues"];
    assert_eq!(limited["reason"], "rate_limited", "{limited}");
    let error = limited["error"].as_str().unwrap();
    assert!(error.contains("secondary rate limit"), "{error}");
    assert!(!error.contains("not installed"), "{error}");
    assert_eq!(github_tasks(&workspace(&info).await).len(), 2);
    drop(service);
}
