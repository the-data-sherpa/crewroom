use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};

fn setup() -> (tempfile::TempDir, Store, String, Vec<Value>) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Team change","description":""}))
        .unwrap();
    store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Inspect evidence","criteria":["Explain findings"]})).unwrap();
    let run = store
        .queue_research(
            id(&task),
            &json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    (root, store, id(&run).to_owned(), roles)
}
fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn writer(roles: &[Value]) -> &Value {
    roles.iter().find(|r| r["id"] == "tech-writer").unwrap()
}
fn request() -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"roleId":"tech-writer","input":"Explain findings","criteria":[0]})
}

#[test]
fn invalidated_admission_rejects_durably_and_allows_a_fresh_proposal() {
    let (root, store, run, roles) = setup();
    let input = request();
    let proposal = store
        .propose_operator_specialist(&run, &input, writer(&roles), &json!({}))
        .unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute(
        "UPDATE runs SET data=json_set(data,'$.teamTransition',json(?)) WHERE id=?",
        rusqlite::params![json!({"proposalId":id(&proposal)}).to_string(), run],
    )
    .unwrap();
    let rejected = store
        .reject_team_proposal(
            id(&proposal),
            "Model preview changed. Submit a fresh proposal.",
        )
        .unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert!(store.get_run(&run).unwrap()["teamTransition"].is_null());
    assert_eq!(
        store.reject_team_proposal(id(&proposal), "Retry").unwrap(),
        rejected
    );
    assert!(
        store.runtime(&run).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        reopened
            .propose_operator_specialist(&run, &input, writer(&roles), &json!({}))
            .unwrap(),
        rejected
    );
    let replacement = reopened
        .propose_operator_specialist(&run, &request(), writer(&roles), &json!({}))
        .unwrap();
    assert_eq!(replacement["status"], "pending");
    assert_ne!(replacement["id"], proposal["id"]);
    assert_eq!(
        reopened
            .delivery_notices(&run)
            .unwrap()
            .iter()
            .filter(|n| n["payload"]["kind"] == "team_proposal_rejected")
            .count(),
        1
    );
}
#[test]
fn failed_operator_notice_rolls_back_the_proposal_and_retries_after_reopen() {
    let (root, store, run, roles) = setup();
    let input = request();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_notice BEFORE INSERT ON delivery_notices BEGIN SELECT RAISE(ABORT,'fixture interrupted notice'); END;").unwrap();
    assert!(
        store
            .propose_operator_specialist(&run, &input, writer(&roles), &json!({}))
            .is_err()
    );
    assert!(store.team_proposals(&run).unwrap().is_empty());
    assert!(store.delivery_notices(&run).unwrap().is_empty());
    db.execute_batch("DROP TRIGGER fail_notice").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    let proposal = reopened
        .propose_operator_specialist(&run, &input, writer(&roles), &json!({}))
        .unwrap();
    assert_eq!(
        reopened
            .propose_operator_specialist(&run, &input, &Value::Null, &Value::Null)
            .unwrap(),
        proposal
    );
    assert_eq!(reopened.team_proposals(&run).unwrap().len(), 1);
    assert_eq!(reopened.delivery_notices(&run).unwrap().len(), 1);
}
#[test]
fn rejection_and_rewritten_pending_notice_commit_together() {
    let (root, store, run, mut roles) = setup();
    let proposal = store
        .propose_operator_specialist(&run, &request(), writer(&roles), &json!({}))
        .unwrap();
    let before = store.delivery_notices(&run).unwrap();
    roles.iter_mut().find(|r| r["id"] == "tech-writer").unwrap()["enabled"] = json!(false);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_rejection BEFORE INSERT ON delivery_notices WHEN json_extract(NEW.data,'$.payload.kind')='team_proposal_rejected' BEGIN SELECT RAISE(ABORT,'fixture interrupted rejection'); END;").unwrap();
    assert!(
        store
            .reject_unavailable_team_proposal(id(&proposal), &roles)
            .is_err()
    );
    assert_eq!(store.get_team_proposal(id(&proposal)).unwrap(), proposal);
    assert_eq!(store.delivery_notices(&run).unwrap(), before);
    db.execute_batch("DROP TRIGGER fail_rejection").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    let rejected = reopened
        .reject_unavailable_team_proposal(id(&proposal), &roles)
        .unwrap()
        .unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(reopened.delivery_notices(&run).unwrap().len(), 2);
    assert!(
        reopened.delivery_notices(&run).unwrap()[0]["payload"]
            .get("specialist")
            .is_none()
    );
    assert_eq!(
        reopened
            .reject_unavailable_team_proposal(id(&proposal), &roles)
            .unwrap()
            .unwrap(),
        rejected
    );
    assert_eq!(reopened.delivery_notices(&run).unwrap().len(), 2);
}
#[test]
fn retirement_and_all_new_assignments_roll_back_if_one_attempt_write_fails() {
    let (root, store, run, roles) = setup();
    let coordinator_role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let team:Vec<_>=[("Coordinator",true),("Previous",false)].into_iter().map(|(name,coordinator)|json!({"name":name,"role":if coordinator{coordinator_role}else{writer(&roles)},"model":{},"coordinator":coordinator,"input":"Inspect","brief":{"criteria":[],"dependsOn":[]}})).collect();
    let admitted = store.admit_initial_team(&run, &team).unwrap();
    let attempt = id(&admitted[0]["attempt"]);
    store.claim_attempt(attempt).unwrap();
    store.record_attempt_identity(attempt,&json!({"hostPid":101,"ompPid":102,"sessionId":"fixture","hostUrl":"http://127.0.0.1:59999","pane":"%1"})).unwrap();
    store.set_desired_state(&run, "running").unwrap();
    store.start_execution(&run).unwrap();
    let specialists: Vec<_> = ["First", "Second"]
        .into_iter()
        .map(|name| json!({"name":name,"role":writer(&roles),"model":{},"input":"Inspect"}))
        .collect();
    let proposal = store
        .propose_team(
            attempt,
            "rotate",
            &specialists,
            &[json!({"name":"Previous","reason":"Finished"})],
        )
        .unwrap();
    store.begin_pause(&run, None).unwrap();
    store.record_ownership(attempt, true, None).unwrap();
    store.finish_pause(&run).unwrap();
    let before = store.runtime(&run).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_second BEFORE INSERT ON attempts WHEN EXISTS(SELECT 1 FROM assignments WHERE id=NEW.assignment_id AND name='Second') BEGIN SELECT RAISE(ABORT,'fixture interrupted admission'); END;").unwrap();
    assert!(store.apply_team_proposal(id(&proposal), &roles).is_err());
    assert_eq!(store.runtime(&run).unwrap(), before);
    assert_eq!(store.get_team_proposal(id(&proposal)).unwrap(), proposal);
    db.execute_batch("DROP TRIGGER fail_second").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    let applied = reopened.apply_team_proposal(id(&proposal), &roles).unwrap();
    assert_eq!(applied["status"], "applied");
    let runtime = reopened.runtime(&run).unwrap();
    assert_eq!(runtime["assignments"].as_array().unwrap().len(), 4);
    assert_eq!(runtime["assignments"][1]["retirementReason"], "Finished");
    assert_eq!(runtime["attempts"].as_array().unwrap().len(), 4);
    assert_eq!(
        reopened.apply_team_proposal(id(&proposal), &roles).unwrap(),
        applied
    );
}
#[test]
fn competing_operator_requests_reserve_only_one_pending_insertion() {
    let (root, store, run, roles) = setup();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = root.path().join("workflow.sqlite");
            let barrier = barrier.clone();
            let run = run.clone();
            let role = writer(&roles).clone();
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap().with_clock(|| 1000);
                barrier.wait();
                store.propose_operator_specialist(&run, &request(), &role, &json!({}))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(store.team_proposals(&run).unwrap().len(), 1);
    assert_eq!(store.delivery_notices(&run).unwrap().len(), 1);
}
#[test]
fn truncated_operator_obligation_preserves_complete_unicode_and_the_full_instruction() {
    let (_root, store, run, roles) = setup();
    let mut input = request();
    let instruction = format!("{}🙂 more", "x".repeat(1999));
    input["input"] = json!(instruction);
    let proposal = store
        .propose_operator_specialist(&run, &input, writer(&roles), &json!({}))
        .unwrap();
    assert_eq!(proposal["specialists"][0]["input"], instruction);
    assert_eq!(
        proposal["specialists"][0]["brief"]["criteria"][0]["obligation"],
        "x".repeat(1999)
    );
}
