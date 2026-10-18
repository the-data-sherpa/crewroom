//! Provider-neutral ticket source model and its schema-31 migration (FOR-40).
use crewroom_core::{store::Store, ticket_source};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::path::Path;

const SITE: &str = "https://fixture.atlassian.net";

fn jira(title: &str, status: &str) -> Value {
    json!({"siteUrl":format!("{SITE}/"),"issues":[{"id":"100","key":"SIM-1","fields":{"summary":title,"description":"Add numbers","status":{"id":"3","name":status},"priority":{"name":"High"},"project":{"name":"Simulated"}}}]})
}
fn version(path: &Path) -> u32 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
fn rows(path: &Path) -> Vec<(String, String, String)> {
    let db = Connection::open(path).unwrap();
    let mut q = db
        .prepare("SELECT id,imported,refinement FROM tickets ORDER BY rowid")
        .unwrap();
    q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

#[test]
fn jira_import_writes_neutral_source_fields_and_local_tickets_have_no_status() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let task = store.import_jira(&jira("Fix sum", "Backlog")).unwrap()[0].clone();
    for key in ["jiraStatus", "jiraStatusId"] {
        assert!(task.get(key).is_none(), "{key} is no longer written");
    }
    assert_eq!(task["source"], "jira");
    assert_eq!(task["sourceStatus"], "Backlog");
    assert_eq!(task["sourceStatusId"], "3");
    assert_eq!(task["sourceSite"], SITE);
    assert_eq!(task["sourceId"], "100");
    assert_eq!(task["key"], "SIM-1");
    assert_eq!(task["sourceUrl"], format!("{SITE}/browse/SIM-1"));
    assert_eq!(ticket_source::status_label(&task), "Jira Backlog");
    assert!(ticket_source::belongs_to(
        &task,
        ticket_source::JIRA,
        &json!(SITE)
    ));
    // The stored field order is unchanged apart from the neutral names.
    let keys: Vec<_> = task.as_object().unwrap().keys().take(9).cloned().collect();
    assert_eq!(
        keys,
        [
            "key",
            "title",
            "description",
            "sourceStatus",
            "priority",
            "project",
            "rank",
            "sourceStatusId",
            "id"
        ]
    );

    let local = store
        .create_task(&json!({"title":"Local work","description":""}))
        .unwrap();
    assert_eq!(local["source"], "local");
    assert!(local.get("sourceStatus").is_none() && local.get("jiraStatus").is_none());
    assert_eq!(ticket_source::status_label(&local), "Not linked");
    assert!(!ticket_source::belongs_to(
        &local,
        ticket_source::JIRA,
        &json!(SITE)
    ));
}

#[test]
fn status_change_alone_does_not_force_reacceptance_but_requirement_change_does() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let id = store.import_jira(&jira("Fix sum", "Backlog")).unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    store.refine(&id,&json!({"revision":1,"sourceRevision":1,"brief":"Handle zero","criteria":["0 + 0 = 0"]})).unwrap();
    let moved = store.import_jira(&jira("Fix sum", "In Progress")).unwrap()[0].clone();
    assert_eq!(moved["sourceStatus"], "In Progress");
    assert_eq!(moved["sourceRevision"], 1);
    assert_eq!(moved["needsReacceptance"], false);
    let changed = store
        .import_jira(&jira("Fix addition", "In Progress"))
        .unwrap()[0]
        .clone();
    assert_eq!(changed["sourceRevision"], 2);
    assert_eq!(changed["needsReacceptance"], true);
    assert_eq!(
        changed["brief"], "Handle zero",
        "the refinement is retained"
    );
}

/// Write the schema-30 shape of tickets into a store and mark it version 30.
fn legacy_state(path: &Path) {
    drop(Store::open(path).unwrap());
    let db = Connection::open(path).unwrap();
    let jira = json!({"key":"SIM-1","title":"Fix sum","description":"Add numbers","jiraStatus":"Backlog","priority":"High","project":"Simulated","rank":0,"jiraStatusId":"3","sourceUpdatedAt":"2026-10-01T00:00:00.000Z","id":"task-jira","source":"jira","sourceSite":SITE,"sourceId":"100","sourceUrl":format!("{SITE}/browse/SIM-1"),"sourceRevision":1,"updatedAt":"2026-10-01T00:00:00.000Z"});
    let local = json!({"id":"task-local","key":"LOCAL-1","title":"Local work","description":"","jiraStatus":"Not linked","priority":"Unspecified","source":"local","project":"","rank":0,"sourceRevision":1,"updatedAt":"2026-10-01T00:00:00.000Z"});
    let evaluation = json!({"id":"task-eval","key":"WORK-1","title":"Case","description":"p","source":"evaluation","internal":true,"jiraStatus":"Not linked","priority":"Unspecified","project":"","rank":0,"sourceRevision":1,"updatedAt":"2026-10-01T00:00:00.000Z"});
    let refined = json!({"revision":2,"brief":"Handle zero","criteria":["0 + 0 = 0"],"needsReacceptance":false});
    db.execute(
        "INSERT INTO tickets VALUES(?,?,?,?,?)",
        params![
            "task-jira",
            SITE,
            "100",
            jira.to_string(),
            refined.to_string()
        ],
    )
    .unwrap();
    for (id, value) in [("task-local", &local), ("task-eval", &evaluation)] {
        db.execute(
            "INSERT INTO tickets VALUES(?,NULL,NULL,?,?)",
            params![id, value.to_string(), refined.to_string()],
        )
        .unwrap();
    }
    // A historical run snapshot is an immutable record.
    db.execute("INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",params!["run-1","task-jira",json!({"id":"run-1","taskId":"task-jira","status":"completed","desiredState":"completed","purpose":"coding","activeMs":0,"taskSnapshot":jira}).to_string()]).unwrap();
    db.execute_batch("PRAGMA user_version=30").unwrap();
}

