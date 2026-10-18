//! A project's ticket source suggested from its repository remote (FOR-47),
//! through the real local service with Git checkouts carrying synthetic
//! remotes. No remote is contacted; only the configured address is read.
use crewroom_service::Service;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

fn git(checkout: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(args)
            .output()
            .unwrap()
            .status
            .success(),
        "{args:?}"
    );
}
fn checkout(root: &Path, name: &str, remote: Option<&str>) -> std::path::PathBuf {
    let path = std::fs::canonicalize(root).unwrap().join(name);
    std::fs::create_dir(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(
        &path,
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
    );
    if let Some(remote) = remote {
        git(&path, &["remote", "add", "origin", remote]);
    }
    path
}
async fn post(
    info: &crewroom_service::ConnectionInfo,
    path: &str,
    body: Value,
    origin: &str,
) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/api{path}", info.origin))
        .bearer_auth(&info.token)
        .header("Origin", origin)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}
async fn workspace(info: &crewroom_service::ConnectionInfo) -> String {
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
async fn source_of(info: &crewroom_service::ConnectionInfo, id: &str) -> Value {
    let view: Value = serde_json::from_str(&workspace(info).await).unwrap();
    view["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap()["projectSource"]
        .clone()
}

#[tokio::test]
async fn linked_repositories_get_a_suggestion_confirmation_and_mismatch_warning() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let roles = root.path().join("roles.json");
    let service = Service::start(&data, &roles, 0).await.unwrap();
    let info = &service.info;
    let own = info.origin.clone();

    let github = checkout(
        root.path(),
        "github",
        Some("git@github.com:Octo-Fixture/Sandbox.git"),
    );
    let bitbucket = checkout(
        root.path(),
        "bitbucket",
        Some("https://bitbucket.org/fixture/repository.git"),
    );
    let other = checkout(
        root.path(),
        "other",
        Some("https://fixture:synthetic-secret@gitlab.example/fixture/repository.git"),
    );
    let mut ids = vec![];
    for (name, path) in [
        ("GitHub", &github),
        ("Bitbucket", &bitbucket),
        ("Other", &other),
    ] {
        let (status, saved) = post(
            info,
            "/repositories",
            json!({"name":name,"path":path,"targetBranch":"main","checks":[]}),
            &own,
        )
        .await;
        assert_eq!(status, 200, "{saved}");
        ids.push(saved["id"].as_str().unwrap().to_owned());
    }
    // A credentialed remote is never projected.
    assert!(!workspace(info).await.contains("synthetic-secret"));

    // GitHub: suggested with owner/repo prefilled, nothing saved yet.
    let suggested = source_of(info, &ids[0]).await;
    assert_eq!(suggested["state"], "unconfirmed");
    assert_eq!(suggested["suggestion"]["tickets"], "github");
    assert_eq!(
        suggested["suggestion"]["repository"],
        "Octo-Fixture/Sandbox"
    );
    assert_eq!(suggested["suggestion"]["pullRequests"], "github");
    assert!(suggested["saved"].is_null());
    // Bitbucket: Jira + Bitbucket, the site must be picked.
    let jira = source_of(info, &ids[1]).await;
    assert_eq!(jira["suggestion"]["tickets"], "jira");
    assert_eq!(jira["suggestion"]["pullRequests"], "bitbucket");
    assert_eq!(jira["suggestion"]["needsSite"], true);
    // Anything else: the operator chooses.
    let choose = source_of(info, &ids[2]).await;
    assert!(choose["suggestion"]["tickets"].is_null());
    assert_eq!(choose["remote"]["state"], "unrecognised");

    // Mutations keep the same-origin boundary.
    let (status, _) = post(
        info,
        &format!("/repositories/{}/ticket-source", ids[0]),
        json!({"tickets":"github","repository":"Octo-Fixture/Sandbox"}),
        "https://evil.example",
    )
    .await;
    assert_eq!(status, 403);
    assert!(source_of(info, &ids[0]).await["saved"].is_null());

    // One explicit confirmation saves it.
    let (status, confirmed) = post(
        info,
        &format!("/repositories/{}/ticket-source", ids[0]),
        json!({"tickets":"github","repository":"Octo-Fixture/Sandbox"}),
        &own,
    )
    .await;
    assert_eq!(status, 200, "{confirmed}");
    let view = source_of(info, &ids[0]).await;
    assert_eq!(view["state"], "confirmed");
    assert_eq!(view["saved"]["repository"], "Octo-Fixture/Sandbox");
    assert!(view["changeBlocked"].is_null());

    // A Jira site cannot be inferred: confirming without one is refused.
    let (status, refused) = post(
        info,
        &format!("/repositories/{}/ticket-source", ids[1]),
        json!({"tickets":"jira"}),
        &own,
    )
    .await;
    assert_eq!(status, 400, "{refused}");
    let (status, _) = post(
        info,
        &format!("/repositories/{}/ticket-source", ids[1]),
        json!({"tickets":"jira","site":"https://fixture.atlassian.net"}),
        &own,
    )
    .await;
    assert_eq!(status, 200);
    // The saved site is offered to other projects' pickers.
    assert_eq!(
        source_of(info, &ids[2]).await["jiraSites"],
        json!(["https://fixture.atlassian.net"])
    );

    // The remote moves: a warning, and the saved source is unchanged.
    git(
        &github,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:Octo-Fixture/Moved.git",
        ],
    );
    let moved = source_of(info, &ids[0]).await;
    assert_eq!(moved["state"], "mismatch");
    assert!(
        moved["warning"]
            .as_str()
            .unwrap()
            .starts_with("Source no longer matches remote")
    );
    assert_eq!(moved["saved"]["repository"], "Octo-Fixture/Sandbox");
    assert_eq!(moved["suggestion"]["repository"], "Octo-Fixture/Moved");
    // It stays that way across a service restart.
    service.close().await.unwrap();
    let service = Service::start(&data, &roles, 0).await.unwrap();
    let after = source_of(&service.info, &ids[0]).await;
    assert_eq!(after["state"], "mismatch");
    assert_eq!(after["saved"]["repository"], "Octo-Fixture/Sandbox");
    service.close().await.unwrap();
}
