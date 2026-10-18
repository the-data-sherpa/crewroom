//! Truthful PR readiness and named revisions (FOR-25: H3, M7, M21, L18).
use super::publication::delivery_headline;
use super::*;
use gpui::test::TestWindowExt;
use gpui::{AnyWindowHandle, Bounds, WindowBounds, WindowOptions, point, size};
use gpui_kit as gpui;

const OLD: &str = "aaaaaaaaaaaa1111111111111111111111111111";
const NEW: &str = "bbbbbbbbbbbb2222222222222222222222222222";

fn record(push: &str, pr: &str) -> Value {
    json!({"id":"publication","ready":false,"push":{"state":push},"pr":{"state":pr},"revision":{"head":NEW},"blockers":[]})
}

#[test]
fn each_push_and_pr_failure_has_its_own_plain_headline() {
    let expected = [
        (
            ("blocked", "pending"),
            "Push blocked · the remote branch or its PR changed",
        ),
        (
            ("uncertain", "pending"),
            "Push outcome unknown · reconcile to confirm",
        ),
        (
            ("confirmed", "blocked"),
            "PR blocked · an open PR exists or the provider refused it",
        ),
        (
            ("confirmed", "uncertain"),
            "PR outcome unknown · reconcile to confirm",
        ),
        (("pending", "pending"), "Publishing · push receipt pending"),
        (("sending", "pending"), "Publishing · push receipt pending"),
        (("confirmed", "pending"), "Publishing · PR receipt pending"),
        (("confirmed", "sending"), "Publishing · PR receipt pending"),
        (
            ("confirmed", "confirmed"),
            "PR created · not yet ready for review",
        ),
    ];
    for ((push, pr), headline) in expected {
        assert_eq!(
            delivery_headline(&record(push, pr), true),
            headline,
            "{push}/{pr}"
        );
    }
    // Distinct outcomes never share a headline.
    let mut categories: Vec<_> = expected.iter().map(|(_, h)| *h).collect();
    categories.dedup();
    let unique: std::collections::HashSet<_> = categories.iter().collect();
    assert_eq!(unique.len(), categories.len());
    let states = ["pending", "sending", "uncertain", "confirmed", "blocked"];
    for push in states {
        for pr in states {
            let headline = delivery_headline(&record(push, pr), true);
            assert_ne!(headline, "Validation pending or blocked", "{push}/{pr}");
            assert_ne!(headline, "Ready for human review", "{push}/{pr}");
        }
    }
}

#[test]
fn stale_observation_never_states_current_ready_or_merged() {
    let mut ready = record("confirmed", "confirmed");
    ready["ready"] = json!(true);
    assert_eq!(delivery_headline(&ready, true), "Ready for human review");
    assert_eq!(
        delivery_headline(&ready, false),
        "Last observed ready for human review · observation not current. Reconnect before relying on this."
    );
    ready["merged"] = json!({"commit":"c"});
    assert_eq!(delivery_headline(&ready, true), "Merged");
    assert_eq!(
        delivery_headline(&ready, false),
        "Last observed merged · observation not current. Reconnect before relying on this."
    );
    assert_eq!(
        delivery_headline(&record("uncertain", "pending"), false),
        "Last observed: Push outcome unknown · reconcile to confirm · observation not current"
    );
}

#[test]
fn revision_labels_use_one_short_form() {
    let candidates = vec![
        json!({"id":"c1","revision":{"head":OLD}}),
        json!({"id":"c2","revision":{"head":NEW}}),
    ];
    assert_eq!(
        revision_label(&candidates, &json!(OLD)),
        "Revision 1 · aaaaaaaaaaaa"
    );
    assert_eq!(
        revision_label(&candidates, &json!(NEW)),
        "Revision 2 · bbbbbbbbbbbb"
    );
    assert_eq!(
        revision_label(&candidates, &json!("cccccccccccc3333")),
        "Unlisted revision · cccccccccccc"
    );
}

