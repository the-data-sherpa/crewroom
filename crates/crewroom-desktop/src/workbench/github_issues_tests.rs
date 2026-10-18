//! Settings → Integrations → GitHub Issues through the real local service and a
//! simulated GitHub API on loopback (not github.com): save a repository with
//! filters, see the missing connection stated, connect, sync, and find the
//! imported issues in the ticket list with GitHub open/closed as source status.
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

#[gpui::test]
fn native_settings_save_a_github_issue_source_and_sync_truthfully(cx: &mut gpui::TestAppContext) {
    let origin = simulated_github();
    let f = Fixture::configured(
        |_, _| {},
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
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["githubIssues"]["state"] == "not_configured"
    });
    navigate(cx, h, "import-jira");
    let text = screen_text(cx, h);
    assert!(text.iter().any(|l| l == "GitHub Issues"), "{text:?}");
    assert!(
        text.iter()
            .any(|l| l == "No GitHub repository is saved as a ticket source.")
    );
    let type_into = |cx: &mut gpui::TestAppContext,
                     pick: fn(&github_issues::IssueSourceInputs) -> Entity<InputState>,
                     value: &str| {
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            let input = pick(&v.read(cx).issue_inputs);
            input.update(cx, |i, cx| i.focus(window, cx));
            window.input(value, cx);
        })
        .unwrap();
    };
    type_into(cx, |i| i.repository.clone(), "fixture/issues");
    type_into(cx, |i| i.labels.clone(), "crewroom");
    type_into(cx, |i| i.state.clone(), "all");
    click(cx, h, "save-github-source");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["githubIssues"]["state"] == "not_synced"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "fixture/issues: saved, not synced yet."),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|l| l == "Filters: state all · labels crewroom")
    );
    assert!(
        text.iter()
            .any(|l| l == "GitHub is not connected, so sync is unavailable.")
    );

    // Issue delivery (FOR-48): workflow labels are off until named.
    assert!(
        text.iter()
            .any(|l| l == "Workflow labels are off. Progress is reported as issue comments only."),
        "{text:?}"
    );
    type_into(cx, |i| i.in_review.clone(), "crewroom: in review");
    click(cx, h, "save-github-labels");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["githubIssues"]["statusLabels"]["inReview"] == "crewroom: in review"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "Workflow labels: in progress off; in review \"crewroom: in review\". Labels never change the issue's open/closed state."),
        "{text:?}"
    );
    assert_eq!(
        f.get("/workspace")["githubIssues"]["statusLabels"]["inProgress"],
        Value::Null
    );

    // Without a connection the sync is refused truthfully and imports nothing.
    click(cx, h, "sync-github-issues");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["githubIssues"]["reason"] == "not_connected"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l.starts_with("fixture/issues: last sync failed. GitHub is not connected.")),
        "{text:?}"
    );

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
    click(cx, h, "sync-github-issues");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["githubIssues"]["state"] == "connected"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "fixture/issues: 2 issues imported in the last complete sync."),
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
        assert_eq!(
            labels(
                cx,
                h,
                Some(&format!("row-jira-{}", task["id"].as_str().unwrap()))
            ),
            [format!("GitHub {status}")]
        );
    }
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}
