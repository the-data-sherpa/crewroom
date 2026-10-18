//! Comparison reads and preflight must not mutate catalogs or launch work.
use crewroom_service::{ConnectionInfo, Service, ServiceOptions};
use serde_json::{Value, json};

async fn get(info: &ConnectionInfo, path: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{}/api{path}", info.origin))
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
fn request(kind: &str, draft: &Value, key: &str) -> Value {
    json!({"kind":kind,"targetId":draft["id"],"baseRevision":draft["revision"],"draft":draft,"requestId":key})
}
#[tokio::test]
async fn comparison_preflight_is_readonly_and_ordinary_saves_work_for_all_instruction_kinds() {
    let root = tempfile::tempdir().unwrap();
    let hosts = root.path().join("hosts");
    std::fs::create_dir(&hosts).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hosts, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = json!({"root":hosts,"socket":hosts.join("tmux.sock"),"ompBinary":"/usr/bin/false",
        "mail":{"url":"http://127.0.0.1:9/mcp/","token":"synthetic-comparison-fixture","projectRoot":root.path(),"retentionSeconds":86400,"atcDisabled":true},
        "catalog":[{"provider":"fixture","id":"model","api":"openai-responses","inputs":["text"],"efforts":["low"],"tools":true}],"policy":{},"models":{"providers":{}}});
    let server = Service::start_with_options(
        root.path(),
        &root.path().join("roles.json"),
        0,
        ServiceOptions {
            runtime: Some(runtime),
            helper: "/usr/bin/false".into(),
            host: "/usr/bin/false".into(),
            tmux: "/usr/bin/false".into(),
            jira: None,
            publication: vec![],
            publication_unavailable: vec![],
            environment: json!({}),
            github: None,
        },
    )
    .await
    .unwrap();
    for (path, body) in [
        (
            "/principles",
            json!({"id":"comparison-principle","name":"Verification","text":"Current","revision":0}),
        ),
        (
            "/playbooks",
            json!({"id":"comparison-playbook","name":"Verification","description":"Current","steps":[{"id":"check","title":"Check"}],"revision":0}),
        ),
    ] {
        let response = post(&server.info, path, body).await;
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
    }
    let before = get(&server.info, "/workspace").await;
    for (kind, catalog, id, field, path) in [
        (
            "role",
            "roles",
            "implementer",
            "instructions",
            "/roles/implementer",
        ),
        (
            "principle",
            "principles",
            "comparison-principle",
            "text",
            "/principles",
        ),
        (
            "playbook",
            "playbooks",
            "comparison-playbook",
            "description",
            "/playbooks",
        ),
    ] {
        let saved = before[catalog]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .unwrap()
            .clone();
        let mut draft = saved.clone();
        if draft["revision"].is_null() {
            draft["revision"] = json!(1);
        }
        draft[field] = json!("Check the current revision before handing off.");
        let response = post(
            &server.info,
            "/comparisons/preflight",
            request(kind, &draft, &format!("preflight-{kind}")),
        )
        .await;
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
        let preflight: Value = response.json().await.unwrap();
        assert_eq!(
            preflight["canStart"], false,
            "No usable protected comparison execution environment is configured"
        );
        assert!(!preflight["reasons"].as_array().unwrap().is_empty());
        assert_eq!(
            get(&server.info, "/comparisons").await,
            json!([]),
            "Preflight must not create history or cells"
        );
        let after = get(&server.info, "/workspace").await;
        assert_eq!(after["runs"], before["runs"]);
        assert_eq!(after["tasks"], before["tasks"]);
        assert_eq!(
            after[catalog]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == id)
                .unwrap(),
            &saved
        );
        // This succeeds without any comparison receipt or adoption token.
        let response = post(&server.info, path, draft.clone()).await;
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
        let stale = post(
            &server.info,
            "/comparisons/preflight",
            request(kind, &draft, &format!("stale-{kind}")),
        )
        .await;
        let stale: Value = stale.json().await.unwrap();
        assert_eq!(
            stale["canStart"], false,
            "Stale target revisions must not be compared as current"
        );
        assert!(stale["reasons"].to_string().contains("changed"), "{stale}");
    }
    assert_eq!(get(&server.info, "/comparisons").await, json!([]));
    server.close().await.unwrap();
}
#[tokio::test]
async fn comparisons_keep_the_loopback_auth_boundary_and_reject_untrusted_creation() {
    let root = tempfile::tempdir().unwrap();
    let server = Service::start(root.path(), &root.path().join("roles.json"), 0)
        .await
        .unwrap();
    let http = reqwest::Client::new();
    for path in ["/comparisons", "/comparisons/missing"] {
        assert_eq!(
            http.get(format!("{}/api{path}", server.info.origin))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    for path in [
        "/comparisons/preflight",
        "/comparisons",
        "/comparisons/missing/control",
        "/comparisons/missing/adoption",
    ] {
        assert_eq!(
            http.post(format!("{}/api{path}", server.info.origin))
                .bearer_auth(&server.info.token)
                .header("Origin", "https://other.invalid")
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let forged = post(
        &server.info,
        "/comparisons",
        json!({"requestId":"forged-comparison","manifest":{"version":1,"mode":"live"}}),
    )
    .await;
    assert!(!forged.status().is_success());
    let workspace = get(&server.info, "/workspace").await;
    assert_eq!(workspace["tasks"], json!([]));
    assert_eq!(workspace["runs"], json!([]));
    assert_eq!(get(&server.info, "/comparisons").await, json!([]));
    server.close().await.unwrap();
}
