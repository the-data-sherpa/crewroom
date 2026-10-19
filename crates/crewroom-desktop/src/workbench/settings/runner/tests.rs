//! Settings → Runner against the real local service (embedded, so it cannot
//! reload itself). Simulated: no Mail server or agent runs.
use super::*;
use crate::workbench::test_support::*;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;
use std::os::unix::fs::PermissionsExt;

const MAIL: &str = "synthetic-desktop-mail-token";
const KEY: &str = "synthetic-desktop-provider-key";

fn type_input(
    cx: &mut gpui::TestAppContext,
    handle: gpui::AnyWindowHandle,
    view: &Entity<Workbench>,
    pick: impl Fn(&RunnerForm) -> Entity<InputState>,
    text: &str,
) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let input = pick(view.read(cx).settings.runner.form.as_ref().unwrap());
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input(text, cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui::test]
fn runner_page_lists_prerequisites_and_saves_the_configuration_privately(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let root = std::fs::canonicalize(fixture.root.path()).unwrap();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "runner");
    wait(
        cx,
        h,
        &v,
        &e,
        |v, _| matches!(&v.settings.runner.view, Some(Ok(view)) if view["source"]["kind"] == "none"),
    );
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l == "Runner not running: no agent can start."),
        "{text:?}"
    );
    for label in [
        "Runtime root · Missing",
        "Rust Agent Mail · Missing",
        "Docker engine · Missing",
        "Execution profiles · Missing",
    ] {
        assert!(
            text.iter().any(|l| l.starts_with(label)),
            "{label}: {text:?}"
        );
    }
    assert!(
        text.iter()
            .any(|l| l.starts_with("Fix: Enter an absolute private folder"))
    );
    assert!(
        text.iter()
            .any(|l| l.starts_with("Codex harness · Missing") && l.ends_with("optional"))
    );

    for (key, value) in [
        ("root", root.join("hosts").display().to_string()),
        ("mailUrl", "http://127.0.0.1:9/mcp/".into()),
        ("mailProjectRoot", root.display().to_string()),
    ] {
        type_input(cx, h, &v, |f| f.fields.inputs[key].clone(), &value);
    }
    type_input(cx, h, &v, |f| f.mail.clone(), MAIL);
    type_input(cx, h, &v, |f| f.provider.clone(), "fixture");
    type_input(cx, h, &v, |f| f.provider_key.clone(), KEY);
    cx.update_window(h, |_, window, cx| {
        let area = v.read(cx).settings.runner.form.as_ref().unwrap().advanced.clone();
        area.update(cx, |a, cx| {
            a.set_value(
                r#"{"catalog":[{"provider":"fixture","id":"model","api":"openai-responses","inputs":["text"],"efforts":["low"],"tools":true}],"policy":{},"models":{"providers":{"fixture":{"baseUrl":"http://127.0.0.1:9","api":"openai-responses"}}}}"#,
                window,
                cx,
            )
        });
        window.render_frame(cx);
    })
    .unwrap();
    assert!(cx.read(|cx| v.read(cx).settings.unsaved(cx)));
    click(cx, h, "save-runner");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && matches!(&v.settings.runner.view, Some(Ok(view))
                if view["source"]["kind"] == "settings"
                    && view["secrets"] == json!({"mail":"stored","providers":{"fixture":"stored"}}))
    });
    cx.read(|cx| {
        let view = v.read(cx);
        assert!(
            view.notice.contains("stop and start the service"),
            "{}",
            view.notice
        );
        let form = view.settings.runner.form.as_ref().unwrap();
        assert_eq!(form.mail.read(cx).value(), "");
        assert_eq!(form.provider_key.read(cx).value(), "");
        assert!(!view.settings.unsaved(cx));
    });
    let text = screen_text(cx, h);
    assert!(!text.iter().any(|l| l.contains(MAIL) || l.contains(KEY)));
    assert!(
        text.iter().any(|l| l == "A token is saved encrypted."),
        "{text:?}"
    );
    assert!(
        text.iter().any(|l| l
            .starts_with("Saved changes are not applied yet. This service cannot reload itself.")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|l| l.starts_with("Runtime root · Ready")),
        "{text:?}"
    );
    // Apply is not offered here: activating it sends nothing.
    click(cx, h, "runner-apply");
    let sent = cx.read(|cx| v.read(cx).busy);
    wait(cx, h, &v, &e, |v, _| !v.busy);
    assert!(!sent && cx.read(|cx| v.read(cx).error.is_empty()));
    let file = root.join("data/runtime.json");
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(!written.contains(MAIL) && !written.contains(KEY));
    let stored = std::fs::read_to_string(root.join("data/credentials.json")).unwrap();
    assert!(!stored.contains(MAIL) && !stored.contains(KEY));
}
