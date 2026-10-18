//! GitHub Issues per project (FOR-45) through the real local service and a
//! simulated GitHub API on loopback (not github.com): confirming GitHub Issues
//! as a project's ticket source (FOR-47) gives it its own filters and sync in
//! Settings → Repositories; the missing connection is stated, then a sync
//! imports the issues with GitHub open/closed as source status, and
//! Settings → Integrations summarises the project.
use super::acceptance_tests::labels;
use super::test_support::{Fixture, click, navigate, open, screen_text, wait};
use super::*;
use gpui_kit::{self as gpui, test::TestWindowExt};

const TOKEN: &str = "ghp_synthetic0desktop0issues";

fn simulated_github() -> String {
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
                if line.trim_end().to_ascii_lowercase()
                    == format!("authorization: bearer {}", TOKEN.to_ascii_lowercase())
                {
                    authorized = true;
                }
            }
            let target = request.split_whitespace().nth(1).unwrap_or("");
            let path = target.split('?').next().unwrap_or("");
            let issues = r#"[{"id":7001,"number":1,"title":"Synthetic open issue","body":"Synthetic body","state":"open","html_url":"https://github.com/fixture/issues/issues/1","updated_at":"2026-10-18T08:00:00Z"},{"id":7002,"number":2,"title":"Synthetic pull request","body":"","state":"open","html_url":"https://github.com/fixture/issues/pull/2","updated_at":"2026-10-18T08:00:00Z","pull_request":{"url":"x"}},{"id":7003,"number":3,"title":"Synthetic closed issue","body":null,"state":"closed","html_url":"https://github.com/fixture/issues/issues/3","updated_at":"2026-10-18T08:00:00Z"}]"#;
            let (status, body) = match (authorized, path) {
                (false, _) => ("401 Unauthorized", r#"{"message":"Bad credentials"}"#),
                (true, "/user") => ("200 OK", r#"{"login":"octo-fixture","id":1}"#),
                (true, "/repos/fixture/issues/issues") if target.contains("labels=crewroom") => {
                    ("200 OK", issues)
                }
                _ => ("404 Not Found", r#"{"message":"Not Found"}"#),
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
fn issues(v: &Workbench, id: &str) -> Value {
    github_issues::project_view(&v.workspace, id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[gpui::test]
fn native_project_confirms_github_issues_sets_filters_and_syncs_truthfully(
    cx: &mut gpui::TestAppContext,
) {
    let origin = simulated_github();
    let mut project = String::new();
    let f = Fixture::configured(
        |store, root| {
            let checkout = std::fs::canonicalize(root).unwrap().join("checkout");
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
                    "git@github.com:fixture/issues.git",
                ],
            );
            project = store
                .create_repository(
                    &json!({"name":"Issues","path":checkout,"targetBranch":"main","checks":[]}),
                )
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
        },
        move |_| crewroom_service::ServiceOptions {
            runtime: None,
            helper: "/usr/bin/false".into(),
            host: "/usr/bin/false".into(),
            tmux: "/usr/bin/false".into(),
            jira: None,
            publication: vec![],
            publication_unavailable: vec![],
            environment: json!({}),
            github: Some(crewroom_service::GitHubOptions {
                client: crewroom_runtime::providers::github::GitHubClient::new(Some(&origin))
                    .unwrap(),
                environment: std::sync::Arc::new(|| None),
            }),
        },
    );
    let id = project;
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["githubIssues"]["projects"] == json!([])
    });
    navigate(cx, h, "import-jira");
    let text = screen_text(cx, h);
    assert!(text.iter().any(|l| l == "GitHub Issues"), "{text:?}");
    assert!(
        text.iter().any(|l| l
            == "No project has GitHub Issues as its ticket source. Confirm it for a project under Settings → Repositories."),
        "{text:?}"
    );

    // Confirming the suggested GitHub source gives the project its own sync,
    // starting with open issues only.
    navigate(cx, h, "repositories");
    click(cx, h, format!("confirm-github-source-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && issues(v, &target)["state"] == "not_synced"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "fixture/issues: saved, not synced yet."),
        "{text:?}"
    );
    assert!(text.iter().any(|l| l == "Filters: state open"), "{text:?}");
    let set = |cx: &mut gpui::TestAppContext,
               pick: fn(&github_issues::IssueSourceInputs) -> Entity<InputState>,
               value: &'static str| {
        let target = id.clone();
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            let input = pick(&v.read(cx).issue_inputs[&target]);
            input.update(cx, |i, cx| i.set_value(value, window, cx));
        })
        .unwrap();
    };
    set(cx, |i| i.labels.clone(), "crewroom");
    set(cx, |i| i.state.clone(), "all");
    click(cx, h, format!("save-github-filters-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && issues(v, &target)["filters"] == "state all · labels crewroom"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "Filters: state all · labels crewroom"),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|l| l == "GitHub is not connected, so sync is unavailable."),
        "{text:?}"
    );

    // Issue delivery (FOR-48): workflow labels are off until named.
    assert!(
        text.iter()
            .any(|l| l == "Workflow labels are off. Progress is reported as issue comments only."),
        "{text:?}"
    );
    set(cx, |i| i.in_review.clone(), "crewroom: in review");
    click(cx, h, format!("save-github-labels-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && issues(v, &target)["statusLabels"]["inReview"] == "crewroom: in review"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "Workflow labels: in progress off; in review \"crewroom: in review\". Labels never change the issue's open/closed state."),
        "{text:?}"
    );
    assert_eq!(
        github_issues::project_view(&f.get("/workspace"), &id).unwrap()["statusLabels"]["inProgress"],
        Value::Null
    );

    // Without a connection the sync is refused truthfully and imports nothing.
    click(cx, h, format!("sync-github-issues-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && issues(v, &target)["reason"] == "not_connected"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l.starts_with("fixture/issues: last sync failed. GitHub is not connected.")),
        "{text:?}"
    );

    navigate(cx, h, "import-jira");
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).credential_inputs.github.clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input(TOKEN, cx);
    })
    .unwrap();
    click(cx, h, "connect-github");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["github"]["state"] == "connected"
    });
    navigate(cx, h, "repositories");
    click(cx, h, format!("sync-github-issues-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && issues(v, &target)["state"] == "connected"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "fixture/issues: 2 issues imported in the last complete sync."),
        "{text:?}"
    );
    // Settings → Integrations names the project beside its status.
    navigate(cx, h, "import-jira");
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "Issues: fixture/issues: 2 issues imported in the last complete sync."),
        "{text:?}"
    );
    assert!(!text.iter().any(|l| l.contains(TOKEN)));
    let tasks = f.get("/workspace")["tasks"].clone();
    let tasks = tasks.as_array().unwrap();
    assert_eq!(tasks.len(), 2, "the pull request is not a ticket");
    for (key, status) in [("#1", "open"), ("#3", "closed")] {
        let task = tasks.iter().find(|t| t["key"] == key).unwrap();
        assert_eq!(task["source"], "github");
        assert_eq!(task["sourceStatus"], status);
        assert_eq!(task["repositoryId"], json!(id), "imported for this project");
        assert_eq!(
            labels(
                cx,
                h,
                Some(&format!("row-source-{}", task["id"].as_str().unwrap()))
            ),
            [format!("GitHub {status}")]
        );
    }
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}
