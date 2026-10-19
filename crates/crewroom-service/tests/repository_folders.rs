//! Choosing a repository folder through the real local service (FOR-70):
//! inspection is an authenticated read that writes nothing; initialization is
//! a mutation accepted only from the service's own origin.
use crewroom_service::{ConnectionInfo, Service};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}");
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned()
}
async fn inspect(info: &ConnectionInfo, query: &[(&str, &str)], token: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .get(format!("{}/api/repository-folders/inspect", info.origin))
        .query(query)
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}
async fn post(
    info: &ConnectionInfo,
    path: &str,
    body: Value,
    origin: Option<&str>,
) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/api{path}", info.origin))
        .bearer_auth(&info.token)
        .json(&body);
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

#[tokio::test]
async fn inspection_reads_and_confirmed_initialization_is_same_origin_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let service = Service::start(&data, &root.join("roles.json"), 0)
        .await
        .unwrap();
    let info = &service.info;
    let folder = root.join("project");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("readme.txt"), "existing").unwrap();
    let path = folder.to_str().unwrap();

    let (status, _) = inspect(info, &[("path", path)], &"0".repeat(64)).await;
    assert_eq!(status, 403, "inspection requires the service token");
    let (status, view) = inspect(info, &[("path", path)], &info.token).await;
    assert_eq!(status, 200);
    assert_eq!(view["state"], "notRepository");
    assert!(!folder.join(".git").exists(), "inspection writes nothing");

    let body = json!({"path":path,"targetBranch":"trunk","confirmed":true});
    for origin in [None, Some("http://127.0.0.1:1")] {
        let (status, _) = post(info, "/repository-folders/initialize", body.clone(), origin).await;
        assert_eq!(status, 403, "{origin:?}");
    }
    let (status, error) = post(
        info,
        "/repository-folders/initialize",
        json!({"path":path,"targetBranch":"trunk","confirmed":false}),
        Some(&info.origin),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        error["error"],
        "Confirm initialization first. Nothing was written."
    );
    assert!(!folder.join(".git").exists());

    let (status, view) = post(
        info,
        "/repository-folders/initialize",
        body,
        Some(&info.origin),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["state"], "repository");
    assert_eq!(view["defaultBranch"], "trunk");
    assert_eq!(git(&folder, &["rev-list", "--count", "trunk"]), "1");
    assert_eq!(git(&folder, &["ls-files", "--cached"]), "");

    // Saving re-verifies; the saved checkout is then refused by inspection,
    // except for the repository being edited.
    let (status, saved) = post(
        info,
        "/repositories",
        json!({"name":"project","path":path,"targetBranch":"trunk","checks":[]}),
        Some(&info.origin),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    let (_, view) = inspect(info, &[("path", path)], &info.token).await;
    assert_eq!(view["refusal"], "alreadyConfigured");
    let id = saved["id"].as_str().unwrap();
    let (_, view) = inspect(info, &[("path", path), ("repository", id)], &info.token).await;
    assert_eq!(view["state"], "repository");
    service.close().await.unwrap();
}
