use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};

fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn source() -> Value {
    json!({"siteUrl":"https://fixture.atlassian.net","cloudId":"fixture","filterId":"1","name":"Assigned","assigneeAccountId":"operator"})
}
fn issue(title: &str, updated: &str) -> Value {
    json!({"id":"1","key":"FIX-1","fields":{"summary":title,"description":"Source","status":{"name":"Open"},"updated":updated}})
}
fn setup() -> (tempfile::TempDir, Store, Value) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Provider records","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(id(&task), &json!({"revision":1,"purpose":"explore"}))
        .unwrap();
    (root, store, run)
}
fn publication(run: &Value) -> Value {
    // Trusted provider observation fixture. No candidate, Git or remote proof is implied.
    json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run["id"],"candidateId":"fixture-candidate","revision":{"head":"fixture-head"},"push":{"state":"confirmed"},"pr":{"state":"confirmed","id":9,"providerState":"OPEN"},"ready":false})
}
fn comments(body: &str) -> Value {
    json!({"state":"complete","comments":[{"id":1,"created_on":"2026-09-28T00:00:00Z","updated_on":"2026-09-28T00:00:00Z","deleted":false,"content":{"raw":body}}]})
}
#[test]
fn publication_insert_rolls_back_if_accounting_cannot_commit() {
    let (root, store, run) = setup();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_account BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'accounting interrupted'); END").unwrap();
    let record = publication(&run);
    assert!(store.save_publication(&record).is_err());
    assert!(store.publication(id(&run)).unwrap().is_none());
    assert_eq!(store.get_run(id(&run)).unwrap(), run);
    db.execute_batch("DROP TRIGGER fail_account").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(reopened.save_publication(&record).unwrap(), record);
}
#[test]
fn failed_slot_release_cannot_partially_commit_a_merge_observation() {
    let (root, store, run) = setup();
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    // A store-only coding-slot fixture; actual admission and Git evidence are separate gates.
    let mut coding = run.clone();
    coding["purpose"] = json!("coding");
    coding["status"] = json!("running");
    coding["desiredState"] = json!("running");
    coding["activeSince"] = json!(1000);
    db.execute(
        "UPDATE runs SET data=? WHERE id=?",
        rusqlite::params![coding.to_string(), id(&run)],
    )
    .unwrap();
    db.execute(
        "INSERT INTO coding_slots(run_id,task_id,project) SELECT id,task_id,'' FROM runs WHERE id=?",
        [id(&run)],
    )
    .unwrap();
    let original = publication(&run);
    store.save_publication(&original).unwrap();
    let before = store.get_run(id(&run)).unwrap();
    db.execute_batch("CREATE TRIGGER fail_slot BEFORE DELETE ON coding_slots BEGIN SELECT RAISE(ABORT,'slot interrupted'); END").unwrap();
    let mut merged = original.clone();
    merged["merged"] = json!({"commit":"fixture-head","observedAt":"2026-09-28T00:00:00Z"});
    assert!(store.save_publication(&merged).is_err());
    assert_eq!(store.publication(id(&run)).unwrap(), Some(original));
    assert_eq!(store.get_run(id(&run)).unwrap(), before);
    assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
    db.execute_batch("DROP TRIGGER fail_slot").unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    reopened.save_publication(&merged).unwrap();
    assert!(reopened.coding_slot().unwrap().is_none());
    assert_eq!(
        reopened.get_run(id(&run)).unwrap()["slotRelease"]["reason"],
        "merged"
    );
}
#[test]
fn failed_feedback_save_retains_prior_observation_then_replays_after_reopen() {
    let (root, store, run) = setup();
    store.save_publication(&publication(&run)).unwrap();
    let old = store
        .save_publication_feedback(id(&run), 9, &comments("Original"), "2026-09-28T00:00:00Z")
        .unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_feedback BEFORE UPDATE ON publications BEGIN SELECT RAISE(ABORT,'feedback interrupted'); END").unwrap();
    let mut next = comments("Newer");
    next["comments"][0]["updated_on"] = json!("2026-09-29T00:00:00+0000");
    assert!(
        store
            .save_publication_feedback(id(&run), 9, &next, "2026-09-29T00:00:00Z")
            .is_err()
    );
    assert_eq!(
        store.publication(id(&run)).unwrap().unwrap()["feedback"],
        old
    );
    db.execute_batch("DROP TRIGGER fail_feedback").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let saved = reopened
        .save_publication_feedback(id(&run), 9, &next, "2026-09-29T00:00:00Z")
        .unwrap();
    assert_eq!(saved["comments"][0]["content"]["raw"], "Newer");
    assert_eq!(
        reopened
            .save_publication_feedback(id(&run), 9, &next, "2026-09-29T00:00:00Z")
            .unwrap(),
        saved
    );
}
fn review(id: i64, state: &str, body: &str) -> Value {
    json!({"id":id,"kind":"review","created_on":"2026-09-28T01:00:00Z","updated_on":"2026-09-28T01:00:00Z","deleted":false,"content":{"raw":body},"user":{"uuid":"github:7","display_name":"reviewer-fixture"},"review":{"state":state,"commit":"a".repeat(40)}})
}
#[test]
fn github_reviews_are_feedback_with_their_own_identity_and_reobserve_without_duplicates() {
    let (_root, store, run) = setup();
    store.save_publication(&publication(&run)).unwrap();
    // Review 1 shares its number with conversation comment 1: separate GitHub id spaces.
    let mut read = comments("Conversation note");
    read["comments"].as_array_mut().unwrap().push(review(
        1,
        "CHANGES_REQUESTED",
        "Add a regression test",
    ));
    let first = store
        .save_publication_feedback(id(&run), 9, &read, "2026-09-28T02:00:00Z")
        .unwrap();
    assert_eq!(first["state"], "complete");
    let items = first["comments"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["content"]["raw"], "Conversation note");
    assert_eq!(items[1]["kind"], "review");
    assert_eq!(items[1]["review"]["state"], "CHANGES_REQUESTED");
    assert_eq!(items[1]["currentness"], "known");

    // Re-observation adds nothing new.
    let again = store
        .save_publication_feedback(id(&run), 9, &read, "2026-09-28T03:00:00Z")
        .unwrap();
    assert_eq!(again["comments"].as_array().unwrap().len(), 2);
    assert_eq!(again["state"], "complete");

    // GitHub keeps no edit time for a review: a dismissal or an edited summary
    // replaces the retained version instead of reading as a conflict. The same
    // change to a versioned comment is still a conflict.
    let mut changed = comments("Conversation note, edited without a new time");
    changed["comments"].as_array_mut().unwrap().push(review(
        1,
        "DISMISSED",
        "Add a regression test, please",
    ));
    let next = store
        .save_publication_feedback(id(&run), 9, &changed, "2026-09-28T04:00:00Z")
        .unwrap();
    let items = next["comments"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(next["state"], "partial");
    assert_eq!(items[0]["currentness"], "conflict");
    assert_eq!(items[0]["content"]["raw"], "Conversation note");
    assert_eq!(items[1]["currentness"], "known");
    assert_eq!(items[1]["review"]["state"], "DISMISSED");
    assert_eq!(items[1]["content"]["raw"], "Add a regression test, please");

    // A review missing from a later read keeps its history with unknown currentness.
    let gone = store
        .save_publication_feedback(
            id(&run),
            9,
            &comments("Conversation note"),
            "2026-09-28T05:00:00Z",
        )
        .unwrap();
    let items = gone["comments"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["kind"], "review");
    assert_eq!(items[1]["currentness"], "unknown");
}
#[test]
fn failed_sync_receipt_rolls_back_filtered_retained_and_run_changes() {
    let (root, store, _) = setup();
    let source = source();
    store
        .commit_jira_sync(
            &source,
            &json!([issue("Original", "2026-09-28")]),
            &json!([]),
        )
        .unwrap();
    let task = store.snapshot().unwrap()["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["sourceId"] == "1")
        .unwrap()
        .clone();
    store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Retain local work","criteria":["Keep history"]})).unwrap();
    store
        .queue_refinement(id(&task), &json!({"revision":2,"purpose":"explore"}))
        .unwrap();
    let before = store.snapshot().unwrap();
    let state = store.jira_state().unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_sync BEFORE INSERT ON jira_sync BEGIN SELECT RAISE(ABORT,'sync interrupted'); END").unwrap();
    let retained = json!([issue("Changed retained source", "2026-09-29")]);
    let mut fresh = issue("New filtered source", "2026-09-29");
    fresh["id"] = json!("2");
    fresh["key"] = json!("FIX-2");
    assert!(
        store
            .commit_jira_sync(&source, &json!([fresh.clone()]), &retained)
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(store.jira_state().unwrap(), state);
    db.execute_batch("DROP TRIGGER fail_sync").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    reopened
        .commit_jira_sync(&source, &json!([fresh]), &retained)
        .unwrap();
    let task = reopened.get_task(id(&task)).unwrap();
    assert_eq!(task["brief"], "Retain local work");
    assert_eq!(task["needsReacceptance"], true);
    assert_eq!(
        reopened.jira_state().unwrap().unwrap()["taskIds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn confirmed_jira_receipt_survives_failed_notice_and_retry_deduplicates() {
    let (root, store, run) = setup();
    let update = json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run["id"],"eventKey":"milestone","state":"confirmed","externalId":"42"});
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_notice BEFORE INSERT ON delivery_notices BEGIN SELECT RAISE(ABORT,'notice interrupted'); END").unwrap();
    assert!(store.save_jira_update(&update).is_err());
    assert_eq!(
        store.jira_updates(Some(id(&run))).unwrap(),
        vec![update.clone()]
    );
    assert!(store.delivery_notices(id(&run)).unwrap().is_empty());
    db.execute_batch("DROP TRIGGER fail_notice").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    for _ in 0..2 {
        reopened.save_jira_update(&update).unwrap();
    }
    let notices = reopened.delivery_notices(id(&run)).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["payload"]["state"], "confirmed");
    assert_eq!(notices[0]["payload"]["externalId"], "42");
}
#[test]
fn concurrent_sync_observations_cannot_replace_a_later_source_with_an_older_one() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    store
        .commit_jira_sync(
            &source(),
            &json!([issue("Original", "2026-09-28")]),
            &json!([]),
        )
        .unwrap();
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let threads = [
        ("Earlier", "2026-09-29T00:00:00+0000"),
        ("Latest", "Wed, 30 Sep 2026 00:00:00 GMT"),
    ]
    .into_iter()
    .map(|(title, date)| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let store = Store::open(&path).unwrap();
            barrier.wait();
            store.commit_jira_sync(&source(), &json!([issue(title, date)]), &json!([]))
        })
    })
    .collect::<Vec<_>>();
    assert!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(Result::is_ok)
            .count()
            >= 1
    );
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.snapshot().unwrap()["tasks"][0]["title"], "Latest");
    assert_eq!(
        reopened.jira_state().unwrap().unwrap()["state"],
        "connected"
    );
}
