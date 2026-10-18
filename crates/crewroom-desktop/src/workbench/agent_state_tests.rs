//! The shared agent state vocabulary across the agents panel, History and the sidebar chip,
//! over recorded fixtures. No agent process is started or claimed.
use super::test_support::{Fixture, click, navigate, open, wait};
use super::*;
use crewroom_core::roles::Roles;
use gpui_kit::{self as gpui, AnyWindowHandle, test::TestWindowExt};

const REASON: &str = "Recorded process exit without a stop request";

fn id(v: &Value) -> String {
    string(v, "id")
}
fn labels(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, scope: Option<&str>) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter(|node| {
                scope.is_none_or(|scope| {
                    node.path()
                        .contains(&gpui::ElementId::from(SharedString::from(scope.to_owned())))
                })
            })
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}
fn contains(labels: &[String], expected: &str) {
    assert!(
        labels.iter().any(|l| l.contains(expected)),
        "Missing {expected:?} in {labels:#?}"
    );
}

#[gpui::test]
fn native_attention_required_reads_needs_attention_with_its_reason_everywhere(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run, mut attempt) = (Value::Null, Value::Null, Value::Null);
    let f = Fixture::projected(|s, root| {
        task = s
            .create_task(&json!({"title":"Agent state vocabulary","description":""}))
            .unwrap();
        run = s
            .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
        let role = Roles::new(root.join("roles.json"))
            .read()
            .unwrap()
            .remove(0);
        let a = s.admit_assignment(&id(&run),&json!({"name":"Reviewer","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Recorded state fixture"})).unwrap();
        attempt = a["attempt"].clone();
        s.claim_attempt(&id(&attempt)).unwrap();
        // Impossible local PIDs; ownership is recorded as uncertain before service startup.
        s.record_attempt_identity(&id(&attempt),&json!({"hostPid":2_000_000_021u32,"ompPid":2_000_000_022u32,"sessionId":"record-only","hostUrl":"http://127.0.0.1:9","pane":"%998"})).unwrap();
        s.record_ownership(&id(&attempt), false, Some(REASON))
            .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(cx, h, SharedString::from(format!("ticket-{}", id(&task))));
    wait(cx, h, &v, &e, |v, _| v.observation.runtime_fresh);
    let attempt = id(&attempt);
    assert_eq!(
        f.get(&format!("/runs/{}/runtime", id(&run)))["attempts"][0]["state"],
        "attention_required"
    );

    // Agents panel: the warning label carries the recorded ownership reason.
    navigate(cx, h, "Conversation");
    contains(
        &labels(cx, h, Some(&format!("agent-state-{attempt}"))),
        &format!("Needs attention: {REASON}"),
    );

    // History: the button and the detail use the same word for the same state.
    navigate(cx, h, "History");
    let button = labels(cx, h, Some(&format!("history-attempt-{attempt}")));
    contains(&button, "· Needs attention");
    click(
        cx,
        h,
        SharedString::from(format!("history-attempt-{attempt}")),
    );
    let view = labels(cx, h, None);
    contains(&view, &format!("State: Needs attention: {REASON}"));
    assert!(
        !view
            .iter()
            .any(|l| l.contains("attention required") || l.contains("Recorded state")),
        "Raw state tokens leak into History: {view:#?}"
    );
}

#[gpui::test]
fn native_sidebar_chip_shows_an_unconfirmed_agent_beside_a_working_one(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let f = Fixture::projected(|s, _| {
        task = s
            .create_task(&json!({"title":"Mixed agent activity","description":""}))
            .unwrap();
        run = s
            .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    // Activity rows as the service projects them: one observed executing agent and one it cannot confirm.
    let run = id(&run);
    v.update(cx, |view, cx| {
        view.workspace["activity"] = json!([
            {"runId":run,"assignmentName":"Coordinator","state":"executing"},
            {"runId":run,"assignmentName":"Reviewer","state":"ownership_unconfirmed"}
        ]);
        cx.notify();
    });
    assert_eq!(
        labels(cx, h, Some(&format!("row-activity-{}", id(&task)))),
        ["1 working · 1 ownership unconfirmed"]
    );
    let run_record = json!({"id":run});
    let (_, detail, tone) = v.read_with(cx, |view, _| view.activity_summary(Some(&run_record)));
    assert_eq!(
        detail,
        "Coordinator: Working · Reviewer: Ownership unconfirmed"
    );
    assert_eq!(tone, crate::agent_state::WARNING);
}