fn result(published: &str, ready: bool) -> Value {
    let c1 = json!({"id":"c1","revision":{"head":OLD}});
    let c2 = json!({"id":"c2","revision":{"head":NEW}});
    json!({
        "candidates":[c1,c2.clone()],
        "assessment":{"ready":true,"candidate":c2,"blockers":[]},
        "publication":{"available":true,"record":{"id":"publication","ready":ready,"push":{"state":"confirmed"},"pr":{"state":"confirmed"},"revision":{"head":published},"blockers":[]}}
    })
}

fn open(
    cx: &mut gpui::TestAppContext,
    setup: impl FnOnce(&mut Workbench) + 'static,
) -> AnyWindowHandle {
    cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(2400.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.selected = Some("fixture".into());
                    view.tab = "Results";
                    view.workspace = json!({"tasks":[{"id":"fixture","key":"SIM-1","source":"local","title":"Native fixture","brief":"","criteria":[],"revision":1,"sourceRevision":1}],"runs":[{"id":"run-1","taskId":"fixture","purpose":"coding","status":"completed"}],"roles":[],"repositories":[]});
                    view.observe_run(Some("run-1".into()));
                    setup(&mut view);
                    view
                })
            },
        )
        .unwrap()
        .0
    })
}

fn labels(cx: &mut gpui::TestAppContext, h: AnyWindowHandle) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}

fn has(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|l| l.contains(expected))
}

#[gpui::test]
fn disconnected_observation_with_ready_record_renders_no_ready_verdict(
    cx: &mut gpui::TestAppContext,
) {
    let h = open(cx, |view| {
        view.observation.result = result(NEW, true);
        view.observation.result_fresh = true;
        view.observation.disconnected();
    });
    let shown = labels(cx, h);
    assert!(
        !has(&shown, "Ready for human review"),
        "Stale record must not read as current: {shown:#?}"
    );
    assert!(
        has(&shown, "Last observed ready for human review"),
        "{shown:#?}"
    );
    assert!(has(&shown, "Reconnect before relying on this."));
}

#[gpui::test]
fn facts_results_and_pr_delivery_name_the_same_short_revision(cx: &mut gpui::TestAppContext) {
    let h = open(cx, |view| {
        view.observation.result = result(OLD, true);
        view.observation.result_fresh = true;
    });
    let shown = labels(cx, h);
    assert!(
        has(
            &shown,
            "Checks and independent review cover Revision 2 · bbbbbbbbbbbb"
        ),
        "Facts bar: {shown:#?}"
    );
    assert!(
        has(&shown, "Locally validated · Revision 2 · bbbbbbbbbbbb"),
        "{shown:#?}"
    );
    assert!(
        has(&shown, "Published: Revision 1 · aaaaaaaaaaaa"),
        "{shown:#?}"
    );
    assert!(has(&shown, "Not the latest revision"), "{shown:#?}");
    // Candidate detail may print the full commit; the summaries use one short form.
    assert!(
        !shown.iter().any(|l| (l.starts_with("Published")
            || l.starts_with("Locally validated")
            || l.starts_with("Checks and independent review"))
            && (l.contains(OLD) || l.contains(NEW))),
        "Full heads are not a second summary format: {shown:#?}"
    );

    let h = open(cx, |view| {
        view.observation.result = result(NEW, true);
        view.observation.result_fresh = true;
    });
    let shown = labels(cx, h);
    assert!(
        has(&shown, "Published: Revision 2 · bbbbbbbbbbbb"),
        "{shown:#?}"
    );
    assert!(!has(&shown, "Not the latest revision"), "{shown:#?}");
}

#[gpui::test]
fn waiting_and_error_hints_are_never_shown_together(cx: &mut gpui::TestAppContext) {
    let h = open(cx, |view| {
        view.observation
            .errors
            .insert("result".into(), "Result store unreadable".into());
    });
    let shown = labels(cx, h);
    assert!(
        has(&shown, "Evidence unavailable: Result store unreadable"),
        "{shown:#?}"
    );
    assert!(!has(&shown, "Waiting for"), "{shown:#?}");

    let h = open(cx, |_| {});
    let shown = labels(cx, h);
    assert!(has(&shown, "Waiting for saved evidence…"), "{shown:#?}");
    assert!(!has(&shown, "Evidence unavailable"), "{shown:#?}");
}
