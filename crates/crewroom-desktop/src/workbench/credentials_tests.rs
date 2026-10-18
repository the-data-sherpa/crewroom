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
