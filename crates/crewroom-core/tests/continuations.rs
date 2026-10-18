mod support;
use crewroom_core::{repositories::validate_update, roles::Roles, store::Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn setup() -> (tempfile::TempDir, Store, Value, Value, Vec<Value>) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let checkout = root.path().join("repo");
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
            std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let store = Store::open(&path).unwrap().with_clock(|| 1000);
    let repo = store
        .create_repository(
            &json!({"name":"Continuation","path":checkout,"targetBranch":"main","checks":[]}),
        )
        .unwrap();
    let task = store
        .create_task(&json!({"title":"Retain work","description":"","repositoryId":repo["id"]}))
        .unwrap();
    store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Retain history","criteria":["New evidence"]})).unwrap();
    support::accept_plan(&store, id(&task));
    let mut run = store.queue(id(&task), &json!({"revision":2})).unwrap();
    run["status"] = json!("cancelled");
    run["desiredState"] = json!("cancelled");
    run.as_object_mut().unwrap().remove("position");
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE runs SET data=? WHERE id=?",
        params![run.to_string(), id(&run)],
    )
    .unwrap();
    // Recorded work and checkpoint are fixtures. Actual worktree/head verification is an N4 boundary.
    let checkpoint = json!({"id":key(),"at":"1970-01-01T00:00:01.000Z","reason":"interrupted","revision":{"head":"b".repeat(40),"base":"a".repeat(40),"worktreeHash":"c".repeat(64),"dirty":true}});
    let work = json!({"runId":run["id"],"repository":repo,"commonDir":checkout.join(".git"),"path":root.path().join("owned"),"branch":"crewroom/retained","targetRef":"refs/heads/main","mode":"new","base":"a".repeat(40),"startHead":"b".repeat(40),"requirementsHash":"fixture","state":"ready","instructions":[],"checkpoints":[checkpoint]});
    db.execute(
        "INSERT INTO repository_work VALUES(?,?,?,?)",
        params![
            id(&run),
            work["commonDir"].as_str(),
            work["branch"].as_str(),
            work.to_string()
        ],
    )
    .unwrap();
    let roles = Roles::new(root.path().join("roles.json")).read().unwrap();
    (root, store, run, work, roles)
}
fn request() -> Value {
    json!({"requestId":key(),"revision":2,"roleId":"coordinator"})
}
#[test]
fn failed_queue_receipt_rolls_back_successor_link_and_queue_then_reopens() {
    let (root, store, run, work, roles) = setup();
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    let before = store.snapshot().unwrap();
    let input = request();
    db.execute_batch("CREATE TRIGGER fail_request BEFORE INSERT ON queue_requests BEGIN SELECT RAISE(ABORT,'receipt interrupted'); END").unwrap();
    assert!(
        store
            .queue_cancelled_continuation(id(&run), &input, &roles, &work)
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), before);
    db.execute_batch("DROP TRIGGER fail_request").unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap().with_clock(|| 2000);
    let next = reopened
        .queue_cancelled_continuation(id(&run), &input, &roles, &work)
        .unwrap();
    assert_eq!(
        reopened.get_run(id(&run)).unwrap()["continuedBy"],
        next["id"]
    );
    assert_eq!(
        reopened
            .queue_cancelled_continuation(id(&run), &input, &roles, &work)
            .unwrap(),
        next
    );
}
#[test]
fn failed_new_ownership_insert_restores_the_only_current_branch_owner() {
    let (root, store, run, work, roles) = setup();
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    let next = store
        .queue_cancelled_continuation(id(&run), &request(), &roles, &work)
        .unwrap();
    db.execute_batch("CREATE TRIGGER fail_transfer BEFORE INSERT ON repository_work BEGIN SELECT RAISE(ABORT,'handoff interrupted'); END").unwrap();
    assert!(
        store
            .transfer_cancelled_work(
                id(&next),
                &work,
                &work["repository"],
                &work["checkpoints"][0],
                None
            )
            .is_err()
    );
    assert_eq!(store.repository_work(id(&run)).unwrap(), Some(work.clone()));
    assert!(store.repository_work(id(&next)).unwrap().is_none());
    assert_eq!(db.query_row("SELECT count(*) FROM repository_work WHERE json_extract(data,'$.transferredTo') IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    db.execute_batch("DROP TRIGGER fail_transfer").unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let saved = reopened
        .transfer_cancelled_work(
            id(&next),
            &work,
            &work["repository"],
            &work["checkpoints"][0],
            None,
        )
        .unwrap();
    assert_eq!(saved["path"], work["path"]);
    assert_eq!(
        reopened.repository_work(id(&run)).unwrap().unwrap()["transferredTo"],
        next["id"]
    );
    assert_eq!(db.query_row("SELECT count(*) FROM repository_work WHERE json_extract(data,'$.transferredTo') IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),1);
}
#[test]
fn failed_predecessor_unlink_cannot_partially_cancel_an_untransferred_successor() {
    let (root, store, run, work, roles) = setup();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    let next = store
        .queue_cancelled_continuation(id(&run), &request(), &roles, &work)
        .unwrap();
    let before = store.snapshot().unwrap();
    db.execute_batch(&format!("CREATE TRIGGER fail_unlink BEFORE UPDATE ON runs WHEN NEW.id='{}' BEGIN SELECT RAISE(ABORT,'unlink interrupted'); END",id(&run))).unwrap();
    let input = json!({"requestId":key()});
    assert!(store.cancel_queued(id(&next), &input).is_err());
    assert_eq!(store.snapshot().unwrap(), before);
    db.execute_batch("DROP TRIGGER fail_unlink").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    reopened.cancel_queued(id(&next), &input).unwrap();
    assert!(
        reopened
            .get_run(id(&run))
            .unwrap()
            .get("continuedBy")
            .is_none()
    );
}
#[test]
fn repository_edit_and_replay_receipt_commit_together() {
    let (root, store, _, work, _) = setup();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    let repo = &work["repository"];
    let update = json!({"name":"Updated","path":repo["path"],"targetBranch":"main","checks":[],"revision":1,"requestId":key()});
    let validated = validate_update(&update).unwrap();
    db.execute_batch("CREATE TRIGGER fail_edit BEFORE INSERT ON queue_requests BEGIN SELECT RAISE(ABORT,'edit receipt interrupted'); END").unwrap();
    assert!(
        store
            .update_repository(id(repo), &validated, &update)
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap()["repositories"][0]["revision"], 1);
    assert!(
        store
            .repository_update_replay(id(repo), &update)
            .unwrap()
            .is_none()
    );
    db.execute_batch("DROP TRIGGER fail_edit").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let saved = reopened
        .update_repository(id(repo), &validated, &update)
        .unwrap();
    assert_eq!(saved["revision"], 2);
    assert_eq!(
        reopened
            .repository_update_replay(id(repo), &update)
            .unwrap(),
        Some(saved)
    );
    assert_eq!(
        reopened
            .repository_work(work["runId"].as_str().unwrap())
            .unwrap()
            .unwrap()["repository"],
        *repo
    );
}
#[test]
fn two_connections_replay_one_successor_for_the_same_request() {
    let (root, store, run, work, roles) = setup();
    let path = root.path().join("workflow.sqlite");
    let input = request();
    let barrier = Arc::new(Barrier::new(2));
    drop(store);
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (path, input, barrier, run, work, roles) = (
                path.clone(),
                input.clone(),
                barrier.clone(),
                run.clone(),
                work.clone(),
                roles.clone(),
            );
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap().with_clock(|| 2000);
                barrier.wait();
                store
                    .queue_cancelled_continuation(id(&run), &input, &roles, &work)
                    .unwrap()
            })
        })
        .collect();
    let mut results = threads.into_iter().map(|t| t.join().unwrap());
    let first = results.next().unwrap();
    assert_eq!(Some(first.clone()), results.next());
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.snapshot().unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        reopened.get_run(id(&run)).unwrap()["continuedBy"],
        first["id"]
    );
}
#[test]
fn concurrent_repository_edits_preserve_one_revision_and_one_receipt() {
    let (root, store, _, work, _) = setup();
    let path = root.path().join("workflow.sqlite");
    let barrier = Arc::new(Barrier::new(2));
    drop(store);
    let threads:Vec<_>=(0..2).map(|n|{let (path,barrier,repo)=(path.clone(),barrier.clone(),work["repository"].clone());std::thread::spawn(move||{let store=Store::open(&path).unwrap();let input=json!({"name":format!("Edit {n}"),"path":repo["path"],"targetBranch":"main","revision":1,"requestId":key()});let validated=validate_update(&input).unwrap();barrier.wait();store.update_repository(id(&repo),&validated,&input)})}).collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let db = Connection::open(path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM queue_requests WHERE kind='repository-update'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
