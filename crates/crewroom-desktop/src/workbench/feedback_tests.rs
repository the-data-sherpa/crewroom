//! Feedback stays with the panel, ticket and control whose action produced it.
use super::test_support::{Fixture, click, navigate, open, wait};
use super::*;
use gpui::test::TestWindowExt;
use gpui::{AnyWindowHandle, Bounds, WindowBounds, WindowOptions, point, size};
use gpui_kit as gpui;

fn offline(
    cx: &mut gpui::TestAppContext,
    workspace: Value,
) -> (AnyWindowHandle, Entity<Workbench>) {
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(1000.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.tab = "Conversation";
                    view.workspace = workspace;
                    view
                })
            },
        )
        .unwrap()
    });
    (handle.into(), view)
}
fn task(id: &str) -> Value {
    json!({"id":id,"key":id.to_uppercase(),"source":"local","title":format!("Ticket {id}"),"brief":"","criteria":[],"revision":1,"sourceRevision":1})
}
/// Records a command as the production send path does, then applies the service result.
fn finish(
    cx: &mut gpui::TestAppContext,
    view: &Entity<Workbench>,
    path: &str,
    body: Value,
    result: Result<Value, String>,
) {
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.started(path.to_owned(), &body);
            v.accept(
                Event::Result {
                    path: path.to_owned(),
                    result,
                },
                cx,
            );
        })
    });
}
fn present(cx: &mut gpui::TestAppContext, handle: AnyWindowHandle, id: &str) -> bool {
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(SharedString::from(id.to_owned())).is_some()
    })
    .unwrap()
}
fn bounds(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    id: &str,
) -> Bounds<gpui::Pixels> {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find(SharedString::from(id.to_owned())).bounds()
    })
    .unwrap()
}

#[gpui::test]
fn native_errors_stay_with_their_ticket_and_panel(cx: &mut gpui::TestAppContext) {
    let (h, v) = offline(
        cx,
        json!({"tasks":[task("a"),task("b")],"runs":[],"roles":[],"repositories":[]}),
    );
    click(cx, h, "ticket-a");
    finish(
        cx,
        &v,
        "/tasks/a/refinement",
        json!({}),
        Err("The brief changed elsewhere.".into()),
    );
    assert!(present(cx, h, "feedback-error-top"));
    click(cx, h, "ticket-b");
    assert!(
        !present(cx, h, "feedback-error-top"),
        "Ticket A's error must not appear on ticket B."
    );
    click(cx, h, "settings");
    assert!(!present(cx, h, "feedback-error-top"));
    click(cx, h, "ticket-a");
    assert!(present(cx, h, "feedback-error-top"));
    // Connection errors describe the service, not one ticket.
    cx.update(|cx| {
        v.update(cx, |v, cx| {
            v.accept(Event::Error("Service unavailable.".into()), cx)
        })
    });
    click(cx, h, "ticket-b");
    assert!(present(cx, h, "error-connection"));
    assert!(!present(cx, h, "feedback-error-top"));
}

#[gpui::test]
fn native_action_notices_name_their_outcome(cx: &mut gpui::TestAppContext) {
    let (_h, v) = offline(
        cx,
        json!({"tasks":[task("a")],"runs":[],"roles":[],"repositories":[],
            "intake":{"a":{"intake":{"id":"intake-1"},"plans":[{"id":"plan-1","version":2}]},
                "b":{"intake":null,"plans":[{"id":"plan-3","version":1}]}}}),
    );
    for (path, value, expected) in [
        (
            "/runs/r/publication/reconcile",
            json!({}),
            "Publication reconciliation finished. Check the push and PR receipts.",
        ),
        (
            "/jira/reconcile",
            json!({}),
            "Jira reconciliation finished. Only confirmed updates have receipts.",
        ),
        (
            "/principles/p/delete",
            json!({}),
            "Principle deleted. Existing assignments keep their copied text.",
        ),
        (
            "/implementation-plans/plan-1/decision",
            json!({"planId":"plan-1","decision":"accept"}),
            "Plan 2 accepted.",
        ),
        (
            "/implementation-plans/plan-1/decision",
            json!({"planId":"plan-1","decision":"reject"}),
            "Plan 2 rejected. A revision was requested.",
        ),
        (
            "/implementation-plans/plan-3/decision",
            json!({"planId":"plan-3","decision":"reject"}),
            "Plan 1 rejected. No agent will revise it.",
        ),
    ] {
        finish(cx, &v, path, json!({}), Ok(value));
        let notice = cx.read(|cx| v.read(cx).notice.clone());
        assert_eq!(notice, expected, "{path}");
        assert_ne!(notice, "Saved on this machine.");
    }
}

