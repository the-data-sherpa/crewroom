//! Jira refresh per project (FOR-53) through the real local service and a
//! simulated Jira API on loopback (not a live Jira site): confirming the
//! connected Jira site as a project's ticket source (FOR-47) gives it its own
//! filters and refresh in Settings → Projects; a project on another site
//! is shown not connected; Settings → Integrations summarises each project.
use super::test_support::{Fixture, click, navigate, open, screen_text, wait};
use super::*;
use gpui_kit::{self as gpui, test::TestWindowExt};

const TOKEN: &str = "synthetic-desktop-jira-token";
const SITE: &str = "https://fixture.atlassian.net";

fn simulated_jira() -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap_or_default();
            let mut authorized = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if line
                    .trim_end()
                    .eq_ignore_ascii_case(&format!("authorization: Bearer {TOKEN}"))
                {
                    authorized = true;
                }
            }
            let target = request.split_whitespace().nth(1).unwrap_or("");
            // project = "CR" ORDER BY Rank ASC, form-encoded.
            let issues = r#"{"isLast":true,"issues":[{"id":"2","key":"CR-2","fields":{"summary":"Ranked first","description":"Source requirement","updated":"2026-10-18T10:00:00Z","status":{"id":"1","name":"To Do"}}},{"id":"1","key":"CR-1","fields":{"summary":"Ranked second","description":"Source requirement","updated":"2026-10-18T10:00:00Z","status":{"id":"3","name":"In Progress"}}}]}"#;
            let (status, body) = match authorized {
                false => ("401 Unauthorized", r#"{"errorMessages":["Unauthorized"]}"#),
                true if target.starts_with("/rest/api/3/search/jql?")
                    && target.contains("project+%3D+%22CR%22") =>
                {
                    ("200 OK", issues)
                }
                true => ("200 OK", r#"{"isLast":true,"issues":[]}"#),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    origin
}

fn git(checkout: &std::path::Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
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
fn checkout(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let checkout = std::fs::canonicalize(root).unwrap().join(name);
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(
        &checkout,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Seed",
        ],
    );
    git(
        &checkout,
        &[
            "remote",
            "add",
            "origin",
            "git@bitbucket.org:fixture/repo.git",
        ],
    );
    checkout
}
fn refresh(v: &Workbench, id: &str) -> Value {
    jira_refresh::project_view(&v.workspace, id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[gpui::test]
fn native_project_confirms_a_jira_site_sets_filters_and_refreshes_truthfully(
    cx: &mut gpui::TestAppContext,
) {
    let origin = simulated_jira();
    let mut ids = vec![];
    let f = Fixture::configured(
        |store, root| {
            for name in ["Payments", "Elsewhere"] {
                let path = checkout(root, name);
                ids.push(
                    store
                        .create_repository(
                            &json!({"name":name,"path":path,"targetBranch":"main","checks":[]}),
                        )
                        .unwrap()["id"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                );
            }
        },
        move |_| {
            crewroom_service::ServiceOptions {
            runtime: None,
            helper: "/usr/bin/false".into(),
            host: "/usr/bin/false".into(),
            tmux: "/usr/bin/false".into(),
            jira: Some(
                crewroom_runtime::providers::jira::JiraClient::new(
                    &json!({"siteUrl":SITE,"cloudId":"00000000-0000-4000-8000-000000000022","name":"Fixture site"}),
                    std::sync::Arc::new(|| {
                        Box::pin(async {
                            Ok(crewroom_runtime::providers::Credential::Bearer(TOKEN.into()))
                        })
                    }),
                    Some(&origin),
                )
                .unwrap(),
            ),
            publication: vec![],
            publication_unavailable: vec![],
            environment: json!({}),
            github: None,
        }
        },
    );
    let (id, other) = (ids[0].clone(), ids[1].clone());
    let (status, confirmed) = f.post(
        &format!("/repositories/{other}/ticket-source"),
        json!({"tickets":"jira","site":"https://other-fixture.atlassian.net"}),
    );
    assert_eq!(status, 200, "{confirmed}");
    let (h, v, e) = open(cx, &f);
    let target = other.clone();
    wait(cx, h, &v, &e, move |v, _| {
        v.connected && refresh(v, &target)["availability"] == "not_connected"
    });

    // The connected site is offered; one click confirms it for the project.
    navigate(cx, h, "repositories");
    click(cx, h, format!("confirm-jira-site-{id}-0"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && refresh(v, &target)["availability"] == "needs_filters"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "https://fixture.atlassian.net: no filters saved yet. Save a project key, saved filter or JQL, then refresh."),
        "{text:?}"
    );
    assert!(
        text.iter().any(|l| l
            == "Not connected: Crewroom's Jira connection is for https://fixture.atlassian.net, not https://other-fixture.atlassian.net, so this project does not refresh."),
        "{text:?}"
    );
    let target = id.clone();
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).jira_inputs[&target].project_key.clone();
        input.update(cx, |i, cx| i.set_value("cr", window, cx));
    })
    .unwrap();
    click(cx, h, format!("save-jira-filters-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && refresh(v, &target)["filters"] == "project CR"
    });
    click(cx, h, format!("refresh-jira-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && refresh(v, &target)["state"] == "connected"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l
                == "https://fixture.atlassian.net: 2 tickets matched the last complete refresh."),
        "{text:?}"
    );
    assert!(text.iter().any(|l| l == "Filters: project CR"), "{text:?}");

    // Settings → Integrations names each project beside its status.
    navigate(cx, h, "import-jira");
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "Payments: https://fixture.atlassian.net: 2 tickets matched the last complete refresh."),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|l| l == "Jira connection: https://fixture.atlassian.net."),
        "{text:?}"
    );
    assert!(!text.iter().any(|l| l.contains(TOKEN)));
    let workspace = f.get("/workspace");
    let tasks = workspace["tasks"].as_array().unwrap();
    assert_eq!(
        tasks
            .iter()
            .map(|t| t["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["CR-2", "CR-1"]
    );
    for task in tasks {
        assert_eq!(task["source"], "jira");
        assert_eq!(task["repositoryId"], json!(id), "imported for this project");
    }
    assert_eq!(tasks[1]["sourceStatus"], "In Progress");
    assert!(workspace["runs"].as_array().unwrap().is_empty());
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}
