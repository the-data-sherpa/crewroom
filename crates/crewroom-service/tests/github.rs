//! GitHub personal-access-token connection against a simulated GitHub API
//! (loopback fixture, not github.com). Proves validation before save,
//! encryption at rest, truthful states, disconnect, the environment override
//! and the same-origin mutation boundary. Live github.com evidence is recorded
//! separately in docs/testing.
use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use crewroom_runtime::providers::{github::GitHubClient, secrets};
use crewroom_service::{GitHubOptions, Service, ServiceOptions};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
};

const CLASSIC: &str = "ghp_synthetic0classic0token";
const FINE: &str = "github_pat_synthetic0fine0grained";
const WRONG: &str = "ghp_synthetic0wrong";

#[derive(Clone, Default)]
struct Fixture {
    revoked: Arc<Mutex<HashSet<String>>>,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}
fn bearer(fixture: &Fixture, headers: &HeaderMap, path: &str) -> Option<String> {
    assert_eq!(
        headers.get("user-agent").and_then(|v| v.to_str().ok()),
        Some("crewroom"),
        "GitHub requires a User-Agent"
    );
    assert_eq!(
        headers.get("accept").and_then(|v| v.to_str().ok()),
        Some("application/vnd.github+json")
    );
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())?
        .strip_prefix("Bearer ")?
        .to_owned();
    fixture
        .seen
        .lock()
        .unwrap()
        .push((path.to_owned(), token.clone()));
    let valid =
        (token == CLASSIC || token == FINE) && !fixture.revoked.lock().unwrap().contains(&token);
    valid.then_some(token)
}
fn bad_credentials() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(
            json!({"message":"Bad credentials","documentation_url":"https://docs.github.com/rest"}),
        ),
    )
        .into_response()
}
async fn user(State(fixture): State<Fixture>, headers: HeaderMap) -> Response {
    let Some(token) = bearer(&fixture, &headers, "/user") else {
        return bad_credentials();
    };
    let mut response = Json(json!({"login":"octo-fixture","id":583231,"name":"Fixture Operator","html_url":"https://github.com/octo-fixture"})).into_response();
    if token == CLASSIC {
        let headers = response.headers_mut();
        headers.insert("x-oauth-scopes", "repo, read:org".parse().unwrap());
        headers.insert(
            "github-authentication-token-expiration",
            "2026-12-31 00:00:00 UTC".parse().unwrap(),
        );
    }
    response
}
async fn repository(
    State(fixture): State<Fixture>,
    UrlPath((owner, name)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if bearer(&fixture, &headers, &format!("/repos/{owner}/{name}")).is_none() {
        return bad_credentials();
    }
    if (owner.as_str(), name.as_str()) == ("fixture", "visible") {
        Json(json!({"full_name":"fixture/visible","private":true,"permissions":{"admin":false,"push":true,"pull":true}})).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({"message":"Not Found"}))).into_response()
    }
}
async fn simulated_github() -> (String, Fixture) {
    let fixture = Fixture::default();
    let app = Router::new()
        .route("/user", get(user))
        .route("/repos/{owner}/{name}", get(repository))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, fixture)
}
type Environment = Arc<Mutex<Option<String>>>;
fn options(origin: &str, environment: &Environment) -> ServiceOptions {
    let environment = environment.clone();
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
            environment: Arc::new(move || environment.lock().unwrap().clone()),
        }),
    }
}
async fn request(
    info: &crewroom_service::ConnectionInfo,
    path: &str,
    body: Value,
    origin: Option<&str>,
) -> (u16, String) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/api{path}", info.origin))
        .bearer_auth(&info.token)
        .json(&body);
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}
async fn post(info: &crewroom_service::ConnectionInfo, path: &str, body: Value) -> (u16, Value) {
    let (status, text) = request(info, path, body, Some(&info.origin)).await;
    for token in [CLASSIC, FINE, WRONG] {
        assert!(!text.contains(token), "response echoed a token: {path}");
    }
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
    for token in [CLASSIC, FINE, WRONG] {
        assert!(!text.contains(token), "workspace carried a token");
    }
    serde_json::from_str(&text).unwrap()
}
fn data_dir(root: &Path) -> std::path::PathBuf {
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    data
}

