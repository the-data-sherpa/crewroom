//! Navigation cues over an injected workspace projection: where the operator is, what needs them,
//! and where each entry point leads. No service, provider or agent process is involved.
use super::*;
use gpui_kit::{self as gpui, AnyWindowHandle, test::TestWindowExt};
use gpui_kit::{Bounds, WindowBounds, WindowOptions, point, size};

fn open(
    cx: &mut gpui::TestAppContext,
    width: f32,
    workspace: Value,
) -> (AnyWindowHandle, Entity<Workbench>) {
    cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(width), px(844.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.workspace = workspace;
                    view
                })
            },
        )
        .unwrap()
    })
}

fn workspace(needs: Value) -> Value {
    json!({
        "tasks":[
            {"id":"one","key":"SIM-1","source":"local","title":"First ticket","brief":"","criteria":[],"revision":1,"sourceRevision":1},
            {"id":"two","key":"SIM-2","source":"local","title":"Second ticket","brief":"","criteria":[],"revision":1,"sourceRevision":1}
        ],
        "runs":[],"roles":[],"repositories":[],
        "needsYou":needs
    })
}

fn render(cx: &mut gpui::TestAppContext, h: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui::test]
fn narrow_header_shows_pending_decisions_without_opening_navigation(cx: &mut gpui::TestAppContext) {
    let needs = json!([
        {"id":"feedback-a","kind":"feedback","title":"PR feedback","taskId":"one","runId":"run-a"},
        {"id":"feedback-b","kind":"feedback","title":"PR feedback","taskId":"two","runId":"run-b"}
    ]);
    let (h, view) = open(cx, 390., workspace(needs));
    render(cx, h);
    cx.update_window(h, |_, window, cx| {
        assert!(!view.read(cx).navigation_open);
        let entry = window
            .try_find("needs-you")
            .expect("Needs you must be in the narrow header");
        assert!(entry.visible());
        assert!(entry.bounds().right() <= px(390.));
        assert_eq!(entry.label(), Some("Needs you · 2"));
        window.click("needs-you", cx);
        window.render_frame(cx);
        assert!(view.read(cx).panel == Panel::NeedsYou);
    })
    .unwrap();
}

#[gpui::test]
fn every_panel_has_exactly_one_selected_sidebar_control(cx: &mut gpui::TestAppContext) {
    let (h, view) = open(cx, 1280., workspace(json!([])));
    // GPUI exposes a button's selected state as presentation only, so the rendered controls are
    // checked by ID against the single source each `.selected(...)` reads.
    let cases = [
        (Panel::Ticket, Some("one"), "ticket-one", "First ticket"),
        (Panel::New, None, "new-ticket", "New ticket"),
        (Panel::Queue, None, "queue", "Coding queue"),
        (Panel::NeedsYou, None, "needs-you", "Needs you · 0"),
        (Panel::Roles, None, "settings", "⚙ Settings"),
        (Panel::Import, None, "settings", "⚙ Settings"),
    ];
    for (panel, selected, id, label) in cases {
        view.update(cx, |v, cx| {
            v.panel = panel;
            v.selected = selected.map(str::to_owned);
            cx.notify();
        });
        render(cx, h);
        let current = view.read_with(cx, |v, _| v.sidebar_current());
        assert_eq!(current.as_deref(), Some(id));
        cx.update_window(h, |_, window, _| {
            let control = window.find(SharedString::from(id));
            assert!(
                control
                    .path()
                    .contains(&gpui::ElementId::from(SharedString::from("sidebar")))
            );
            assert_eq!(control.label(), Some(label));
        })
        .unwrap();
    }
    // Before a ticket is chosen, no ticket row claims the selection.
    view.update(cx, |v, cx| {
        v.panel = Panel::Ticket;
        v.selected = None;
        cx.notify();
    });
    assert_eq!(view.read_with(cx, |v, _| v.sidebar_current()), None);
}

#[gpui::test]
fn queue_entry_matches_the_queue_page_title(cx: &mut gpui::TestAppContext) {
    let (h, view) = open(cx, 1280., workspace(json!([])));
    render(cx, h);
    cx.update_window(h, |_, window, cx| {
        assert_eq!(window.find("queue").label(), Some("Coding queue"));
        window.click("queue", cx);
        window.render_frame(cx);
        assert!(view.read(cx).panel == Panel::Queue);
    })
    .unwrap();
}

#[gpui::test]
fn pr_feedback_card_opens_the_results_tab(cx: &mut gpui::TestAppContext) {
    let needs = json!([
        {"id":"feedback-a","kind":"feedback","title":"PR feedback","taskId":"one","runId":"run-a"}
    ]);
    let (h, view) = open(cx, 1280., workspace(needs));
    view.update(cx, |v, cx| {
        v.panel = Panel::NeedsYou;
        cx.notify();
    });
    render(cx, h);
    cx.update_window(h, |_, window, cx| {
        let open = window.find("needs-open-feedback-a");
        assert_eq!(open.label(), Some("Open PR comments"));
        window.click("needs-open-feedback-a", cx);
        window.render_frame(cx);
        let v = view.read(cx);
        assert!(v.panel == Panel::Ticket && v.selected.as_deref() == Some("one"));
        assert_eq!(v.tab, "Results");
    })
    .unwrap();
}

#[gpui::test]
fn empty_workspace_reaches_jira_intake_in_one_activation(cx: &mut gpui::TestAppContext) {
    let (h, view) = open(
        cx,
        1280.,
        json!({"tasks":[],"runs":[],"roles":[],"repositories":[],"needsYou":[]}),
    );
    render(cx, h);
    cx.update_window(h, |_, window, cx| {
        assert!(window.try_find("empty-new").is_some());
        assert_eq!(
            window.find("empty-import").label(),
            Some("Import from Jira")
        );
        window.click("empty-import", cx);
        window.render_frame(cx);
        assert!(view.read(cx).panel == Panel::Import);
        assert!(window.try_find("save-import").is_some());
    })
    .unwrap();
}
