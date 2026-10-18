use crewroom_core::{learning::Learning, roles::Roles, store::Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    os::unix::fs::MetadataExt,
    sync::{Arc, Barrier},
};

fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn setup() -> (tempfile::TempDir, Store, Roles, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Learning recovery","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(id(&task), &json!({"revision":1,"purpose":"explore"}))
        .unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    (root, store, roles, id(&run).to_owned())
}
// Trusted proposal fixtures isolate the persistence boundary. The compatibility
// oracle separately verifies run-owned evidence and captured target versions.
fn proposal(store: &Store, run: &str, target: Value, base: u64) -> Value {
    store.propose_learning(&json!({"runId":run,"attemptId":"fixture-attempt","toolCallId":uuid::Uuid::new_v4().to_string(),"target":target,"baseVersion":base,"change":"Approved instructions","rationale":"Recorded correction","evidence":[{"kind":"steering","id":"fixture-evidence"}]})).unwrap()
}
fn decision() -> Value {
    json!({"decision":"approve","requestId":uuid::Uuid::new_v4().to_string(),"reason":"Reviewed evidence"})
}
fn pending(store: &Store, proposal: &Value) {
    store.decide_learning(id(proposal), &json!({"status":"approved","requestId":uuid::Uuid::new_v4().to_string(),"reason":"Reviewed evidence","pendingRoleSave":true})).unwrap();
}
fn reviewer(roles: &Roles) -> Value {
    roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "reviewer")
        .unwrap()
}
fn seed_repository(db: &Connection) {
    db.execute(
        "INSERT INTO repositories VALUES(?,?)",
        params![
            "fixture-repository",
            json!({"id":"fixture-repository","guidance":[]}).to_string()
        ],
    )
    .unwrap();
}
#[test]
fn project_guidance_and_decision_roll_back_together() {
    let (root, store, roles, run) = setup();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    seed_repository(&db);
    let p = proposal(
        &store,
        &run,
        json!({"kind":"project","repositoryId":"fixture-repository"}),
        0,
    );
    db.execute_batch("CREATE TRIGGER fail_decision BEFORE UPDATE ON learning_proposals BEGIN SELECT RAISE(ABORT,'interrupted decision'); END").unwrap();
    let input = decision();
    assert!(
        Learning::new(&store, &roles)
            .decide(id(&p), &input)
            .is_err()
    );
    assert_eq!(
        store.get_learning_proposal(id(&p)).unwrap()["status"],
        "pending"
    );
    let data: String = db
        .query_row("SELECT data FROM repositories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&data).unwrap()["guidance"],
        json!([])
    );
    db.execute_batch("DROP TRIGGER fail_decision").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let applied = Learning::new(&reopened, &roles)
        .decide(id(&p), &input)
        .unwrap();
    assert_eq!(applied["decision"]["appliedVersion"], 1);
    assert_eq!(
        Learning::new(&reopened, &roles)
            .decide(id(&p), &input)
            .unwrap(),
        applied
    );
}
#[test]
fn failed_approval_never_writes_an_unapproved_role() {
    let (root, store, roles, run) = setup();
    let p = proposal(&store, &run, json!({"kind":"role","roleId":"reviewer"}), 1);
    let original = reviewer(&roles);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_approval BEFORE UPDATE ON learning_proposals BEGIN SELECT RAISE(ABORT,'interrupted approval'); END").unwrap();
    assert!(
        Learning::new(&store, &roles)
            .decide(id(&p), &decision())
            .is_err()
    );
    assert_eq!(reviewer(&roles), original);
    assert!(!roles.path().exists());
    assert_eq!(
        store.get_learning_proposal(id(&p)).unwrap()["status"],
        "pending"
    );
}
#[test]
fn filesystem_failure_retains_approval_and_retry_uses_the_original_decision() {
    let (root, store, _, run) = setup();
    // A regular file in the parent path gives a deterministic real I/O failure,
    // including when tests run as root; no chmod-dependent simulation.
    let blocked = root.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let roles = Roles::new(blocked.join("roles.json"));
    let p = proposal(&store, &run, json!({"kind":"role","roleId":"reviewer"}), 1);
    let input = decision();
    assert!(
        Learning::new(&store, &roles)
            .decide(id(&p), &input)
            .is_err()
    );
    let saved = store.get_learning_proposal(id(&p)).unwrap();
    assert_eq!(saved["status"], "approved");
    assert_eq!(saved["decision"]["pendingRoleSave"], true);
    assert_eq!(
        std::fs::read_to_string(&blocked).unwrap(),
        "not a directory"
    );
    std::fs::remove_file(blocked).unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let applied = Learning::new(&reopened, &roles)
        .decide(id(&p), &decision())
        .unwrap();
    assert_eq!(applied["decision"]["requestId"], input["requestId"]);
    assert_eq!(applied["decision"]["appliedVersion"], 2);
    assert!(applied["decision"].get("pendingRoleSave").is_none());
    assert_eq!(
        std::fs::metadata(roles.path()).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_dir(roles.path().parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    Learning::new(&reopened, &roles).reconcile().unwrap();
    assert_eq!(reviewer(&roles)["revision"], 2);
}
#[test]
fn restart_after_real_catalog_save_recognizes_provenance_without_incrementing_again() {
    let (root, store, roles, run) = setup();
    let p = proposal(&store, &run, json!({"kind":"role","roleId":"reviewer"}), 1);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_completion BEFORE UPDATE ON learning_proposals WHEN json_extract(OLD.data,'$.decision.pendingRoleSave')=1 AND json_extract(NEW.data,'$.decision.pendingRoleSave') IS NULL BEGIN SELECT RAISE(ABORT,'lost completion'); END").unwrap();
    assert!(
        Learning::new(&store, &roles)
            .decide(id(&p), &decision())
            .is_err()
    );
    assert_eq!(
        store.get_learning_proposal(id(&p)).unwrap()["decision"]["pendingRoleSave"],
        true
    );
    assert_eq!(reviewer(&roles)["revision"], 2);
    assert_eq!(reviewer(&roles)["learningProposalId"], p["id"]);
    let saved = std::fs::read(roles.path()).unwrap();
    db.execute_batch("DROP TRIGGER fail_completion").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    for _ in 0..2 {
        Learning::new(&reopened, &roles).reconcile().unwrap();
    }
    assert_eq!(std::fs::read(roles.path()).unwrap(), saved);
    assert_eq!(
        reopened.get_learning_proposal(id(&p)).unwrap()["decision"]["appliedVersion"],
        2
    );
}
#[test]
fn pending_approval_cannot_overwrite_a_later_operator_edit() {
    let (root, store, roles, run) = setup();
    let p = proposal(&store, &run, json!({"kind":"role","roleId":"reviewer"}), 1);
    pending(&store, &p);
    let mut edit = reviewer(&roles);
    edit["revision"] = json!(1);
    edit["instructions"] = json!("Keep the operator correction");
    edit["learningProposalId"] = p["id"].clone();
    roles.update("reviewer", &edit).unwrap();
    let saved = std::fs::read(roles.path()).unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    Learning::new(&reopened, &roles).reconcile().unwrap();
    let result = reopened.get_learning_proposal(id(&p)).unwrap();
    assert_eq!(result["status"], "superseded");
    assert!(result["decision"]["notApplied"].is_string());
    assert_eq!(std::fs::read(roles.path()).unwrap(), saved);
    assert!(reviewer(&roles).get("learningProposalId").is_none());
}
#[test]
fn competing_project_approvals_have_one_winner_across_connections() {
    let (root, store, roles, run) = setup();
    let path = root.path().join("workflow.sqlite");
    seed_repository(&Connection::open(&path).unwrap());
    let proposals = (0..2)
        .map(|_| {
            proposal(
                &store,
                &run,
                json!({"kind":"project","repositoryId":"fixture-repository"}),
                0,
            )
        })
        .collect::<Vec<_>>();
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let threads = proposals
        .into_iter()
        .map(|p| {
            let path = path.clone();
            let roles = Roles::new(roles.path());
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let s = Store::open(&path).unwrap();
                barrier.wait();
                Learning::new(&s, &roles).decide(id(&p), &decision())
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
    let reopened = Store::open(&path).unwrap();
    let proposals = reopened.learning_proposals().unwrap();
    assert_eq!(
        proposals
            .iter()
            .filter(|p| p["status"] == "approved")
            .count(),
        1
    );
    assert_eq!(
        proposals
            .iter()
            .filter(|p| p["status"] == "pending")
            .count(),
        1
    );
}
