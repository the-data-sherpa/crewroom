use crewroom_core::store::Store;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

fn setup() -> (tempfile::TempDir, Store, Value, Value) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Attempt recovery","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let role: Value = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let input = json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Inspect retained evidence"});
    (root, store, run, input)
}
fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}

#[test]
fn failed_attempt_insert_rolls_back_assignment_and_allows_reopen_retry() {
    let (root, store, run, input) = setup();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_attempt BEFORE INSERT ON attempts BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
    assert!(store.admit_assignment(id(&run), &input).is_err());
    assert_eq!(store.runtime(id(&run)).unwrap()["assignments"], json!([]));
    assert_eq!(store.get_run(id(&run)).unwrap(), run);
    db.execute_batch("DROP TRIGGER fail_attempt").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let admitted = reopened.admit_assignment(id(&run), &input).unwrap();
    assert_eq!(admitted["attempt"]["state"], "admitted");
    assert_eq!(
        reopened.runtime(id(&run)).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn event_and_observer_write_failures_leave_no_partial_receipts() {
    let (root, store, run, input) = setup();
    let admitted = store.admit_assignment(id(&run), &input).unwrap();
    let attempt_id = id(&admitted["attempt"]);
    let assignment_id = id(&admitted["assignment"]);
    store.claim_attempt(attempt_id).unwrap();
    let before = store.get_attempt(attempt_id).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_run BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
    assert!(
        store
            .record_agent_event(attempt_id, 1, &json!({"type":"agent_start"}))
            .is_err()
    );
    assert_eq!(store.get_attempt(attempt_id).unwrap(), before);
    assert!(store.agent_events(id(&run)).unwrap().is_empty());
    db.execute_batch("DROP TRIGGER fail_run").unwrap();
    store
        .record_agent_event(attempt_id, 1, &json!({"type":"agent_start"}))
        .unwrap();
    store
        .record_agent_event(attempt_id, 1, &json!({"type":"forged_replay"}))
        .unwrap();
    assert_eq!(
        store.agent_events(id(&run)).unwrap()[0]["event"]["type"],
        "agent_start"
    );
    db.execute_batch("CREATE TRIGGER fail_observation BEFORE INSERT ON observations WHEN NEW.cursor=2 BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
    assert!(
        store
            .record_observer_page(
                assignment_id,
                &[json!({"cursor":1}), json!({"cursor":2})],
                2
            )
            .is_err()
    );
    assert_eq!(store.observer_cursor(assignment_id).unwrap(), 0);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM observations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_observation").unwrap();
    store
        .record_observer_page(
            assignment_id,
            &[json!({"cursor":1}), json!({"cursor":2})],
            2,
        )
        .unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(reopened.observer_cursor(assignment_id).unwrap(), 2);
}

#[test]
fn concurrent_store_claim_and_failed_replacement_preserve_ownership() {
    let (root, store, run, input) = setup();
    let admitted = store.admit_assignment(id(&run), &input).unwrap();
    let attempt_id = id(&admitted["attempt"]);
    let assignment_id = id(&admitted["assignment"]);
    let other = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    store.claim_attempt(attempt_id).unwrap();
    assert!(other.claim_attempt(attempt_id).is_err());
    store.begin_pause(id(&run), None).unwrap();
    store
        .record_ownership(attempt_id, false, Some("Unverified process"))
        .unwrap();
    assert_eq!(
        other.finish_pause(id(&run)).unwrap()["status"],
        "attention_required"
    );
    assert!(
        other
            .replace_attempt(assignment_id, &json!({"automatic":false}))
            .is_err()
    );
    store
        .record_ownership(attempt_id, true, Some("Verified stop"))
        .unwrap();
    store.finish_pause(id(&run)).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_assignment BEFORE UPDATE ON assignments BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
    assert!(
        other
            .replace_attempt(assignment_id, &json!({"automatic":true}))
            .is_err()
    );
    assert_eq!(
        store.get_assignment(assignment_id).unwrap()["currentAttemptId"],
        attempt_id
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM attempts WHERE assignment_id=?",
            params![assignment_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    db.execute_batch("DROP TRIGGER fail_assignment").unwrap();
    let replaced = other
        .replace_attempt(assignment_id, &json!({"automatic":true}))
        .unwrap();
    assert_eq!(replaced["generation"], 2);
    assert_eq!(replaced["previousAttemptId"], attempt_id);
    assert_eq!(
        store.get_attempt(attempt_id).unwrap()["ownership"],
        "stopped"
    );
}
