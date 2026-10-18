//! Before the first workspace snapshot the shell shows one waiting state, never
//! empty or unconfigured facts it has not yet read from the service.
use super::*;
use gpui::test::TestWindowExt;
use gpui::{Bounds, WindowBounds, WindowOptions, point, size};
use gpui_kit as gpui;

#[gpui::test]
fn shell_waits_for_the_first_workspace_instead_of_reporting_empty_or_unconfigured(
    cx: &mut gpui::TestAppContext,
) {
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
                    // No service client: the workspace has never been read.
                    let mut view = Workbench::new(None, None, window, cx);
                    view.panel = Panel::NeedsYou;
                    view
                })
            },
        )
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("workspace-waiting").label(),
            Some(WORKSPACE_WAITING)
        );
        assert!(window.try_find("needs-waiting").is_some());
        for absent in ["queue-empty", "needs-unavailable", "needs-empty"] {
            assert!(window.try_find(absent).is_none(), "{absent} rendered");
        }
        let labels: Vec<String> = gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect();
        for phrase in ["execution layer", "not configured", "No queued plans"] {
            assert!(
                !labels.iter().any(|l| l.contains(phrase)),
                "{phrase:?} rendered before the first workspace: {labels:#?}"
            );
        }
        let summary = window
            .find("connection-summary")
            .label()
            .unwrap()
            .to_owned();
        assert!(!summary.contains("not configured"), "{summary}");
        assert!(!summary.contains("execution layer"), "{summary}");
        assert!(
            summary.contains("Waiting for the workflow service"),
            "{summary}"
        );

        // A successful snapshot without an execution layer keeps the accurate
        // unconfigured and empty states.
        view.update(cx, |view, cx| {
            view.accept(
                Event::Workspace(json!({"tasks":[],"runs":[],"capabilities":{}})),
                cx,
            )
        });
        window.render_frame(cx);
        assert!(window.try_find("workspace-waiting").is_none());
        assert!(window.try_find("needs-waiting").is_none());
        assert!(window.try_find("queue-empty").is_some());
        assert!(window.try_find("needs-unavailable").is_some());
        assert!(
            window
                .find("connection-summary")
                .label()
                .unwrap()
                .contains("Jira not configured")
        );

        view.update(cx, |view, cx| {
            view.accept(
                Event::Workspace(
                    json!({"tasks":[],"runs":[],"capabilities":{"execution":true},"needsYou":[]}),
                ),
                cx,
            )
        });
        window.render_frame(cx);
        assert!(window.try_find("needs-empty").is_some());
        assert!(window.try_find("needs-unavailable").is_none());
    })
    .unwrap();
}
