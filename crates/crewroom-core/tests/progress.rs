//! A run's latest observed progress (FOR-77): agent output, peer messages and the run's start,
//! read from recorded facts only.
use crewroom_core::store::Store;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

const MINUTE: i64 = 60_000;

#[test]
fn progress_names_the_latest_observed_event_and_its_source() {
    let root = tempfile::tempdir().unwrap();
    let now = Arc::new(AtomicI64::new(1_700_000_000_000));
    let clock = now.clone();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(move || clock.load(Ordering::SeqCst));
    let task = store
        .create_task(&json!({"title":"Progress","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let run_id = run["id"].as_str().unwrap().to_owned();
    // A queued run has no observed progress at all.
    assert_eq!(
        store.run_progress(&run_id).unwrap(),
        json!({"lastAt":null,"source":null})
    );
    let role: Value = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let admitted = store
        .admit_assignment(
            &run_id,
            &json!({"name":"Coordinator","coordinator":true,"role":role,"model":{},"input":"Inspect"}),
        )
        .unwrap();
    let attempt = admitted["attempt"]["id"].as_str().unwrap().to_owned();
    store.claim_attempt(&attempt).unwrap();
    store.set_desired_state(&run_id, "running").unwrap();
    store.start_execution(&run_id).unwrap();
    let started = store.run_progress(&run_id).unwrap();
    assert_eq!(started["source"], "run start");
    assert_eq!(started["lastAt"], "2023-11-14T22:13:20.000Z");

    now.fetch_add(5 * MINUTE, Ordering::SeqCst);
    store
        .record_agent_event(&attempt, 1, &json!({"type":"agent_start"}))
        .unwrap();
    let output = store.run_progress(&run_id).unwrap();
    assert_eq!(output["source"], "agent output");
    assert_eq!(output["lastAt"], "2023-11-14T22:18:20.000Z");

    // A replayed sequence is not new output.
    now.fetch_add(5 * MINUTE, Ordering::SeqCst);
    store
        .record_agent_event(&attempt, 1, &json!({"type":"agent_start"}))
        .unwrap();
    assert_eq!(store.run_progress(&run_id).unwrap(), output);

    store
        .prepare_message(
            &attempt,
            "send",
            &json!({"to":"Coordinator","body":"Status"}),
        )
        .unwrap();
    let message = store.run_progress(&run_id).unwrap();
    assert_eq!(message["source"], "agent message");
    assert_eq!(message["lastAt"], "2023-11-14T22:23:20.000Z");
}
