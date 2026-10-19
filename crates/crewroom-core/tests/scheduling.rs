mod support;
use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};

fn setup() -> (tempfile::TempDir, Store, Vec<Value>, Value) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("repo");
    std::fs::create_dir(&path).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let repository = store
        .create_repository(&json!({"name":"Fixture","path":path,"targetBranch":"main"}))
        .unwrap();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    let launch = json!({"team":[{"name":"Coordinator","roleId":"coordinator","coordinator":true,"input":"Inspect accepted work."}]});
    let runs=(0..2).map(|i|{
        let task=store.create_task(&json!({"title":format!("Plan {i}"),"description":"","repositoryId":repository["id"]})).unwrap();
        let task_id=task["id"].as_str().unwrap();
        store.refine(task_id,&json!({"revision":1,"sourceRevision":1,"brief":"Keep behavior","criteria":["Retain evidence"]})).unwrap();
        support::accept_plan(&store,task_id);
        store.queue_with_roles(task_id,&json!({"revision":2,"launch":launch}),&roles).unwrap()
    }).collect();
    (root, store, runs, launch)
}

#[test]
fn failed_slot_write_rolls_back_schedule_positions_and_queue_revision() {
    let (root, store, runs, _) = setup();
    let before = store.coding_queue().unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_slot BEFORE INSERT ON coding_slots BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
    assert!(store.claim_next_coding_slot().is_err());
    assert_eq!(store.coding_queue().unwrap(), before);
    assert_eq!(
        store.get_run(runs[0]["id"].as_str().unwrap()).unwrap(),
        runs[0]
    );
    db.execute_batch("DROP TRIGGER fail_slot").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let claimed = reopened.claim_next_coding_slot().unwrap().unwrap();
    assert_eq!(claimed["id"], runs[0]["id"]);
    assert!(claimed.get("position").is_none());
    assert_eq!(reopened.coding_queue().unwrap()["plans"][0]["position"], 0);
}

#[test]
fn concurrent_connections_can_claim_only_one_coding_slot() {
    let (root, store, runs, _) = setup();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = root.path().join("workflow.sqlite");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap().with_clock(|| 1000);
                barrier.wait();
                store.claim_next_coding_slot().unwrap()
            })
        })
        .collect();
    let claimed: Vec<_> = threads
        .into_iter()
        .filter_map(|t| t.join().unwrap())
        .collect();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["id"], runs[0]["id"]);
    assert_eq!(store.coding_slot().unwrap().unwrap()["id"], runs[0]["id"]);
    assert_eq!(
        store.coding_queue().unwrap()["plans"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn interrupted_launch_replacement_is_atomic_across_the_entire_team() {
    let (root, store, runs, _) = setup();
    let run_id = runs[0]["id"].as_str().unwrap();
    store.claim_coding_slot(run_id, false).unwrap();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    let team:Vec<_>=[("Coordinator",true),("Reader",false)].into_iter().map(|(name,coordinator)|json!({"name":name,"coordinator":coordinator,"role":roles[0],"model":{},"input":"Inspect evidence","brief":{"criteria":[],"dependsOn":[]}})).collect();
    let admitted = store.admit_initial_team(run_id, &team).unwrap();
    store.set_desired_state(run_id, "running").unwrap();
    for item in admitted.as_array().unwrap() {
        store
            .claim_attempt(item["attempt"]["id"].as_str().unwrap())
            .unwrap();
        store
            .record_ownership(item["attempt"]["id"].as_str().unwrap(), true, None)
            .unwrap();
    }
    store.begin_pause(run_id, Some("recovery")).unwrap();
    store.finish_pause(run_id).unwrap();
    let before = store.runtime(run_id).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_second BEFORE INSERT ON attempts WHEN (SELECT COUNT(*) FROM attempts)=3 BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
    assert!(store.restart_scheduled_launch(run_id).is_err());
    assert_eq!(store.runtime(run_id).unwrap(), before);
    db.execute_batch("DROP TRIGGER fail_second").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    assert_eq!(
        reopened.restart_scheduled_launch(run_id).unwrap()["status"],
        "awaiting_runner"
    );
    let runtime = reopened.runtime(run_id).unwrap();
    assert_eq!(runtime["attempts"].as_array().unwrap().len(), 4);
    assert_eq!(
        runtime["assignments"][0]["id"],
        before["assignments"][0]["id"]
    );
    assert_eq!(
        runtime["assignments"][1]["id"],
        before["assignments"][1]["id"]
    );
}