#[gpui::test]
fn native_pending_text_names_the_command(cx: &mut gpui::TestAppContext) {
    let (_h, v) = offline(
        cx,
        json!({"tasks":[],"runs":[],"roles":[],"repositories":[]}),
    );
    for (path, body, expected) in [
        ("/repositories/r/retire", json!({}), "Retiring…"),
        ("/principles/p/delete", json!({"revision":1}), "Deleting…"),
        (
            "/learning/l/decision",
            json!({"decision":"approve","reason":"Fits"}),
            "Recording decision…",
        ),
        (
            "/learning/l/decision",
            json!({"decision":"reject","reason":"Wrong"}),
            "Recording decision…",
        ),
        (
            "/implementation-plans/p/decision",
            json!({"decision":"accept"}),
            "Recording decision…",
        ),
        (
            "/comparisons/c/control",
            json!({"action":"cancel"}),
            "Cancelling remaining cells…",
        ),
        (
            "/runs/r/publication/reconcile",
            json!({}),
            "Reconciling publication…",
        ),
    ] {
        let notice = cx.update(|cx| {
            v.update(cx, |v, _| {
                v.started(path.into(), &body);
                let notice = v.notice.clone();
                v.pending_commands.clear();
                notice
            })
        });
        assert_eq!(notice, expected, "{path}");
    }
}

#[gpui::test]
fn native_steering_conflict_renders_beside_the_composer_with_the_draft(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let fixture = Fixture::seeded(false, |s, _| {
        task = s
            .create_task(&json!({"title":"Steering conflict","description":""}))
            .unwrap();
        run = s
            .queue_refinement(
                &string(&task, "id"),
                &json!({"revision":1,"purpose":"explore"}),
            )
            .unwrap();
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| v.connected);
    click(
        cx,
        h,
        SharedString::from(format!("ticket-{}", string(&task, "id"))),
    );
    navigate(cx, h, "Conversation");
    let run_id = string(&run, "id");
    wait(cx, h, &v, &events, |v, _| v.steering.contains_key(&run_id));
    cx.update_window(h, |_, window, cx| {
        let input = v.read(cx).steering[&run_id].clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        window.input("Keep this direction.", cx);
    })
    .unwrap();
    finish(
        cx,
        &v,
        &format!("/runs/{run_id}/steering"),
        json!({"body":"Keep this direction."}),
        Err("409 Conflict: the run changed; this instruction was not saved.".into()),
    );
    assert!(present(cx, h, "feedback-error-composer"));
    assert!(!present(cx, h, "feedback-error-top"));
    let error = bounds(cx, h, "feedback-error-composer");
    let send = bounds(cx, h, "save-steering");
    assert!(
        (send.origin.y - error.bottom()).abs() < px(240.),
        "The error must render beside the composer: {error:?} / {send:?}"
    );
    cx.read(|cx| {
        assert_eq!(
            v.read(cx).steering[&run_id].read(cx).value(),
            "Keep this direction."
        )
    });
}

#[gpui::test]
fn native_brief_save_error_renders_inside_the_wide_drawer(cx: &mut gpui::TestAppContext) {
    let mut task = Value::Null;
    let fixture = Fixture::seeded(false, |s, _| {
        task = s
            .create_task(&json!({"title":"Drawer feedback","description":""}))
            .unwrap();
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| v.connected);
    let id = string(&task, "id");
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    navigate(cx, h, "Brief");
    finish(
        cx,
        &v,
        &format!("/tasks/{id}/refinement"),
        json!({}),
        Err("The accepted brief is now revision 2. Your draft is retained.".into()),
    );
    assert!(present(cx, h, "feedback-error-drawer"));
    assert!(!present(cx, h, "feedback-error-top"));
    // The drawer's tabs mark its left edge; the session column lies to their left.
    let tab = bounds(cx, h, "Brief");
    let session = bounds(cx, h, "toggle-details");
    let error = bounds(cx, h, "feedback-error-drawer");
    assert!(
        error.origin.x >= tab.origin.x - px(1.) && error.origin.x > session.right(),
        "{error:?} / {tab:?} / {session:?}"
    );
}
