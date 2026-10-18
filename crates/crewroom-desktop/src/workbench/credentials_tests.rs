//! Settings → Integrations saves provider tokens through the real local service.
//! The token is typed once, stored encrypted, and never rendered or returned.
use super::test_support::{Fixture, click, navigate, open, screen_text, wait};
use super::*;
use gpui_kit::{self as gpui, test::TestWindowExt};

const TOKEN: &str = "synthetic-desktop-jira-token";

#[gpui::test]
fn native_settings_save_a_jira_token_encrypted_and_never_show_it_again(
    cx: &mut gpui::TestAppContext,
) {
    let f = Fixture::start();
    let data = f.root.path().join("data");
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["credentials"]["jira"]["source"] == "none"
    });
    navigate(cx, h, "import-jira");
    let text = screen_text(cx, h);
    assert!(text.iter().any(|l| l == "Jira token"), "{text:?}");
    assert!(text.iter().any(|l| l == "Bitbucket token"));
    assert!(text.iter().any(|l| l == "No token saved."));

    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).credential_inputs.jira.clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input(TOKEN, cx);
    })
    .unwrap();
    click(cx, h, "save-jira-token");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["credentials"]["jira"]["source"] == "stored"
    });
    cx.update_window(h, |_, _, cx| {
        let view = v.read(cx);
        assert_eq!(view.credential_inputs.jira.read(cx).value(), "");
        assert!(view.notice.starts_with("Token saved encrypted"));
        assert!(!view.workspace.to_string().contains(TOKEN));
    })
    .unwrap();
    let text = screen_text(cx, h);
    assert!(!text.iter().any(|l| l.contains(TOKEN)));
    assert!(
        text.iter()
            .any(|l| l == "Saved token (encrypted with this install's key).")
    );
    let raw = std::fs::read_to_string(data.join("credentials.json")).unwrap();
    assert!(raw.contains("$crewroom$v1$") && !raw.contains(TOKEN));

    // Losing the install key: the row reads unreadable and offers Reconnect.
    std::fs::remove_file(data.join("crewroom.secret")).unwrap();
    let workspace = f.get("/workspace");
    assert_eq!(workspace["credentials"]["jira"]["source"], "unreadable");
    cx.update_window(h, |_, _, cx| {
        v.update(cx, |view, cx| {
            view.workspace["credentials"] = workspace["credentials"].clone();
            cx.notify();
        })
    })
    .unwrap();
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "Credentials unreadable — reconnect"),
        "{text:?}"
    );
    assert!(text.iter().any(|l| l == "Reconnect"));
    cx.update_window(h, |_, window, cx| {
        let input = v.read(cx).credential_inputs.jira.clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input(TOKEN, cx);
    })
    .unwrap();
    click(cx, h, "save-jira-token");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["credentials"]["jira"]["source"] == "stored"
    });
    assert!(data.join("crewroom.secret").exists());

    click(cx, h, "clear-jira-token");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["credentials"]["jira"]["source"] == "none"
    });
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}

const GITHUB_TOKEN: &str = "ghp_synthetic0desktop0token";

/// A minimal simulated GitHub API on loopback: `/user` accepts one synthetic
/// token, and only `fixture/visible` is a visible repository.
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
                    == format!(
                        "authorization: bearer {}",
                        GITHUB_TOKEN.to_ascii_lowercase()
                    )
                {
                    authorized = true;
                }
            }
            let path = request.split_whitespace().nth(1).unwrap_or("");
            let (status, body) = match (authorized, path) {
                (false, _) => ("401 Unauthorized", r#"{"message":"Bad credentials"}"#),
                (true, "/user") => (
                    "200 OK",
                    r#"{"login":"octo-fixture","id":1,"name":"Fixture Operator"}"#,
                ),
                (true, "/repos/fixture/visible") => (
                    "200 OK",
                    r#"{"full_name":"fixture/visible","private":true,"permissions":{"push":true,"pull":true}}"#,
                ),
                _ => ("404 Not Found", r#"{"message":"Not Found"}"#),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nX-OAuth-Scopes: repo\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    origin
}

#[gpui::test]
fn native_settings_connect_github_with_a_validated_token_and_disconnect(
    cx: &mut gpui::TestAppContext,
) {
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
    let data = f.root.path().join("data");
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["github"]["state"] == "disconnected"
    });
    navigate(cx, h, "import-jira");
    let text = screen_text(cx, h);
    assert!(text.iter().any(|l| l == "GitHub"), "{text:?}");
    assert!(
        text.iter()
            .any(|l| l == "Disconnected. Paste a personal access token to connect GitHub.")
    );
    let type_into = |cx: &mut gpui::TestAppContext, repository: bool, value: &str| {
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            let inputs = &v.read(cx).credential_inputs;
            let input = if repository {
                inputs.github_repository.clone()
            } else {
                inputs.github.clone()
            };
            input.update(cx, |i, cx| i.focus(window, cx));
            window.input(value, cx);
        })
        .unwrap();
    };

    // A token GitHub rejects: the reason is shown and nothing is stored.
    type_into(cx, false, "ghp_synthetic0rejected");
    click(cx, h, "connect-github");
    wait(cx, h, &v, &e, |v, _| !v.busy && !v.error.is_empty());
    cx.update_window(h, |_, _, cx| {
        let view = v.read(cx);
        assert!(view.error.starts_with("Token not saved."), "{}", view.error);
        assert!(view.error.contains("invalid, expired or revoked"));
        assert_eq!(view.credential_inputs.github.read(cx).value(), "");
    })
    .unwrap();
    assert!(!data.join("credentials.json").exists());

    // A valid token: connected with the account, never shown again.
    type_into(cx, false, GITHUB_TOKEN);
    click(cx, h, "connect-github");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["github"]["state"] == "connected"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l == "Connected as @octo-fixture."),
        "{text:?}"
    );
    assert!(
        text.iter().any(|l| {
            l.starts_with("Account: @octo-fixture (Fixture Operator) · classic token")
        })
    );
    assert!(!text.iter().any(|l| l.contains(GITHUB_TOKEN)));
    cx.update_window(h, |_, _, cx| {
        let view = v.read(cx);
        assert!(view.notice.starts_with("GitHub accepted the token"));
        assert!(!view.workspace.to_string().contains(GITHUB_TOKEN));
    })
    .unwrap();
    let raw = std::fs::read_to_string(data.join("credentials.json")).unwrap();
    assert!(raw.contains("$crewroom$v1$") && !raw.contains(GITHUB_TOKEN));

    // No access to a repository is stated, not hidden.
    type_into(cx, true, "fixture/hidden");
    click(cx, h, "check-github-repository");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["github"]["repository"]["access"] == "none"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l.starts_with("No access to fixture/hidden")),
        "{text:?}"
    );

    click(cx, h, "disconnect-github");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["github"]["state"] == "disconnected"
    });
    let raw = std::fs::read_to_string(data.join("credentials.json")).unwrap();
    assert!(!raw.contains("github"));
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}
