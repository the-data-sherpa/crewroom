//! FOR-79: per-project setting overrides through the local service.
use crewroom_service::{ConnectionInfo, Service};
use serde_json::{Value, json};

async fn post(info: &ConnectionInfo, path: &str, body: Value, origin: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/api{path}", info.origin))
        .bearer_auth(&info.token)
        .header("Origin", origin)
        .json(&body)
        .send()
        .await
        .unwrap()
}
async fn workspace(info: &ConnectionInfo) -> Value {
    reqwest::Client::new()
        .get(format!("{}/api/workspace", info.origin))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
fn checkout(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = root.join(name);
    std::fs::create_dir(&path).unwrap();
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
                .current_dir(&path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    path
}

#[tokio::test]
async fn overrides_save_reset_and_report_without_touching_other_layers() {
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
    let server = Service::start(root.path(), &roles, 0).await.unwrap();
    let info = &server.info;
    let origin = info.origin.clone();
    let mut ids = vec![];
    for name in ["alpha", "beta"] {
        let repo: Value = post(
            info,
            "/repositories",
            json!({"name":name,"path":checkout(root.path(), name),"targetBranch":"main"}),
            &origin,
        )
        .await
        .json()
        .await
        .unwrap();
        ids.push(repo["id"].as_str().unwrap().to_owned());
    }
    let (alpha, beta) = (&ids[0], &ids[1]);
    // A foreign origin cannot change settings.
    let refused = post(
        info,
        "/project-settings",
        json!({"repositoryId":alpha,"set":{"capacity":2}}),
        "http://example.invalid",
    )
    .await;
    assert!(refused.status().is_client_error());
    assert_eq!(
        workspace(info).await["projectSettings"],
        json!({"workspace":{},"projects":{}})
    );

    let saved = post(
        info,
        "/project-settings",
        json!({"repositoryId":alpha,"set":{"models.budget":"small","roles.reviewer":false,"instructions":"Alpha only.","capacity":2}}),
        &origin,
    )
    .await;
    assert!(saved.status().is_success());
    let saved = post(
        info,
        "/project-settings",
        json!({"repositoryId":null,"set":{"instructions":"Everyone.","capacity":3}}),
        &origin,
    )
    .await;
    assert!(saved.status().is_success());
    let view = workspace(info).await;
    assert_eq!(
        view["projectSettings"]["projects"][alpha]["instructions"],
        "Alpha only."
    );
    assert_eq!(
        view["projectSettings"]["projects"][alpha]["models.budget"],
        "small"
    );
    assert_eq!(
        view["projectSettings"]["workspace"]["instructions"],
        "Everyone."
    );
    let capacity = |view: &Value, id: &str| {
        view["queue"]["capacity"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["repositoryId"] == id)
            .unwrap()["capacity"]
            .clone()
    };
    assert_eq!(capacity(&view, alpha), 2);
    assert_eq!(capacity(&view, beta), 3);
    // The workspace role catalog is unchanged by a project's role override.
    let reviewer = view["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "reviewer")
        .unwrap()
        .clone();
    assert_eq!(reviewer["enabled"], true);
    assert!(reviewer.get("projectSettings").is_none());

    // Models are not a workspace setting here; unknown roles are refused.
    for body in [
        json!({"repositoryId":null,"set":{"models.budget":"small"}}),
        json!({"repositoryId":alpha,"set":{"roles.unknown-role":true}}),
        json!({"repositoryId":alpha,"set":{"principles":["missing"]}}),
    ] {
        assert!(
            post(info, "/project-settings", body.clone(), &origin)
                .await
                .status()
                .is_client_error(),
            "{body}"
        );
    }
    let reset = post(
        info,
        "/project-settings",
        json!({"repositoryId":alpha,"reset":["instructions","capacity"]}),
        &origin,
    )
    .await;
    assert!(reset.status().is_success());
    let view = workspace(info).await;
    assert!(
        view["projectSettings"]["projects"][alpha]
            .get("instructions")
            .is_none()
    );
    assert_eq!(
        view["projectSettings"]["projects"][alpha]["roles.reviewer"],
        false
    );
    assert_eq!(capacity(&view, alpha), 3);
}

/// FOR-88: a role preview for a project resolves the role as that project
/// admits it; without one it stays the workspace resolution.
#[tokio::test]
async fn a_role_preview_resolves_the_selected_project() {
    let root = tempfile::tempdir().unwrap();
    let roles = root.path().join("roles.json");
    let server = Service::start(root.path(), &roles, 0).await.unwrap();
    let info = &server.info;
    let origin = info.origin.clone();
    let mut ids = vec![];
    for name in ["alpha", "beta"] {
        let repo: Value = post(
            info,
            "/repositories",
            json!({"name":name,"path":checkout(root.path(), name),"targetBranch":"main"}),
            &origin,
        )
        .await
        .json()
        .await
        .unwrap();
        ids.push(repo["id"].as_str().unwrap().to_owned());
    }
    let (alpha, beta) = (&ids[0], &ids[1]);
    let saved = post(
        info,
        "/project-settings",
        json!({"repositoryId":alpha,"set":{"instructions":"Use the alpha fixtures.","models.jobs.code":"alpha/code"}}),
        &origin,
    )
    .await;
    assert!(saved.status().is_success());
    let task: Value = post(
        info,
        "/tasks",
        json!({"title":"Alpha preview","description":"","repositoryId":alpha}),
        &origin,
    )
    .await
    .json()
    .await
    .unwrap();
    let role = workspace(info).await["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "implementer")
        .unwrap()
        .clone();
    let preview = |project: Value| {
        post(
            info,
            "/role-preview",
            json!({"role":role,"taskId":task["id"],"repositoryId":project}),
            &origin,
        )
    };
    let scoped: Value = preview(json!(alpha)).await.json().await.unwrap();
    assert_eq!(scoped["repositoryId"], json!(alpha));
    assert_eq!(
        scoped["role"]["projectSettings"]["repositoryId"],
        json!(alpha)
    );
    assert_eq!(
        scoped["role"]["projectSettings"]["models"]["jobs"]["code"],
        "alpha/code"
    );
    assert!(
        scoped["prompt"]
            .as_str()
            .unwrap()
            .contains("Use the alpha fixtures.")
    );
    // Without a runner the model stays explicitly unverified.
    assert_eq!(scoped["modelError"], "Runner integration is unavailable.");
    let workspace_preview: Value = preview(Value::Null).await.json().await.unwrap();
    assert!(workspace_preview["repositoryId"].is_null());
    assert!(workspace_preview["role"].get("projectSettings").is_none());
    assert!(
        !workspace_preview["prompt"]
            .as_str()
            .unwrap()
            .contains("Use the alpha fixtures.")
    );
    // The ticket must belong to the project being previewed.
    for project in [json!(beta), json!(7)] {
        assert!(
            preview(project.clone()).await.status().is_client_error(),
            "{project}"
        );
    }
}
