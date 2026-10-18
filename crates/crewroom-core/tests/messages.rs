use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};

fn setup() -> (tempfile::TempDir, Store, String, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Message interruption","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let run_id = run["id"].as_str().unwrap().to_owned();
    let role: Value = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let admitted=store.admit_assignment(&run_id,&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{},"input":"Inspect"})).unwrap();
    let attempt = admitted["attempt"]["id"].as_str().unwrap().to_owned();
    store.claim_attempt(&attempt).unwrap();
    store.record_attempt_identity(&attempt,&json!({"hostPid":101,"ompPid":102,"hostUrl":"http://127.0.0.1:59999","sessionId":"fixture","pane":"%1"})).unwrap();
    store.set_desired_state(&run_id, "running").unwrap();
    store.start_execution(&run_id).unwrap();
    (root, store, run_id, attempt)
}
fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
#[test]
fn dispatch_and_recipient_input_commit_together_after_reopen() {
    let (root, store, _, attempt) = setup();
    let message = store
        .prepare_message(
            &attempt,
            "send",
            &json!({"to":"Coordinator","body":"Inspect"}),
        )
        .unwrap();
    store.record_message_stored(id(&message), 1).unwrap();
    let before = store.accept_message(id(&message)).unwrap();
    let prior_attempt = store.get_attempt(&attempt).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_input BEFORE UPDATE ON attempts BEGIN SELECT RAISE(ABORT,'fixture interrupted input'); END;").unwrap();
    assert!(store.record_dispatch(id(&message), false).is_err());
    assert_eq!(store.get_peer_message(id(&message)).unwrap(), before);
    assert_eq!(store.get_attempt(&attempt).unwrap(), prior_attempt);
    db.execute_batch("DROP TRIGGER fail_input").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    reopened.record_dispatch(id(&message), false).unwrap();
    assert_eq!(
        reopened.get_peer_message(id(&message)).unwrap()["disposition"],
        "dispatched"
    );
    assert_eq!(
        reopened.get_attempt(&attempt).unwrap()["activeInputMessageId"],
        message["id"]
    );
}
#[test]
fn steering_batch_and_lost_receipt_application_are_atomic() {
    let (root, store, run, attempt) = setup();
    let first = store
        .steer(&run, &json!({"requestId":"first","body":"First"}))
        .unwrap();
    let second = store
        .steer(&run, &json!({"requestId":"second","body":"Second"}))
        .unwrap();
    let before = store.steering(&run).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_second BEFORE UPDATE ON steering WHEN json_extract(NEW.data,'$.body')='Second' BEGIN SELECT RAISE(ABORT,'fixture interrupted batch'); END;").unwrap();
    let ids = vec![id(&first).to_owned(), id(&second).to_owned()];
    assert!(store.dispatch_steering(&attempt, &ids, "input").is_err());
    assert_eq!(store.steering(&run).unwrap(), before);
    db.execute_batch("DROP TRIGGER fail_second").unwrap();
    store.dispatch_steering(&attempt, &ids, "input").unwrap();
    let before = store.steering(&run).unwrap();
    db.execute_batch("CREATE TRIGGER fail_application BEFORE UPDATE ON steering WHEN json_extract(NEW.data,'$.delivery')='applied' BEGIN SELECT RAISE(ABORT,'fixture interrupted decision'); END;").unwrap();
    let decision = json!({"messageId":first["id"],"disposition":"applied","reason":"Included"});
    assert!(store.apply_steering(&attempt, "apply", &decision).is_err());
    assert_eq!(store.steering(&run).unwrap(), before);
    db.execute_batch("DROP TRIGGER fail_application").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    let applied = reopened
        .apply_steering(&attempt, "apply", &decision)
        .unwrap();
    assert_eq!(
        applied["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["dispatched", "received", "applied"]
    );
    assert_eq!(
        reopened
            .apply_steering(&attempt, "replay", &decision)
            .unwrap(),
        applied
    );
}
#[test]
fn stopped_input_restoration_rolls_back_across_steering_and_controller_notice() {
    let (root, store, run, attempt) = setup();
    let notice = store
        .notice_delivery(&run, &json!({"kind":"fixture","subjectId":"one"}))
        .unwrap();
    let message = store
        .steer(&run, &json!({"requestId":"one","body":"Instruction"}))
        .unwrap();
    store
        .dispatch_steering(&attempt, &[id(&message).to_owned()], id(&notice))
        .unwrap();
    store
        .dispatch_delivery_notice(&attempt, id(&notice), false)
        .unwrap();
    store.record_ownership(&attempt, true, None).unwrap();
    let before_steering = store.steering(&run).unwrap();
    let before_notices = store.delivery_notices(&run).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_notice BEFORE UPDATE ON delivery_notices BEGIN SELECT RAISE(ABORT,'fixture interrupted restoration'); END;").unwrap();
    assert!(
        store
            .restore_undispatched_input(&attempt, id(&notice))
            .is_err()
    );
    assert_eq!(store.steering(&run).unwrap(), before_steering);
    assert_eq!(store.delivery_notices(&run).unwrap(), before_notices);
    db.execute_batch("DROP TRIGGER fail_notice").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    reopened
        .restore_undispatched_input(&attempt, id(&notice))
        .unwrap();
    assert_eq!(reopened.steering(&run).unwrap()[0]["delivery"], "pending");
    assert_eq!(
        reopened.delivery_notices(&run).unwrap()[0]["disposition"],
        "pending"
    );
    assert!(reopened.steering(&run).unwrap()[0].get("inputId").is_none());
    assert!(
        reopened.delivery_notices(&run).unwrap()[0]
            .get("attemptId")
            .is_none()
    );
}
