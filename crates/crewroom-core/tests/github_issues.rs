//! GitHub Issues intake records and the schema-32 table (FOR-45). Transport is
//! tested in the service against a simulated GitHub API.
use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};

fn issue(number: u64, title: &str, state: &str, updated: &str) -> Value {
    json!({"id":5000+number,"number":number,"title":title,"body":"Synthetic body","state":state,"html_url":format!("https://github.com/Fixture/Issues/issues/{number}"),"updated_at":updated})
}

#[test]
fn sync_matches_on_identity_keeps_refinements_and_records_failures_without_partial_imports() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(store.github_sync_state().unwrap(), None);
    let saved = store
        .save_github_source(&json!({"repository":"Fixture/Issues","labels":["crewroom"]}))
        .unwrap();
    assert_eq!(saved["state"], "not_synced");
    let source = saved["source"].clone();
    // Saving the same source again keeps its state.
    assert_eq!(
        store
            .save_github_source(&json!({"repository":"Fixture/Issues","labels":"crewroom"}))
            .unwrap(),
        saved
    );

    let state = store
        .commit_github_sync(
            &source,
            &json!([
                issue(1, "First", "open", "2026-10-18T08:00:00Z"),
                issue(2, "Second", "closed", "2026-10-18T08:00:00Z")
            ]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    assert_eq!(state["state"], "connected");
    assert_eq!(state["issueCount"], 2);
    let tasks = store.snapshot().unwrap()["tasks"].clone();
    let first = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "#1")
        .unwrap()
        .clone();
    assert_eq!(first["source"], "github");
    assert_eq!(first["sourceSite"], "https://github.com/fixture/issues");
    assert_eq!(first["sourceStatus"], "open");
    assert_eq!(first["project"], "Fixture/Issues");
    let id = first["id"].as_str().unwrap().to_owned();
    store
        .refine(
            &id,
            &json!({"revision":1,"sourceRevision":1,"brief":"Operator brief","criteria":["Kept"]}),
        )
        .unwrap();

    // Closing the issue is a status change, not a requirement change.
    store
        .commit_github_sync(
            &source,
            &json!([issue(1, "First", "closed", "2026-10-18T09:00:00Z")]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    let closed = store.get_task(&id).unwrap();
    assert_eq!(closed["sourceStatus"], "closed");
    assert_eq!(closed["needsReacceptance"], false);
    // A body change asks for re-acceptance and keeps the operator's brief.
    let mut changed = issue(1, "First", "closed", "2026-10-18T10:00:00Z");
    changed["body"] = json!("Synthetic body, revised");
    store
        .commit_github_sync(&source, &json!([changed]), &json!([]), &json!([]))
        .unwrap();
    let revised = store.get_task(&id).unwrap();
    assert_eq!(revised["needsReacceptance"], true);
    assert_eq!(revised["brief"], "Operator brief");
    assert_eq!(revised["sourceRevision"], 2);

    // An older observation, a pull request, or a duplicate saves nothing.
    let before = store.snapshot().unwrap()["tasks"].clone();
    for bad in [
        json!([issue(1, "Older", "open", "2026-10-18T07:00:00Z")]),
        json!([{"id":5009,"number":9,"title":"PR","body":"","state":"open","html_url":"https://github.com/fixture/issues/pull/9","pull_request":{}}]),
        json!([
            issue(3, "Dup", "open", "2026-10-18T11:00:00Z"),
            issue(3, "Dup", "open", "2026-10-18T11:00:00Z")
        ]),
    ] {
        assert!(
            store
                .commit_github_sync(&source, &bad, &json!([]), &json!([]))
                .is_err()
        );
    }
    assert_eq!(store.snapshot().unwrap()["tasks"], before);

    // A failure keeps the last complete sync and the imported tickets.
    let synced_at = store.github_sync_state().unwrap().unwrap()["lastSyncedAt"].clone();
    store
        .fail_github_sync(&source, "rate_limited", "GitHub rate-limited this request.")
        .unwrap();
    let failed = store.github_sync_state().unwrap().unwrap();
    assert_eq!(failed["state"], "unavailable");
    assert_eq!(failed["reason"], "rate_limited");
    assert_eq!(failed["lastSyncedAt"], synced_at);

    // Issues GitHub no longer serves are recorded with the sync, validated,
    // and replaced by the next complete sync.
    let gone = json!([{"taskId": id, "key": "#1", "status": 410}]);
    let state = store
        .commit_github_sync(&source, &json!([]), &json!([]), &gone)
        .unwrap();
    assert_eq!(state["unreadable"], gone);
    assert_eq!(state["state"], "connected");
    assert!(state.get("reason").is_none());
    for bad in [
        json!([{"taskId": id, "key": "1", "status": 410}]),
        json!([{"taskId": id, "key": "#1"}]),
        json!({"taskId": id}),
    ] {
        assert!(
            store
                .commit_github_sync(&source, &json!([]), &json!([]), &bad)
                .is_err(),
            "{bad}"
        );
    }
    let state = store
        .commit_github_sync(&source, &json!([]), &json!([]), &json!([]))
        .unwrap();
    assert_eq!(state["unreadable"], json!([]));

    // Replacing the source starts over; a sync of the old source is refused
    // and a late failure for it is ignored. Earlier tickets stay.
    store
        .save_github_source(&json!({"repository":"fixture/other"}))
        .unwrap();
    assert!(
        store
            .commit_github_sync(&source, &json!([]), &json!([]), &json!([]))
            .is_err()
    );
    store
        .fail_github_sync(&source, "rejected", "stale")
        .unwrap();
    let replaced = store.github_sync_state().unwrap().unwrap();
    assert_eq!(replaced["state"], "not_synced");
    assert_eq!(replaced["source"]["repository"], "fixture/other");
    assert_eq!(store.get_task(&id).unwrap()["brief"], "Operator brief");
}

#[test]
fn schema_32_adds_the_github_sync_table_to_a_schema_31_store() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE github_sync; PRAGMA user_version=31;")
        .unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.github_sync_state().unwrap(), None);
    store
        .save_github_source(&json!({"repository":"fixture/issues"}))
        .unwrap();
    drop(store);
    let version: u32 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 32);
}