#[test]
fn schema_31_migrates_tickets_losslessly_and_whole_state_restore_reproduces_it() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let backup = root.path().join("backup");
    let path = live.join("workflow.sqlite");
    legacy_state(&path);
    let legacy = rows(&path);
    // Whole-state backup of the stopped schema-30 store.
    copy_dir(&live, &backup);

    let store = Store::open(&path).unwrap();
    let migrated: Vec<_> = ["task-jira", "task-local", "task-eval"]
        .iter()
        .map(|id| store.get_task(id).unwrap())
        .collect();
    drop(store);
    assert_eq!(version(&path), 32);
    let after = rows(&path);

    // Jira: renamed in place, every other field and the refinement unchanged.
    let old: Value = serde_json::from_str(&legacy[0].1).unwrap();
    let new: Value = serde_json::from_str(&after[0].1).unwrap();
    let rename = |k: &str| match k {
        "jiraStatus" => "sourceStatus".to_owned(),
        "jiraStatusId" => "sourceStatusId".to_owned(),
        k => k.to_owned(),
    };
    assert_eq!(
        old.as_object()
            .unwrap()
            .keys()
            .map(|k| rename(k))
            .collect::<Vec<_>>(),
        new.as_object().unwrap().keys().cloned().collect::<Vec<_>>()
    );
    for (k, v) in old.as_object().unwrap() {
        assert_eq!(&new[rename(k)], v, "{k}");
    }
    assert_eq!(ticket_source::status_label(&migrated[0]), "Jira Backlog");
    // Local and evaluation: the derived "Not linked" placeholder is dropped; the
    // label is reconstructed from the source kind.
    for (i, row) in after.iter().enumerate().skip(1) {
        let mut old: Value = serde_json::from_str(&legacy[i].1).unwrap();
        old.as_object_mut().unwrap().shift_remove("jiraStatus");
        assert_eq!(row.1, old.to_string());
        assert_eq!(ticket_source::status_label(&migrated[i]), "Not linked");
    }
    for (before, after) in legacy.iter().zip(&after) {
        assert_eq!(before.2, after.2, "refinements are untouched");
    }
    let snapshot: Value = serde_json::from_str(
        &Connection::open(&path)
            .unwrap()
            .query_row("SELECT data FROM runs WHERE id='run-1'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["taskSnapshot"]["jiraStatus"], "Backlog");

    // Restore the whole-state backup at its original path; reopening migrates it
    // to the identical result.
    std::fs::remove_dir_all(&live).unwrap();
    copy_dir(&backup, &live);
    assert_eq!(version(&path), 30);
    assert_eq!(rows(&path), legacy);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.get_task("task-jira").unwrap(), migrated[0]);
    drop(store);
    assert_eq!(rows(&path), after);

    // Jira behaviour continues on migrated records: a status move keeps the
    // accepted brief; a requirements change asks for re-acceptance.
    let store = Store::open(&path).unwrap();
    let moved = store.import_jira(&jira("Fix sum", "In Progress")).unwrap()[0].clone();
    assert_eq!(moved["id"], "task-jira");
    assert_eq!(moved["sourceStatus"], "In Progress");
    assert_eq!(moved["needsReacceptance"], false);
    assert_eq!(moved["sourceRevision"], 1);
    let changed = store
        .import_jira(&jira("Fix addition", "In Progress"))
        .unwrap()[0]
        .clone();
    assert_eq!(changed["needsReacceptance"], true);
    assert_eq!(
        changed["acceptedSource"],
        json!({"title":"Fix sum","description":"Add numbers"})
    );
}

#[test]
fn ambiguous_ticket_source_fails_the_migration_without_partial_changes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    legacy_state(&path);
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE tickets SET imported=json_set(imported,'$.sourceStatus','Done') WHERE id='task-local'",
        [],
    )
    .unwrap();
    drop(db);
    let before = rows(&path);
    assert!(Store::open(&path).is_err());
    assert_eq!(version(&path), 30);
    assert_eq!(rows(&path), before);
}
