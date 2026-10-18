//! Delivery and specialist proposal wording over recorded fixtures. Dispositions are
//! injected into the observed runtime; no message is delivered and no process is started.
use super::acceptance_tests::{coding_plan, contains, labels, select};
use super::test_support::{Fixture, click, navigate};
use super::*;
use crewroom_core::roles::Roles;
use gpui_kit::{self as gpui, AnyWindowHandle};

fn id(v: &Value) -> String {
    string(v, "id")
}
fn scoped(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, scope: &str) -> String {
    labels(cx, h, Some(scope)).join("\n")
}

#[gpui::test]
fn native_peer_delivery_reads_the_same_in_conversation_and_work_packages(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let f = Fixture::projected(|s, root| {
        let (t, _, _) = coding_plan(s, root);
        task = t;
    });
    let (h, v, e) = super::test_support::open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Conversation");
    for (disposition, wording) in [
        ("accepted", "Accepted · Not yet dispatched"),
        ("uncertain", "Delivery uncertain · Reconciliation needed"),
    ] {
        v.update(cx, |v, cx| {
            v.observation.runtime["choreography"] = json!({"revision":1,"binding":{"planId":"accepted-plan","requirementRevision":1},"schedule":{"assignments":[]},"packages":[{"id":"backend","state":"blocked","reason":"Awaiting navigator concern"}],"reports":[],"exchanges":[{"id":"concern","packageId":"backend","kind":"challenge","messageId":"mail","artifact":"pagination.js","body":"A zero cursor must not restart pagination.","proposedChange":"Add a regression test.","source":{"head":"abcdef12345"}}]});
            v.observation.runtime["messages"] = json!([{"id":"mail","disposition":disposition,"body":"A zero cursor must not restart pagination."}]);
            cx.notify();
        });
        let peer = scoped(cx, h, "peer-message-mail");
        let package = scoped(cx, h, "package-exchange-concern");
        assert!(peer.contains(wording), "{disposition}: {peer}");
        assert!(package.contains(wording), "{disposition}: {package}");
        for view in [&peer, &package] {
            assert!(!view.contains("Delivery state unavailable"));
            assert!(!view.contains(&format!("Delivery: {disposition}")));
        }
    }
}

#[gpui::test]
fn native_specialist_proposals_never_promise_a_start_beside_a_rejection(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let f = Fixture::projected(|s, root| {
        let (t, run, _) = coding_plan(s, root);
        task = t;
        let run = id(&run);
        s.claim_coding_slot(&run, true).unwrap();
        let roles = Roles::new(root.join("roles.json")).read().unwrap();
        let role = |id: &str| roles.iter().find(|r| r["id"] == id).unwrap().clone();
        let model = json!({"requested":{"model":"fixture/model","effort":"low"},"resolved":{"selector":"fixture/model","effort":"low"}});
        s.admit_assignment(&run, &json!({"name":"Coordinator","coordinator":true,"role":role("coordinator"),"model":model,"input":"Recorded fixture"})).unwrap();
        let propose = |input: &str| {
            s.propose_operator_specialist(&run, &json!({"requestId":uuid::Uuid::new_v4().to_string(),"roleId":"tech-writer","input":input,"dependsOn":[],"criteria":[]}), &role("tech-writer"), &model).unwrap()
        };
        let rejected = propose("Document the change");
        s.reject_team_proposal(&id(&rejected), "Outside the accepted scope.")
            .unwrap();
        propose("Explain the result");
    });
    let (h, v, e) = super::test_support::open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Conversation");
    super::test_support::wait(cx, h, &v, &e, |v, _| {
        records(&v.observation.runtime, "teamProposals").len() == 2
    });
    // Runner availability is asserted in memory so the pending-proposal reason is reachable.
    v.update(cx, |v, cx| {
        v.workspace["capabilities"]["execution"] = json!(true);
        cx.notify();
    });
    assert_eq!(
        labels(cx, h, Some("proposed-note-0")),
        ["No process exists yet. The coordinator decides at the next safe coordination point."]
    );
    click(cx, h, "add-specialist-slot-1");
    let view = labels(cx, h, None);
    contains(&view, "Rejected · not admitted");
    contains(
        &view,
        "Wait until the coordinator admits or rejects the pending proposal.",
    );
    for label in &view {
        let lower = label.to_lowercase();
        for promise in [
            "starts it",
            "will start",
            "to be applied",
            "receives the request",
        ] {
            assert!(!lower.contains(promise), "{label:?} implies a start");
        }
    }
}
