use crewroom_core::{
    repositories::{validate, validate_update},
    store::Store,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    process::Command,
    sync::{Arc, Barrier},
};
fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn setup() -> (tempfile::TempDir, Store, Value) {
    let root = tempfile::tempdir().unwrap();
    let checkout = std::fs::canonicalize(root.path()).unwrap().join("repo");
    std::fs::create_dir(&checkout).unwrap();
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
            Command::new("git")
                .arg("-C")
                .arg(&checkout)
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
    let input = json!({"name":"Fixture","path":checkout,"targetBranch":"main","requestId":key()});
    (root, store, input)
}
fn accepted(store: &Store, repository: &Value) -> Value {
    let task = store
        .create_task(&json!({"title":"Research","description":"","repositoryId":repository["id"]}))
        .unwrap();
    store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Accepted","criteria":["Verify source"]})).unwrap()
}
#[test]
fn research_rechecks_scope_selection_configuration_retirement_and_checkout() {
    for change in [
        "scope",
        "selection",
        "configuration",
        "retirement",
        "checkout",
    ] {
        let (root, store, input) = setup();
        let repo = store.create_repository(&input).unwrap();
        let task = accepted(&store, &repo);
        let request = json!({"revision":2,"requestId":key()});
        let pinned = store
            .prepare_research(id(&task), &request)
            .unwrap()
            .validate()
            .unwrap();
        match change {
            "scope" => {
                store.refine(id(&task),&json!({"revision":2,"sourceRevision":1,"brief":"Changed","criteria":["Changed"]})).unwrap();
            }
            "selection" => {
                let mut other = input.clone();
                other["name"] = json!("Other");
                other["targetBranch"] = json!("alternate");
                other["requestId"] = json!(key());
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(input["path"].as_str().unwrap())
                        .args(["branch", "alternate"])
                        .status()
                        .unwrap()
                        .success()
                );
                let other = store.create_repository(&other).unwrap();
                store
                    .assign_repository(id(&task), &json!({"revision":2,"repositoryId":other["id"]}))
                    .unwrap();
            }
            "configuration" => {
                let mut update = input.clone();
                update["name"] = json!("Edited");
                update["revision"] = json!(1);
                update["requestId"] = json!(key());
                store
                    .update_repository(id(&repo), &validate_update(&update).unwrap(), &update)
                    .unwrap();
            }
            "retirement" => {
                store
                    .retire_repository(id(&repo), &json!({"revision":1,"requestId":key()}))
                    .unwrap();
            }
            "checkout" => {
                std::fs::rename(input["path"].as_str().unwrap(), root.path().join("moved"))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = store.snapshot().unwrap();
        assert!(store.queue_pinned_research(&pinned).is_err(), "{change}");
        assert_eq!(store.snapshot().unwrap(), before, "{change} must be atomic");
        drop(store);
        let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
        assert!(
            reopened.snapshot().unwrap()["runs"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn research_failure_rolls_back_created_run_then_reopens_and_replays_without_checkout() {
    let (root, store, input) = setup();
    let repo = store.create_repository(&input).unwrap();
    let task = accepted(&store, &repo);
    let request = json!({"revision":2,"requestId":key()});
    let pinned = store
        .prepare_research(id(&task), &request)
        .unwrap()
        .validate()
        .unwrap();
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_pin BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'pin interrupted'); END").unwrap();
    assert!(store.queue_pinned_research(&pinned).is_err());
    assert!(
        store.snapshot().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    db.execute_batch("DROP TRIGGER fail_pin").unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let saved = reopened.queue_pinned_research(&pinned).unwrap();
    std::fs::rename(input["path"].as_str().unwrap(), root.path().join("moved")).unwrap();
    assert_eq!(reopened.queue_research(id(&task), &request).unwrap(), saved);
}
#[test]
fn repository_and_receipt_are_atomic_and_replay_after_reopen() {
    let (root, store, input) = setup();
    let validated = validate(&input).unwrap();
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON queue_requests BEGIN SELECT RAISE(ABORT,'receipt interrupted'); END").unwrap();
    assert!(store.save_repository(&validated).is_err());
    assert!(
        store.snapshot().unwrap()["repositories"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    db.execute_batch("DROP TRIGGER fail_receipt").unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let saved = reopened.save_repository(&validated).unwrap();
    drop(reopened);
    std::fs::rename(input["path"].as_str().unwrap(), root.path().join("moved")).unwrap();
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.create_repository(&input).unwrap(), saved);
}
#[test]
fn concurrent_repository_preflights_commit_one_configuration() {
    for same_key in [true, false] {
        let (root, store, input) = setup();
        drop(store);
        let path = root.path().join("workflow.sqlite");
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let (path, barrier, mut input) = (path.clone(), barrier.clone(), input.clone());
                if !same_key {
                    input["requestId"] = json!(key());
                }
                std::thread::spawn(move || {
                    let store = Store::open(&path).unwrap();
                    let validated = validate(&input).unwrap();
                    barrier.wait();
                    store.save_repository(&validated)
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if same_key { 2 } else { 1 }
        );
        if same_key {
            assert_eq!(results[0].as_ref().unwrap(), results[1].as_ref().unwrap());
        }
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.snapshot().unwrap()["repositories"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM queue_requests", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
#[test]
fn competing_research_preflights_replay_one_run_or_refuse_the_other_key() {
    for same_key in [true, false] {
        let (root, store, input) = setup();
        let repo = store.create_repository(&input).unwrap();
        let task = accepted(&store, &repo);
        drop(store);
        let path = root.path().join("workflow.sqlite");
        let barrier = Arc::new(Barrier::new(2));
        let request = json!({"revision":2,"requestId":key()});
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let (path, barrier, task, mut request) =
                    (path.clone(), barrier.clone(), task.clone(), request.clone());
                if !same_key {
                    request["requestId"] = json!(key());
                }
                std::thread::spawn(move || {
                    let store = Store::open(&path).unwrap();
                    let pinned = store
                        .prepare_research(id(&task), &request)
                        .unwrap()
                        .validate()
                        .unwrap();
                    barrier.wait();
                    store.queue_pinned_research(&pinned)
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if same_key { 2 } else { 1 }
        );
        if same_key {
            assert_eq!(results[0].as_ref().unwrap(), results[1].as_ref().unwrap());
        }
        assert_eq!(
            Store::open(&path).unwrap().snapshot().unwrap()["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
