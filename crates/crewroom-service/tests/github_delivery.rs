//! GitHub issue delivery routes (FOR-48) through the local service: optional
//! workflow labels are off until named and validated, the outbox is projected
//! in the workspace, and reconcile/retry are operator routes that never carry
//! the token. Delivery itself is proved in crewroom-runtime against a simulated
//! GitHub API; live github.com evidence is recorded separately in docs/testing.
use crewroom_runtime::providers::github::GitHubClient;
use crewroom_service::{GitHubOptions, Service, ServiceOptions};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, process::Command, sync::Arc};

const TOKEN: &str = "ghp_synthetic0delivery0token";

fn options() -> ServiceOptions {
    ServiceOptions {
        runtime: None,
        helper: "/usr/bin/false".into(),
        host: "/usr/bin/false".into(),
        tmux: "/usr/bin/false".into(),
        jira: None,
        publication: vec![],
        publication_unavailable: vec![],
        environment: json!({}),
        runner: Default::default(),
        github: Some(GitHubOptions {
            // Nothing in this test reaches GitHub; the loopback origin is unused.
            client: GitHubClient::new(Some("http://127.0.0.1:9/")).unwrap(),
            environment: Arc::new(|| Some(TOKEN.into())),
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

/// A configured project on a Git checkout without a remote. Returns its id.
async fn project(info: &crewroom_service::ConnectionInfo, root: &std::path::Path) -> String {
    let path = std::fs::canonicalize(root).unwrap().join("checkout");
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
        json!({"name":"Fixture","path":path,"targetBranch":"main","checks":[]}),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    saved["id"].as_str().unwrap().to_owned()
}
fn issues_of(value: &Value, id: &str) -> Value {
    value["githubIssues"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["repositoryId"] == id)
        .cloned()
        .unwrap()
}

#[tokio::test]
async fn workflow_labels_are_opt_in_and_delivery_routes_report_the_outbox() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let roles = root.path().join("roles.json");
    let service = Service::start_with_options(&data, &roles, 0, options())
        .await
        .unwrap();
    let info = service.info.clone();

    let p = project(&info, root.path()).await;
    let labels = format!("/repositories/{p}/github-issues/labels");
    let (status, _) = post(
        &info,
        &labels,
        json!({"inProgress":"crewroom: in progress"}),
    )
    .await;
    assert_eq!(status, 409, "labels need a confirmed GitHub source");
    let (status, confirmed) = post(
        &info,
        &format!("/repositories/{p}/ticket-source"),
        json!({"tickets":"github","repository":"fixture/issues"}),
    )
    .await;
    assert_eq!(status, 200, "{confirmed}");
    let saved = workspace(&info).await;
    assert_eq!(
        issues_of(&saved, &p)["statusLabels"],
        json!({"inProgress":null,"inReview":null}),
        "off by default"
    );
    assert!(
        issues_of(&saved, &p)["labelsNote"]
            .as_str()
            .unwrap()
            .contains("off")
    );
    for bad in [
        json!({"inProgress":"a,b"}),
        json!({"inProgress":"same","inReview":"Same"}),
        json!({"token":TOKEN}),
    ] {
        let (status, _) = post(&info, &labels, bad).await;
        assert_eq!(status, 400);
    }
    let (status, labelled) = post(
        &info,
        &labels,
        json!({"inProgress":"crewroom: in progress","inReview":"crewroom: in review"}),
    )
    .await;
    assert_eq!(status, 200, "{labelled}");
    assert_eq!(
        issues_of(&labelled, &p)["statusLabels"]["inReview"],
        "crewroom: in review"
    );
    assert!(
        issues_of(&labelled, &p)["labelsNote"]
            .as_str()
            .unwrap()
            .contains("never change the issue's open/closed state")
    );

    let view = workspace(&info).await;
    assert_eq!(view["githubUpdates"], json!([]));
    assert_eq!(
        issues_of(&view, &p)["statusLabels"]["inProgress"],
        "crewroom: in progress"
    );
    let (status, updates) = post(&info, "/github/delivery/reconcile", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(updates, json!([]));
    let (status, missing) = post(
        &info,
        "/github/delivery/00000000-0000-4000-8000-000000000000/retry",
        json!({}),
    )
    .await;
    assert_eq!(status, 404, "{missing}");
    service.close().await.unwrap();
}
