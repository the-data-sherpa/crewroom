use crewroom_core::store::Store;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
const NOW: i64 = 1791000123456;
const AT: &str = "2026-10-01T00:00:00.000Z";
fn fixture(version: u32) -> (tempfile::TempDir, PathBuf, Connection) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    let catalog: Value =
        serde_json::from_str(include_str!("fixtures/historical-schemas.json")).unwrap();
    db.execute_batch(catalog["schemas"][version.to_string()].as_str().unwrap())
        .unwrap();
    (root, path, db)
}
fn ticket(db: &Connection, n: u32) {
    db.execute("INSERT INTO tickets VALUES(?,NULL,NULL,?,?)",params![format!("task-{n}"),json!({"id":format!("task-{n}"),"sourceRevision":7,"updatedAt":AT,"title":"Retained source"}).to_string(),json!({"revision":3,"brief":"Retained requirements","criteria":["Preserve work"],"needsReacceptance":true}).to_string()]).unwrap();
}
fn run(db: &Connection, n: u32, status: &str) {
    ticket(db, n);
    db.execute("INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",params![format!("run-{n}"),format!("task-{n}"),json!({"id":format!("run-{n}"),"taskId":format!("task-{n}"),"status":status,"desiredState":"running","createdAt":AT,"taskSnapshot":{"sourceRevision":5},"activeMs":0}).to_string()]).unwrap();
}
fn attempt(db: &Connection, n: u32, ownership: &str) {
    db.execute("INSERT INTO assignments VALUES(?,?,?,?)",params![format!("assignment-{n}"),format!("run-{n}"),"Coordinator",json!({"id":format!("assignment-{n}"),"runId":format!("run-{n}"),"currentAttemptId":format!("attempt-{n}"),"coordinator":true}).to_string()]).unwrap();
    db.execute("INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)",params![format!("attempt-{n}"),format!("assignment-{n}"),json!({"id":format!("attempt-{n}"),"runId":format!("run-{n}"),"assignmentId":format!("assignment-{n}"),"state":"idle","ownership":ownership}).to_string()]).unwrap();
}
fn work(db: &Connection, n: u32, refreshes: Value) {
    db.execute("INSERT INTO repository_work VALUES(?,?,?,?)",params![format!("run-{n}"),"/disposable/.git",format!("branch-{n}"),json!({"runId":format!("run-{n}"),"path":format!("/disposable/work-{n}"),"branch":format!("branch-{n}"),"refreshes":refreshes}).to_string()]).unwrap();
}
fn open(path: &Path) -> Store {
    Store::open_with_clock(path, || NOW).unwrap()
}
#[test]
fn earliest_schema_retains_local_requirements_and_unknown_accepted_source() {
    let (_root, path, db) = fixture(1);
    ticket(&db, 1);
    drop(db);
    let store = open(&path);
    assert_eq!(
        store.get_task("task-1").unwrap()["brief"],
        "Retained requirements"
    );
    drop(store);
    let db = Connection::open(&path).unwrap();
    let record: Value = serde_json::from_str(
        &db.query_row("SELECT data FROM requirement_revisions", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(record["sourceRevision"], 6);
    assert_eq!(record["createdAt"], AT);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        31
    );
}
#[test]
fn completed_message_compaction_keeps_cursor_holes_and_autoincrement_high_watermark() {
    let (_root, path, db) = fixture(14);
    run(&db, 1, "running");
    attempt(&db, 1, "stopped");
    for (cursor, seq, data) in [
        (
            2,
            2,
            json!({"type":"message_update","messageId":"done","phase":"text_delta"}),
        ),
        (
            5,
            5,
            json!({"type":"message_end","messageId":"done","message":{"id":"done","content":[]}}),
        ),
        (
            80,
            4,
            json!({"type":"message_update","messageId":"done","phase":"text_delta"}),
        ),
        (
            17,
            9,
            json!({"type":"message_update","messageId":"incomplete","phase":"text_delta"}),
        ),
    ] {
        db.execute(
            "INSERT INTO agent_events(rowid,attempt_id,sequence,data) VALUES(?, 'attempt-1',?,?)",
            params![cursor, seq, data.to_string()],
        )
        .unwrap();
    }
    drop(db);
    let store = open(&path);
    let page = store.agent_event_page("run-1", 0, 20).unwrap();
    assert_eq!(
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["cursor"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![5, 17]
    );
    store
        .record_agent_event("attempt-1", 100, &json!({"type":"fixture"}))
        .unwrap();
    assert_eq!(
        store.agent_event_page("run-1", 17, 20).unwrap()["events"][0]["cursor"],
        81
    );
    drop(store);
    assert_eq!(
        open(&path).agent_event_page("run-1", 17, 20).unwrap()["events"][0]["cursor"],
        81
    );
}
#[test]
fn slot_upgrade_keeps_first_claimed_owner_and_holds_other_retained_owners() {
    let (_root, path, db) = fixture(20);
    for n in 1..=3 {
        run(&db, n, "running");
        attempt(&db, n, if n == 3 { "stopped" } else { "held" });
    }
    run(&db, 4, "running");
    attempt(&db, 4, "held");
    db.execute(
        "INSERT INTO publications VALUES('run-4',?)",
        [
            json!({"runId":"run-4","merged":{"commit":"retained-merge","observedAt":AT}})
                .to_string(),
        ],
    )
    .unwrap();
    drop(db);
    let store = open(&path);
    let first = store.get_run("run-1").unwrap();
    assert_eq!(first["schedule"]["at"], AT);
    assert_eq!(first["launchCompletedAt"], AT);
    assert_eq!(
        uuid::Uuid::parse_str(first["schedule"]["id"].as_str().unwrap())
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(store.get_run("run-4").unwrap()["status"], "running");
    assert!(store.get_run("run-4").unwrap().get("slotRelease").is_none());
    let second = store.get_run("run-2").unwrap();
    assert_eq!(second["status"], "attention_required");
    assert_eq!(second["desiredState"], "paused");
    assert_eq!(second["slotRelease"]["at"], crewroom_core::timestamp(NOW));
    assert_eq!(store.get_run("run-3").unwrap()["status"], "paused");
    assert_eq!(store.get_attempt("attempt-2").unwrap()["ownership"], "held");
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT run_id FROM coding_slot", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "run-1"
    );
    assert_eq!(
        db.query_row("SELECT revision FROM queue_state", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
fn retry(n: u32) -> Value {
    json!([{"requestId":format!("old-{n}"),"state":"conflicted","supersededBy":{"requestId":format!("retry-{n}"),"reason":"Inspect and retry","at":AT}},{"requestId":format!("retry-{n}"),"state":"interrupted","at":AT,"supersedes":{"requestId":format!("old-{n}"),"reason":"Inspect and retry"}}])
}
#[test]
fn malformed_late_retry_rolls_back_every_earlier_hold_in_that_version() {
    let (_root, path, db) = fixture(21);
    for n in 1..=2 {
        run(&db, n, "running");
        let mut refreshes = retry(n);
        if n == 2 {
            refreshes[1]["supersedes"]["reason"] = json!("mismatched");
        }
        work(&db, n, refreshes);
    }
    let before: Vec<String> = db
        .prepare("SELECT data FROM repository_work ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    drop(db);
    let error = Store::open_with_clock(&path, || NOW)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("ambiguous"));
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        21
    );
    let after: Vec<String> = db
        .prepare("SELECT data FROM repository_work ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM runs WHERE status='running'",
            [],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        2
    );
}
#[test]
fn legacy_mutation_hold_preserves_workspace_and_unknown_process_ownership() {
    let (_root, path, db) = fixture(22);
    run(&db, 1, "running");
    attempt(&db, 1, "held");
    work(
        &db,
        1,
        json!([{"requestId":"legacy","state":"conflicted","resolutionWorkspace":{"state":"operator_required","path":"/retained/workspace","promotionProcess":{"token":"unproven"}}}]),
    );
    drop(db);
    let store = open(&path);
    let work = store.repository_work("run-1").unwrap().unwrap();
    assert_eq!(
        work["refreshes"][0]["resolutionWorkspace"]["legacyOwnershipUnknown"],
        true
    );
    assert_eq!(
        work["refreshes"][0]["resolutionWorkspace"]["path"],
        "/retained/workspace"
    );
    assert_eq!(
        work["refreshes"][0]["resolutionWorkspace"]["promotionProcess"]["token"],
        "unproven"
    );
    assert_eq!(
        store.get_run("run-1").unwrap()["status"],
        "attention_required"
    );
    assert_eq!(store.get_attempt("attempt-1").unwrap()["ownership"], "held");
}
#[test]
fn transferred_history_does_not_claim_the_current_branch_after_upgrade() {
    let (_root, path, db) = fixture(25);
    run(&db, 1, "cancelled");
    run(&db, 2, "running");
    work(&db, 1, json!([]));
    db.execute(
        "UPDATE repository_work SET data=json_set(data,'$.transferredTo','run-2')",
        [],
    )
    .unwrap();
    drop(db);
    drop(open(&path));
    let db = Connection::open(&path).unwrap();
    db.execute("INSERT INTO repository_work VALUES('run-2','/disposable/.git','branch-1','{\"runId\":\"run-2\"}')",[]).unwrap();
    assert!(db.execute("UPDATE repository_work SET data=json_remove(data,'$.transferredTo') WHERE run_id='run-1'",[]).is_err());
}
#[test]
fn newer_and_nonempty_unversioned_databases_are_refused_without_adopting_tables() {
    for version in [0, 32] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workflow.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE foreign_data (value TEXT); INSERT INTO foreign_data VALUES('preserve');",
        )
        .unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        drop(db);
        assert!(Store::open(&path).is_err());
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT value FROM foreign_data", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "preserve"
        );
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            version
        );
    }
}
