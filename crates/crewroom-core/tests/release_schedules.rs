//! LIM-04: retained seeded schedule contract on real native SQLite state.
//! Assignment IDs are stopped records; reopen is not a claim of OS process death.
use crewroom_core::store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn schedule(seed: u32) -> Vec<Value> {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let mut store = Store::open(&path).unwrap().with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Seeded native schedule","description":""}))
        .unwrap();
    store.refine(id(&task), &json!({"revision":1,"sourceRevision":1,"brief":"Retain message state","criteria":["Exactly one logical effect"]})).unwrap();
    let run = store
        .queue_research(
            id(&task),
            &json!({"revision":2,"requestId":"00000000-0000-4000-8000-000000000001"}),
        )
        .unwrap();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    let coordinator = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let specialist = roles.iter().find(|r| r["id"] == "tech-writer").unwrap();
    let sender = store.admit_assignment(id(&run), &json!({"name":"Coordinator","coordinator":true,"role":coordinator,"model":{},"input":"Coordinate"})).unwrap();
    store.admit_assignment(id(&run), &json!({"name":"Specialist","coordinator":false,"role":specialist,"model":{},"input":"Read"})).unwrap();
    let mut random = seed;
    let mut next = || {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        random
    };
    let mut effects = BTreeMap::new();
    let mut trace = Vec::new();
    for step in 0..100 {
        let key = next() % 12;
        let event = next() % 4;
        trace.push(json!({"seed":seed,"step":step,"event":event,"key":key}));
        if event == 0 {
            drop(store);
            store = Store::open(&path).unwrap().with_clock(|| 1000);
        }
        let message = store
            .prepare_message(
                id(&sender["attempt"]),
                &format!("key-{key}"),
                &json!({"to":"Specialist","body":format!("Message {key}")}),
            )
            .unwrap();
        if let Some(previous) = effects.get(&key) {
            assert_eq!(message["id"], *previous, "trace={trace:?}");
        }
        effects.insert(key, message["id"].clone());
        if event >= 2 {
            store
                .record_message_stored(id(&message), u64::from(key + 1))
                .unwrap();
        }
        if event == 3 {
            store.accept_message(id(&message)).unwrap();
            store.record_dispatch(id(&message), true).unwrap();
            store.record_dispatch(id(&message), false).unwrap();
            assert_eq!(
                store.get_peer_message(id(&message)).unwrap()["disposition"],
                "received",
                "trace={trace:?}"
            );
        }
        let state = store.runtime(id(&run)).unwrap();
        assert_eq!(
            state["messages"].as_array().unwrap().len(),
            effects.len(),
            "trace={trace:?}"
        );
        trace.last_mut().unwrap()["disposition"] =
            store.get_peer_message(id(&message)).unwrap()["disposition"].clone();
        trace.last_mut().unwrap()["logicalMessages"] = json!(effects.len());
    }
    store.set_desired_state(id(&run), "cancelled").unwrap();
    store.begin_pause(id(&run), None).unwrap();
    store.finish_pause(id(&run)).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.get_run(id(&run)).unwrap()["status"],
        "cancelled",
        "trace={trace:?}"
    );
    assert!(
        store.set_desired_state(id(&run), "running").is_err(),
        "trace={trace:?}"
    );
    assert_eq!(
        store.runtime(id(&run)).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        effects.len(),
        "trace={trace:?}"
    );
    trace.push(json!({"seed":seed,"control":"cancel-reopen-refuse-resume","status":"cancelled","logicalMessages":effects.len()}));
    trace
}
#[test]
fn retained_seeded_schedules_replay_native_messages_and_terminal_controls() {
    for seed in [7, 42, 20261002] {
        let first = schedule(seed);
        let replay = schedule(seed);
        assert_eq!(
            first, replay,
            "seed={seed}; first={first:?}; replay={replay:?}"
        );
        eprintln!(
            "{}",
            json!({"seed":seed,"events":first,"replayed":true,"boundary":"Native SQLite state; stopped assignment records; reopen, not OS-crash injection."})
        );
    }
}
