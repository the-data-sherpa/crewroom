//! Keyboard routes past the ticket list, and saved evidence opened where the operator looks.
use super::acceptance_tests::keyboard_activate;
use super::test_support::{Fixture, navigate_by_keyboard, open, wait};
use super::*;
use gpui::{AnyWindowHandle, Focusable, test::TestWindowExt};
use gpui_kit as gpui;
fn id(v: &Value) -> String {
    string(v, "id")
}
fn focused(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, target: &str) -> Option<bool> {
    cx.run_until_parked();
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.find(SharedString::from(target.to_owned())).focused()
    })
    .unwrap()
}
fn press(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, key: &str) {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui::test]
fn keyboard_ticket_selection_moves_focus_into_the_session(cx: &mut gpui::TestAppContext) {
    let mut tasks = vec![];
    let fixture = Fixture::seeded(false, |s, _| {
        for title in ["First keyboard ticket", "Second keyboard ticket", "Third"] {
            tasks.push(
                s.create_task(&json!({"title":title,"description":""}))
                    .unwrap(),
            );
        }
    });
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && records(&v.workspace, "tasks").len() == 3
    });
    keyboard_activate(cx, h, &format!("ticket-{}", id(&tasks[1])));
    press(cx, h, "tab");
    assert_eq!(
        focused(cx, h, "workspace"),
        Some(true),
        "One Tab after selecting a ticket reaches the session, not the next ticket row"
    );
    assert_eq!(focused(cx, h, "ticket-list"), Some(false));

    press(cx, h, "secondary-1");
    assert_eq!(focused(cx, h, "ticket-list"), Some(true));
    press(cx, h, "secondary-2");
    assert_eq!(focused(cx, h, "workspace"), Some(true));
    press(cx, h, "secondary-1");
    press(cx, h, "secondary-3");
    assert!(cx.read(|cx| v.read(cx).panel == Panel::NeedsYou));
    assert_eq!(focused(cx, h, "workspace"), Some(true));
}

#[gpui::test]
fn view_change_focuses_saved_evidence_and_close_returns_to_the_button(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let fixture = Fixture::projected(|s, root| {
        let (t, r, _) = super::lifecycle_tests::paused(s, root);
        task = t;
        run = r;
        s.update_coding_record(&id(&run), |_| {
            Ok(json!({"runId":run["id"],"requiredChecks":[],"candidates":[{"id":"only","revision":{"head":"only-head","base":"base","tree":"only-tree"}}],"checks":[],"reviews":[],"checkPolicyChanges":[],"checkOperations":[]}))
        })
        .unwrap();
    });
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    keyboard_activate(cx, h, &format!("ticket-{}", id(&task)));
    wait(cx, h, &v, &e, |v, _| {
        v.observation.runtime_fresh && v.observation.result_fresh
    });
    navigate_by_keyboard(cx, h, "Results");
    keyboard_activate(cx, h, "view-candidate-diff");
    // The fixture candidate has no Git objects, so its diff reply is replaced by a recorded
    // patch. Other service events pass through the production reducer unchanged.
    let path = cx.read(|cx| v.read(cx).observation.detail.as_ref().unwrap().path.clone());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let generation = loop {
        assert!(std::time::Instant::now() < until, "No diff reply");
        let Ok(event) = e.try_recv() else {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        };
        match event {
            Event::Read {
                generation,
                path: read,
                ..
            } if read == path => break generation,
            event => cx.update(|cx| v.update(cx, |v, cx| v.accept(event, cx))),
        }
    };
    cx.update(|cx| {
        v.update(cx, |v, cx| {
            v.accept(
                Event::Read {
                    generation,
                    path,
                    result: Ok(json!({"patch":"+Saved change under review\n"})),
                },
                cx,
            );
        })
    });
    cx.run_until_parked();
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        // Tests have no platform frame loop; deliver the scroll requested by the opening.
        window.simulate_next_frame(cx);
        window.render_frame(cx);
        let content = v
            .read(cx)
            .observation
            .detail
            .as_ref()
            .unwrap()
            .content
            .clone();
        let content = content.expect("saved content is rendered");
        assert!(
            content.read(cx).focus_handle(cx).is_focused(window),
            "View change leaves focus on the saved evidence content"
        );
        let shown = window.find("close-evidence").bounds();
        assert!(
            shown.origin.y >= px(0.) && shown.bottom() <= window.viewport_size().height,
            "Saved evidence is scrolled into view: {shown:?}"
        );
    })
    .unwrap();
    keyboard_activate(cx, h, "close-evidence");
    assert!(cx.read(|cx| v.read(cx).observation.detail.is_none()));
    assert_eq!(
        focused(cx, h, "view-candidate-diff"),
        Some(true),
        "Close returns focus to the button that opened the evidence"
    );
}
