//! GitHub issue delivery records (FOR-48): the schema-33 outbox table, the
//! optional workflow labels and the issue state Crewroom observes after a
//! merge. Transport is tested in the runtime against a simulated GitHub API.
use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};

fn issue(number: u64, state: &str, updated: &str) -> Value {
    json!({"id":7000+number,"number":number,"title":"Synthetic","body":"Synthetic body","state":state,"html_url":format!("https://github.com/fixture/issues/issues/{number}"),"updated_at":updated})
}

fn synced(store: &Store) -> Value {
    let saved = store
        .save_github_source(&json!({"repository":"fixture/issues"}))
        .unwrap();
    store
        .commit_github_sync(
            &saved["source"],
            &json!([issue(1, "open", "2026-10-18T08:00:00Z")]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    store.snapshot().unwrap()["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "#1")
        .unwrap()
        .clone()
}

#[test]
fn workflow_labels_are_off_by_default_validated_and_kept_across_source_changes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert!(
        store
            .save_github_status_labels(&json!({"inProgress":"crewroom:in-progress"}))
            .is_err(),
        "labels need a saved source"
    );
    let saved = store
        .save_github_source(&json!({"repository":"fixture/issues"}))
        .unwrap();
    assert!(saved.get("statusLabels").is_none(), "off by default");
    for bad in [
        json!({"inProgress":"a,b"}),
        json!({"inProgress":"x".repeat(51)}),
        json!({"inProgress":"Same","inReview":"same"}),
        json!({"inProgress":7}),
        json!({"other":"x"}),
    ] {
        assert!(store.save_github_status_labels(&bad).is_err(), "{bad}");
    }
    let state = store
        .save_github_status_labels(&json!({"inProgress":" in progress ","inReview":""}))
        .unwrap();
    assert_eq!(
        state["statusLabels"],
        json!({"inProgress":"in progress","inReview":null})
    );
    // A repository change resets sync state but keeps delivery labels.
    let changed = store
        .save_github_source(&json!({"repository":"fixture/other"}))
        .unwrap();
    assert_eq!(changed["state"], "not_synced");
    assert_eq!(changed["statusLabels"]["inProgress"], "in progress");
    // Clearing both turns labels off again.
    let cleared = store
        .save_github_status_labels(&json!({"inProgress":null,"inReview":" "}))
        .unwrap();
    assert!(cleared.get("statusLabels").is_none());
}

#[test]
fn observed_issue_state_changes_only_the_source_status() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let task = synced(&store);
    let id = task["id"].as_str().unwrap();
    let closed = store
        .record_github_issue_state(id, "closed", Some("2026-10-18T09:00:00Z"))
        .unwrap();
    assert_eq!(closed["sourceStatus"], "closed");
    assert_eq!(closed["sourceUpdatedAt"], "2026-10-18T09:00:00Z");
    for key in [
        "title",
        "description",
        "sourceRevision",
        "revision",
        "brief",
    ] {
        assert_eq!(closed[key], task[key], "{key} is unchanged");
    }
    // An older observation is ignored.
    let kept = store
        .record_github_issue_state(id, "open", Some("2026-10-18T08:30:00Z"))
        .unwrap();
    assert_eq!(kept["sourceStatus"], "closed");
    assert!(store.record_github_issue_state(id, "merged", None).is_err());
    let local = store
        .create_task(&json!({"title":"Local","description":""}))
        .unwrap();
    assert!(
        store
            .record_github_issue_state(local["id"].as_str().unwrap(), "closed", None)
            .is_err(),
        "only GitHub-sourced tickets carry a GitHub state"
    );
}

#[test]
fn github_receipts_are_kept_apart_from_jira_and_notify_the_coordinator() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Receipts","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let run_id = run["id"].as_str().unwrap();
    let mut update = json!({"id":"00000000-0000-4000-8000-000000000048","runId":run_id,"eventKey":"impact","kind":"milestone","state":"pending"});
    store.save_github_update(&update).unwrap();
    assert!(store.jira_updates(Some(run_id)).unwrap().is_empty());
    assert!(store.delivery_notices(run_id).unwrap().is_empty());
    update["state"] = json!("confirmed");
    update["externalId"] = json!(99);
    store.save_github_update(&update).unwrap();
    assert_eq!(store.github_updates(Some(run_id)).unwrap(), vec![update]);
    let notices = store.delivery_notices(run_id).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["payload"]["kind"], "github");
    assert_eq!(notices[0]["payload"]["externalId"], 99);
}

#[test]
fn schema_33_adds_the_github_updates_table_to_a_schema_32_store() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE github_updates; PRAGMA user_version=32;")
        .unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    assert!(store.github_updates(None).unwrap().is_empty());
    drop(store);
    let version: u32 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 33);
}
