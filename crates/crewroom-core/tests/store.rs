mod support;
use crewroom_core::{roles::Roles, store::Store};
use serde_json::{Value, json};

fn imported(title: &str) -> Value {
    json!({"siteUrl":"https://fixture.atlassian.net/","issues":[{"id":"100","key":"SIM-1","fields":{"summary":title,"description":"Imported requirement","status":{"name":"Backlog"}}}]})
}
fn setup() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    (root, store)
}
fn repository(store: &Store) -> (tempfile::TempDir, Value) {
    let root = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    git(&["init", "-b", "main"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "fixture",
    ]);
    let value = store
        .create_repository(
            &json!({"name":"Fixture","path":root.path(),"targetBranch":"main","checks":[]}),
        )
        .unwrap();
    (root, value)
}
fn ready(store: &Store) -> Value {
    let (_repo_root, repo) = repository(store);
    let task = store
        .create_task(
            &json!({"title":"Handle zero","description":"Local request","repositoryId":repo["id"]}),
        )
        .unwrap();
    let task=store.refine(task["id"].as_str().unwrap(),&json!({"revision":1,"sourceRevision":1,"brief":"Handle zero","criteria":["0 + 0 = 0"]})).unwrap();
    support::accept_plan(store, task["id"].as_str().unwrap());
    task
}
#[test]
fn jira_source_refresh_reopen_and_local_revision_are_independent() {
    let (root, store) = setup();
    let task = store.import_jira(&imported("Fix sum")).unwrap()[0].clone();
    let id = task["id"].as_str().unwrap();
    store.refine(id,&json!({"revision":1,"sourceRevision":1,"brief":"Handle zero","criteria":["0 + 0 = 0"]})).unwrap();
    store.import_jira(&imported("Fix arithmetic")).unwrap();
    let mut another = imported("Fix elsewhere");
    another["siteUrl"] = json!("https://other.atlassian.net");
    store.import_jira(&another).unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let saved = reopened.get_task(id).unwrap();
    assert_eq!(
        reopened.snapshot().unwrap()["tasks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(saved["title"], "Fix arithmetic");
    assert_eq!(saved["brief"], "Handle zero");
    assert_eq!(saved["criteria"], json!(["0 + 0 = 0"]));
    assert_eq!(saved["revision"], 2);
    assert_eq!(saved["sourceRevision"], 2);
    assert_eq!(saved["needsReacceptance"], true);
    assert!(
        reopened
            .refine(
                id,
                &json!({"revision":2,"sourceRevision":1,"brief":"Stale","criteria":["Old"]})
            )
            .is_err()
    );
    assert_eq!(reopened.requirement_revisions(id).unwrap().len(), 1);
}
#[test]
fn immutable_queue_and_steering_replay_survive_reopen() {
    let (root, store) = setup();
    let task = ready(&store);
    let task_id = task["id"].as_str().unwrap();
    let queued = store.queue(task_id, &json!({"revision":2})).unwrap();
    let run_id = queued["id"].as_str().unwrap();
    assert_eq!(
        store.queue(task_id, &json!({"revision":2})).unwrap()["id"],
        queued["id"]
    );
    assert_eq!(queued["status"], "awaiting_runner");
    assert_eq!(queued["position"], 0);
    store.refine(task_id,&json!({"revision":2,"sourceRevision":1,"brief":"New brief","criteria":["New criterion"]})).unwrap();
    assert_eq!(
        store.get_run(run_id).unwrap()["taskSnapshot"]["brief"],
        "Handle zero"
    );
    assert!(store.queue(task_id, &json!({"revision":3})).is_err());
    let body = json!({"requestId":"click-1","body":"Keep this small."});
    let message = store.steer(run_id, &body).unwrap();
    assert_eq!(store.steer(run_id, &body).unwrap(), message);
    assert!(
        store
            .steer(run_id, &json!({"requestId":"click-1","body":"Different"}))
            .is_err()
    );
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        reopened.snapshot().unwrap()["messages"][0]["delivery"],
        "pending"
    );
    assert_eq!(reopened.snapshot().unwrap()["connections"]["runner"], false);
    assert!(
        reopened.runtime(run_id).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn cancelling_unadmitted_work_settles_pending_messages_and_preserves_history() {
    let (_, store) = setup();
    let task = ready(&store);
    let run = store
        .queue(task["id"].as_str().unwrap(), &json!({"revision":2}))
        .unwrap();
    let id = run["id"].as_str().unwrap();
    store
        .steer(
            id,
            &json!({"requestId":"instruction","body":"Wait for clarification"}),
        )
        .unwrap();
    let decision = json!({"requestId":uuid::Uuid::new_v4()});
    let cancelled = store.cancel_queued(id, &decision).unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled.get("position").is_none());
    assert_eq!(store.cancel_queued(id, &decision).unwrap(), cancelled);
    assert_eq!(
        store.snapshot().unwrap()["messages"][0]["delivery"],
        "not_delivered"
    );
    assert!(
        store
            .steer(id, &json!({"requestId":"later","body":"Run again"}))
            .is_err()
    );
    assert!(
        store.queue_view().unwrap()["plans"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.get_run(id).unwrap()["taskSnapshot"], task);
}
#[test]
fn invalid_import_rolls_back_earlier_rows_and_future_schema_is_rejected() {
    let (root, store) = setup();
    let mut batch = imported("Original");
    batch["issues"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"200","key":"BAD KEY","fields":{}}));
    assert!(store.import_jira(&batch).is_err());
    assert!(
        store.snapshot().unwrap()["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(store);
    let path = root.path().join("workflow.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=35").unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
}
#[test]
fn user_role_revisions_are_optimistic_and_core_roles_stay_enabled() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let mut role = roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap();
    role["revision"] = json!(1);
    role["model"] = json!("fixture/model");
    roles.update("coordinator", &role).unwrap();
    assert!(roles.update("coordinator", &role).is_err());
    role["revision"] = json!(2);
    role["enabled"] = json!(false);
    assert!(roles.update("coordinator", &role).is_err());
    assert_eq!(
        roles
            .read()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "coordinator")
            .unwrap()["model"],
        "fixture/model"
    );
}