#[tokio::test]
async fn connect_validates_the_token_stores_it_encrypted_and_reports_truthful_states() {
    let root = tempfile::tempdir().unwrap();
    let data = data_dir(root.path());
    let roles = root.path().join("roles.json");
    let (origin, fixture) = simulated_github().await;
    let environment = Environment::default();
    let service = Service::start_with_options(&data, &roles, 0, options(&origin, &environment))
        .await
        .unwrap();
    let info = service.info.clone();

    let view = workspace(&info).await;
    assert_eq!(view["github"]["state"], "disconnected");
    assert_eq!(view["credentials"]["github"]["source"], "none");
    assert_eq!(
        view["credentials"]["github"]["environment"],
        "CREWROOM_GITHUB_TOKEN"
    );

    // A token GitHub rejects is refused with the reason and never stored.
    let (status, refused) = post(&info, "/github/connect", json!({"token": WRONG})).await;
    assert_eq!(status, 400, "{refused}");
    let reason = refused["error"].as_str().unwrap();
    assert!(reason.starts_with("Token not saved."), "{reason}");
    assert!(reason.contains("invalid, expired or revoked"), "{reason}");
    assert!(reason.contains("Bad credentials"), "{reason}");
    assert!(!data.join(secrets::CREDENTIALS_FILE).exists());
    // The unvalidated save route is closed for GitHub.
    let (status, _) = post(&info, "/credentials/github", json!({"token": CLASSIC})).await;
    assert_eq!(status, 400);
    assert!(!data.join(secrets::CREDENTIALS_FILE).exists());
    for (name, body) in [
        ("empty", json!({"token": "  "})),
        ("extra field", json!({"token": CLASSIC, "scope": "repo"})),
        ("not an object", json!([CLASSIC])),
    ] {
        let (status, body) = post(&info, "/github/connect", body).await;
        assert_eq!(status, 400, "{name}: {body}");
    }

    // A valid token: validated with /user, saved encrypted, account shown.
    let (status, connected) = post(&info, "/github/connect", json!({"token": CLASSIC})).await;
    assert_eq!(status, 200, "{connected}");
    let github = &connected["github"];
    assert_eq!(github["state"], "connected");
    assert_eq!(github["source"], "stored");
    assert_eq!(github["tokenKind"], "classic");
    assert_eq!(github["account"]["login"], "octo-fixture");
    assert_eq!(github["account"]["scopes"], json!(["repo", "read:org"]));
    assert_eq!(github["account"]["expiresAt"], "2026-12-31 00:00:00 UTC");
    assert_eq!(github["label"], "Connected as @octo-fixture.");
    assert_eq!(connected["credentials"]["github"]["source"], "stored");
    let raw = std::fs::read_to_string(data.join(secrets::CREDENTIALS_FILE)).unwrap();
    assert!(!raw.contains(CLASSIC) && raw.contains("$crewroom$v1$"));
    assert_eq!(
        std::fs::metadata(data.join(secrets::CREDENTIALS_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(workspace(&info).await["github"]["state"], "connected");

    // Repository access: visible, and "no access to this repo".
    let (status, checked) = post(
        &info,
        "/github/check",
        json!({"repository": "fixture/visible"}),
    )
    .await;
    assert_eq!(status, 200, "{checked}");
    assert_eq!(checked["github"]["state"], "connected");
    assert_eq!(checked["github"]["repository"]["access"], "granted");
    assert_eq!(checked["github"]["repository"]["permissions"]["push"], true);
    let (_, hidden) = post(
        &info,
        "/github/check",
        json!({"repository": "fixture/hidden"}),
    )
    .await;
    assert_eq!(hidden["github"]["state"], "connected");
    assert_eq!(hidden["github"]["repository"]["access"], "none");
    assert!(
        hidden["github"]["repository"]["label"]
            .as_str()
            .unwrap()
            .starts_with("No access to fixture/hidden")
    );
    let (status, _) = post(&info, "/github/check", json!({"repository": "../x"})).await;
    assert_eq!(status, 400);

    // Revoked on GitHub: the next check says so; the saved token is unchanged.
    fixture.revoked.lock().unwrap().insert(CLASSIC.into());
    let (status, revoked) = post(&info, "/github/check", json!({})).await;
    assert_eq!(status, 200, "{revoked}");
    assert_eq!(revoked["github"]["state"], "rejected");
    assert!(
        revoked["github"]["label"]
            .as_str()
            .unwrap()
            .starts_with("Expired or revoked:")
    );
    assert_eq!(revoked["credentials"]["github"]["source"], "stored");
    fixture.revoked.lock().unwrap().clear();

    // Mutations stay same-origin and authenticated.
    for (name, origin) in [
        ("foreign origin", Some("http://127.0.0.1:1")),
        ("no origin", None),
    ] {
        let (status, body) = request(&info, "/github/disconnect", json!({}), origin).await;
        assert_eq!(status, 403, "{name}: {body}");
    }
    let unauthenticated = reqwest::Client::new()
        .post(format!("{}/api/github/disconnect", info.origin))
        .header("Origin", &info.origin)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status().as_u16(), 403);
    assert_eq!(
        workspace(&info).await["credentials"]["github"]["source"],
        "stored"
    );

    // CREWROOM_GITHUB_TOKEN overrides the saved token and is reported.
    *environment.lock().unwrap() = Some(FINE.into());
    let view = workspace(&info).await;
    assert_eq!(view["github"]["source"], "environment");
    assert_eq!(view["github"]["state"], "unchecked");
    assert_eq!(view["credentials"]["github"]["source"], "environment");
    assert_eq!(view["credentials"]["github"]["stored"], "stored");
    let (_, checked) = post(&info, "/github/check", json!({})).await;
    assert_eq!(checked["github"]["state"], "connected");
    assert_eq!(checked["github"]["tokenKind"], "fine-grained");
    assert!(
        checked["github"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("fine-grained")
    );
    assert_eq!(fixture.seen.lock().unwrap().last().unwrap().1, FINE);
    // Disconnect removes the saved token; the override stays active.
    let (status, disconnected) = post(&info, "/github/disconnect", json!({})).await;
    assert_eq!(status, 200, "{disconnected}");
    assert_eq!(disconnected["credentials"]["github"]["stored"], "none");
    assert_eq!(disconnected["github"]["source"], "environment");
    assert_eq!(disconnected["github"]["state"], "connected");
    *environment.lock().unwrap() = None;
    let view = workspace(&info).await;
    assert_eq!(view["github"]["state"], "disconnected");
    let raw = std::fs::read_to_string(data.join(secrets::CREDENTIALS_FILE)).unwrap();
    assert!(!raw.contains("github"), "{raw}");
    let (status, _) = post(&info, "/github/check", json!({})).await;
    assert_eq!(status, 409);

    // A saved token survives restart and is shown as not yet re-checked.
    let (status, _) = post(&info, "/github/connect", json!({"token": CLASSIC})).await;
    assert_eq!(status, 200);
    service.close().await.unwrap();
    let restarted = Service::start_with_options(&data, &roles, 0, options(&origin, &environment))
        .await
        .unwrap();
    let view = workspace(&restarted.info).await;
    assert_eq!(view["github"]["state"], "unchecked");
    assert_eq!(view["github"]["source"], "stored");
    let (_, checked) = post(&restarted.info, "/github/check", json!({})).await;
    assert_eq!(checked["github"]["state"], "connected");
    restarted.close().await.unwrap();

    // Lost install key: unreadable, no request is sent, reconnect replaces it.
    std::fs::remove_file(data.join(secrets::SECRET_FILE)).unwrap();
    let service = Service::start_with_options(&data, &roles, 0, options(&origin, &environment))
        .await
        .unwrap();
    let view = workspace(&service.info).await;
    assert_eq!(view["github"]["state"], "unreadable");
    let before = fixture.seen.lock().unwrap().len();
    let (status, _) = post(&service.info, "/github/check", json!({})).await;
    assert_eq!(status, 409);
    assert_eq!(fixture.seen.lock().unwrap().len(), before);
    let (status, _) = post(&service.info, "/github/connect", json!({"token": CLASSIC})).await;
    assert_eq!(status, 409, "an ordinary save must not orphan values");
    let (status, connected) = post(
        &service.info,
        "/github/connect",
        json!({"token": CLASSIC, "replaceUnreadable": true}),
    )
    .await;
    assert_eq!(status, 200, "{connected}");
    assert_eq!(connected["github"]["state"], "connected");
    // The generic clear route also disconnects.
    let (status, cleared) = post(&service.info, "/credentials/github/clear", json!({})).await;
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared["github"]["state"], "disconnected");
    service.close().await.unwrap();
}

#[tokio::test]
async fn an_unreachable_github_is_not_reported_as_a_rejected_token() {
    let root = tempfile::tempdir().unwrap();
    let data = data_dir(root.path());
    let roles = root.path().join("roles.json");
    // Bind and drop a listener so the port refuses connections.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let environment = Environment::default();
    let service = Service::start_with_options(&data, &roles, 0, options(&closed, &environment))
        .await
        .unwrap();
    let (status, refused) = post(&service.info, "/github/connect", json!({"token": CLASSIC})).await;
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("could not be validated")
    );
    assert!(!data.join(secrets::CREDENTIALS_FILE).exists());
    // With an override, a failed check reads "not confirmed", not revoked.
    *environment.lock().unwrap() = Some(CLASSIC.into());
    let (status, checked) = post(&service.info, "/github/check", json!({})).await;
    assert_eq!(status, 200, "{checked}");
    assert_eq!(checked["github"]["state"], "unavailable");
    assert!(
        checked["github"]["label"]
            .as_str()
            .unwrap()
            .starts_with("Connection not confirmed")
    );
    service.close().await.unwrap();
}
