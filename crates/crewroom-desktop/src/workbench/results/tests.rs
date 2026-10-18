//! Truthful PR readiness and named revisions (FOR-25: H3, M7, M21, L18).
use super::publication::{delivery_headline, pr_facts, repository_label};
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

/// FOR-46: the provider's own PR state and observed head sit beside the
/// published revision, for GitHub and Bitbucket alike.
#[test]
fn provider_pr_state_and_observed_head_are_named_beside_the_published_revision() {
    let github = json!({"provider":"github","owner":"octo-fixture","name":"sandbox","id":42,"uuid":"github:42"});
    let mut record = json!({"repository":github,"revision":{"head":NEW},"pr":{"state":"pending"}});
    assert_eq!(
        pr_facts(&record),
        None,
        "no provider facts before a PR receipt"
    );
    record["pr"] = json!({"state":"confirmed","id":5,"providerState":"OPEN"});
    assert_eq!(
        pr_facts(&record).unwrap(),
        "GitHub PR #5 · open · head not yet observed (published bbbbbbbbbbbb)"
    );
    record["observation"] = json!({"pr":{"draft":false,"source":{"commit":{"hash":NEW}},"destination":{"commit":{"hash":OLD}}}});
    assert_eq!(
        pr_facts(&record).unwrap(),
        "GitHub PR #5 · open · observed head bbbbbbbbbbbb on base aaaaaaaaaaaa"
    );
    record["observation"]["pr"]["source"]["commit"]["hash"] = json!(OLD);
    record["pr"]["providerState"] = json!("DECLINED");
    assert_eq!(
        pr_facts(&record).unwrap(),
        "GitHub PR #5 · closed without merging · observed head aaaaaaaaaaaa on base aaaaaaaaaaaa · differs from the published revision bbbbbbbbbbbb"
    );
    assert_eq!(
        repository_label(&github),
        "GitHub octo-fixture/sandbox · repository github:42"
    );
    let bitbucket = json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"});
    record["repository"] = bitbucket.clone();
    record["pr"]["providerState"] = json!("MERGED");
    assert!(
        pr_facts(&record)
            .unwrap()
            .starts_with("Bitbucket PR #5 · merged")
    );
    assert_eq!(
        repository_label(&bitbucket),
        "Bitbucket fixture/repository · repository {00000000-0000-4000-8000-000000000011}"
    );
}

/// FOR-44: usage groups by billing route. API shows a list-price estimate,
/// subscription shows runs and tokens with no dollar figure, unknown and
/// unrecorded routes show no cost, and nothing is totalled across routes.
fn usage_by_route() -> Value {
    let row = |name: &str, billing: &str, recorded: bool, cost: Value, withheld: bool| {
        json!({"attemptId":format!("attempt-{name}"),"assignmentId":format!("assignment-{name}"),"name":name,"generation":1,"model":"claude-code/fixture",
            "awaitingStart":false,"state":"partial","providerRequests":0,"actualResponses":0,"requestCounts":"unavailable",
            "tokens":{"input":40.0,"total":40.0},"unavailableReasons":["Native harness reports cumulative tokens only."],
            "responses":0,"costResponses":0,"costUsd":cost,"billing":billing,"billingRecorded":recorded,"estimateWithheld":withheld})
    };
    let group = |billing: &str, cost: Value, unrecorded: u64, withheld: bool| {
        json!({"billing":billing,"state":"partial","requestCounts":"unavailable","providerRequests":0,"actualResponses":0,
            "responses":0,"costResponses":0,"costUsd":cost,"tokens":{"input":40.0,"total":40.0},"attempts":1,"assignments":1,
            "tokenAttempts":1,"unrecordedAttempts":unrecorded,"awaitingStart":false,"estimateWithheld":withheld,"incompleteAttempts":[]})
    };
    json!({"source":"Fixture","note":"Usage is grouped by billing route and never totalled across routes.","billedUsd":null,
        "routes":[group("api",json!(0.25),0,false),group("subscription",Value::Null,0,true),group("unknown",Value::Null,1,false)],
        "rows":[row("Coordinator","api",true,json!(0.25),false),row("Implementer","subscription",true,Value::Null,true),row("Legacy","unknown",false,Value::Null,false)]})
}

#[gpui::test]
fn usage_is_grouped_by_billing_route_without_a_dollar_figure_for_subscription(
    cx: &mut gpui::TestAppContext,
) {
    let h = open(cx, |view| {
        let mut fixture = result(NEW, false);
        fixture["usage"] = usage_by_route();
        view.observation.result = fixture;
        view.observation.result_fresh = true;
    });
    let shown = labels(cx, h);
    for expected in [
        "Usage by billing route · never totalled across routes",
        "API · estimated cost",
        "Subscription · runs and tokens",
        "Unknown billing route",
        "Estimated cost (runner list price, not provider billing): $0.250000",
        "Subscription use · no dollar figure is shown",
        "it is not a subscription charge and is not shown",
        "Billing route unknown · no cost estimate is shown",
        "Billing route not recorded: assigned before billing routes were detected.",
        "1 attempt(s) were assigned before billing routes were recorded.",
        "Provider request counts unavailable (not zero)",
        "Coordinator · attempt 1 · claude-code/fixture · API",
        "Implementer · attempt 1 · claude-code/fixture · subscription",
        "Legacy · attempt 1 · claude-code/fixture · unknown billing",
    ] {
        assert!(has(&shown, expected), "missing {expected:?}: {shown:#?}");
    }
    // Exactly one dollar figure, shown on the API group and its API row:
    // never a subscription, unknown or cross-route amount.
    let dollars: Vec<_> = shown.iter().filter(|l| l.contains('$')).collect();
    assert_eq!(dollars.len(), 2, "{dollars:#?}");
    assert!(
        dollars.iter().all(|l| l.contains("$0.250000")),
        "{dollars:#?}"
    );
}

#[test]
fn route_headings_name_each_billing_route() {
    use super::usage::route_heading;
    assert_eq!(route_heading(&json!("api")), "API · estimated cost");
    assert_eq!(
        route_heading(&json!("subscription")),
        "Subscription · runs and tokens"
    );
    assert_eq!(route_heading(&json!("unknown")), "Unknown billing route");
    assert_eq!(route_heading(&Value::Null), "Unknown billing route");
}
