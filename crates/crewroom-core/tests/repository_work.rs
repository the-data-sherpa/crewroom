mod support;
use crewroom_core::store::Store;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Barrier};
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    run: Value,
    work: Value,
    checkpoint: Value,
    members: Vec<Value>,
}
impl Fixture {
    fn db(&self) -> Connection {
        Connection::open(self.root.path().join("workflow.sqlite")).unwrap()
    }
    fn patch(&self, table: &str, key_field: &str, key_value: &str, changes: Value) {
        let db = self.db();
        let mut v: Value = serde_json::from_str(
            &db.query_row(
                &format!("SELECT data FROM {table} WHERE {key_field}=?"),
                [key_value],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        v.as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        db.execute(
            &format!("UPDATE {table} SET data=? WHERE {key_field}=?"),
            params![v.to_string(), key_value],
        )
        .unwrap();
    }
    fn claim(&self, member: usize) -> Value {
        json!({"id":key(),"writerAttemptId":self.members[member]["attempt"]["id"],"coordinatorAttemptId":self.members[0]["attempt"]["id"],"toolCallId":key(),"instruction":"Implement scoped work","automatic":true})
    }
    fn activate(&self) {
        self.patch(
            "runs",
            "id",
            id(&self.run),
            json!({"status":"running","desiredState":"running"}),
        );
        for m in &self.members {
            self.patch(
                "attempts",
                "id",
                id(&m["attempt"]),
                json!({"state":"idle","ownership":"held"}),
            );
        }
    }
}
fn setup() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap().with_clock(|| 1000);
    // Explicit repository identity fixture; these tests exercise records and SQLite failure recovery only.
    let repo = json!({"id":key(),"name":"Fixture","path":root.path().join("checkout"),"targetBranch":"main","revision":1,"checks":[]});
    let db = Connection::open(&path).unwrap();
    db.execute(
        "INSERT INTO repositories VALUES(?,?)",
        params![id(&repo), repo.to_string()],
    )
    .unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-q",
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
    let task = store
        .create_task(&json!({"title":"Ownership","description":"","repositoryId":repo["id"]}))
        .unwrap();
    store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Scoped work","criteria":["Fresh proof"]})).unwrap();
    support::accept_plan(&store, id(&task));
    let run = store.queue(id(&task), &json!({"revision":2})).unwrap();
    let role: Value = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let members=(0..3).map(|i|store.admit_assignment(id(&run),&json!({"name":format!("Member{i}"),"coordinator":i==0,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Recorded fixture"})).unwrap()).collect();
    let revision = json!({"head":"b".repeat(40),"base":"a".repeat(40),"tree":"c".repeat(40),"worktreeHash":"d".repeat(64),"requirementsHash":"e".repeat(64),"instructionsHash":format!("{:x}",Sha256::digest(b"[]")),"dirty":false});
    let checkpoint =
        json!({"id":key(),"at":"1970-01-01T00:00:01.000Z","reason":"prepared","revision":revision});
    let work = json!({"runId":run["id"],"repository":repo,"commonDir":root.path().join("common.git"),"path":root.path().join("owned"),"branch":"crewroom/owned","targetRef":"refs/heads/main","mode":"new","base":revision["base"],"startHead":revision["head"],"requirementsHash":revision["requirementsHash"],"state":"preparing","instructions":[],"checkpoints":[]});
    store.reserve_repository_work(&work).unwrap();
    let work = store
        .confirm_repository_work(id(&run), "fixture-git", &json!([]), &checkpoint)
        .unwrap();
    store.update_coding_record(id(&run),|_|Ok(json!({"runId":run["id"],"candidates":[{"id":"source","revision":revision}],"requiredChecks":[],"checks":[],"reviews":[],"claims":[],"completions":[]}))).unwrap();
    Fixture {
        root,
        store,
        run,
        work,
        checkpoint,
        members,
    }
}
#[test]
fn interrupted_writer_receipt_rolls_back_repair_counter_claim_and_checkpoint() {
    let f = setup();
    f.activate();
    let db = f.db();
    let writer = &f.members[1];
    let claim = f.claim(1);
    let before = f.store.get_assignment(id(&writer["assignment"])).unwrap();
    let coding = f.store.coding_record(id(&f.run)).unwrap();
    db.execute_batch("CREATE TRIGGER fail_writer BEFORE UPDATE ON repository_work BEGIN SELECT RAISE(ABORT,'writer interrupted'); END").unwrap();
    assert!(
        f.store
            .claim_repository_writer(id(&writer["attempt"]), &f.checkpoint, Some(&claim))
            .is_err()
    );
    assert_eq!(
        f.store.get_assignment(id(&writer["assignment"])).unwrap(),
        before
    );
    assert_eq!(f.store.coding_record(id(&f.run)).unwrap(), coding);
    assert_eq!(
        f.store.repository_work(id(&f.run)).unwrap(),
        Some(f.work.clone())
    );
    db.execute_batch("DROP TRIGGER fail_writer").unwrap();
    let reopened = Store::open(&f.root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    let saved = reopened
        .claim_repository_writer(id(&writer["attempt"]), &f.checkpoint, Some(&claim))
        .unwrap();
    assert_eq!(saved["writer"]["attemptId"], writer["attempt"]["id"]);
    assert_eq!(
        reopened.get_assignment(id(&writer["assignment"])).unwrap()["automaticCycles"],
        1
    );
    reopened
        .claim_repository_writer(id(&writer["attempt"]), &f.checkpoint, Some(&claim))
        .unwrap();
    assert_eq!(
        reopened.get_assignment(id(&writer["assignment"])).unwrap()["automaticCycles"],
        1
    );
}
#[test]
fn concurrent_writer_claims_grant_only_one_writer_and_one_repair() {
    let f = setup();
    f.activate();
    let path = f.root.path().join("workflow.sqlite");
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (1..=2)
        .map(|i| {
            let (path, barrier, attempt, checkpoint, claim) = (
                path.clone(),
                barrier.clone(),
                id(&f.members[i]["attempt"]).to_owned(),
                f.checkpoint.clone(),
                f.claim(i),
            );
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                store.claim_repository_writer(&attempt, &checkpoint, Some(&claim))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        f.store.coding_record(id(&f.run)).unwrap().unwrap()["claims"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let used: u64 = f.members[1..]
        .iter()
        .map(|m| {
            f.store.get_assignment(id(&m["assignment"])).unwrap()["automaticCycles"]
                .as_u64()
                .unwrap_or(0)
        })
        .sum();
    assert_eq!(used, 1);
}
#[test]
fn concurrent_reservations_keep_one_current_branch_owner() {
    let f = setup();
    let mut records = Vec::new();
    for i in 0..2 {
        let task=f.store.create_task(&json!({"title":format!("Reservation{i}"),"description":"","repositoryId":f.work["repository"]["id"]})).unwrap();
        f.store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Reserve","criteria":["One owner"]})).unwrap();
        support::accept_plan(&f.store, id(&task));
        let run = f.store.queue(id(&task), &json!({"revision":2})).unwrap();
        let mut record = f.work.clone();
        record["runId"] = run["id"].clone();
        record["branch"] = json!("crewroom/raced");
        record["base"] = run["implementationPlan"]["decision"]["basis"]["commit"].clone();
        records.push(record);
    }
    let barrier = Arc::new(Barrier::new(2));
    let path = f.root.path().join("workflow.sqlite");
    let threads: Vec<_> = records
        .into_iter()
        .map(|record| {
            let (path, barrier) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                store.reserve_repository_work(&record)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        f.db()
            .query_row(
                "SELECT count(*) FROM repository_work WHERE branch='crewroom/raced'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
fn terminal_fixture(f: &Fixture) -> (Value, Value, Value) {
    f.patch("runs","id",id(&f.run),json!({"status":"paused","desiredState":"running","codingTransition":{"completionId":"completion","phase":"stopping"}}));
    for member in &f.members {
        f.patch(
            "attempts",
            "id",
            id(&member["attempt"]),
            json!({"state":"exited","ownership":"stopped"}),
        );
    }
    let revision = &f.checkpoint["revision"];
    let workspace = json!({"path":"fixture-isolated","branch":"fixture-resolution","gitFile":"fixture-isolated-git","state":"promoting","operationId":"completion","toHead":"f".repeat(40)});
    let refresh = json!({"requestId":"refresh","at":"1970-01-01T00:00:01.000Z","state":"conflicted","sourceCandidateId":"source","fromHead":revision["head"],"fromBase":revision["base"],"toBase":"0".repeat(40),"targetRef":"refs/crewroom/target","resolutionWorkspace":workspace});
    f.patch(
        "repository_work",
        "run_id",
        id(&f.run),
        json!({"refreshes":[refresh]}),
    );
    let completion = json!({"id":"completion","attemptId":f.members[1]["attempt"]["id"],"state":"requested","conflictRefreshId":"refresh","head":revision["head"],"worktreeHash":revision["worktreeHash"]});
    f.patch("coding","run_id",id(&f.run),json!({"completions":[completion],"claims":[{"writerAttemptId":f.members[1]["attempt"]["id"],"targetRefreshId":"refresh"}]}));
    let proof = json!({"originalHead":revision["head"],"originalBase":revision["base"],"originalPath":f.work["path"],"originalBranch":f.work["branch"],"originalTargetRef":f.work["targetRef"],"targetRef":refresh["targetRef"],"originalGitFile":f.work["gitFile"],"workspaceGitFile":workspace["gitFile"],"workspacePath":workspace["path"],"workspaceBranch":workspace["branch"],"toHead":workspace["toHead"],"worktreeHash":revision["worktreeHash"],"requirementsHash":revision["requirementsHash"],"instructionsHash":revision["instructionsHash"],"expectedTarget":refresh["toBase"],"observedTarget":"1".repeat(40)});
    (proof, completion, refresh)
}
#[test]
fn failed_terminal_notice_restores_transition_completion_and_workspace_before_retry() {
    for rejected in [true, false] {
        let f = setup();
        let (proof, _, _) = terminal_fixture(&f);
        let before = f.store.get_run(id(&f.run)).unwrap();
        let coding = f.store.coding_record(id(&f.run)).unwrap();
        let work = f.store.repository_work(id(&f.run)).unwrap();
        let db = f.db();
        db.execute_batch("CREATE TRIGGER fail_notice BEFORE INSERT ON delivery_notices BEGIN SELECT RAISE(ABORT,'notice interrupted'); END").unwrap();
        let result = if rejected {
            f.store
                .reject_unpromoted_conflict(id(&f.run), "refresh", "completion", &proof)
        } else {
            f.store
                .interrupt_unpromoted_conflict(id(&f.run), "refresh", "completion", &proof)
        };
        assert!(result.is_err());
        assert_eq!(f.store.get_run(id(&f.run)).unwrap(), before);
        assert_eq!(f.store.coding_record(id(&f.run)).unwrap(), coding);
        assert_eq!(f.store.repository_work(id(&f.run)).unwrap(), work);
        assert!(f.store.delivery_notices(id(&f.run)).unwrap().is_empty());
        db.execute_batch("DROP TRIGGER fail_notice").unwrap();
        let reopened = Store::open(&f.root.path().join("workflow.sqlite"))
            .unwrap()
            .with_clock(|| 2000);
        let result = if rejected {
            reopened.reject_unpromoted_conflict(id(&f.run), "refresh", "completion", &proof)
        } else {
            reopened.interrupt_unpromoted_conflict(id(&f.run), "refresh", "completion", &proof)
        }
        .unwrap();
        assert_eq!(result["state"], "rejected");
        assert!(
            reopened
                .get_run(id(&f.run))
                .unwrap()
                .get("codingTransition")
                .is_none()
        );
        assert_eq!(
            reopened.repository_work(id(&f.run)).unwrap().unwrap()["refreshes"][0]["resolutionWorkspace"]
                ["state"],
            "operator_required"
        );
        assert_eq!(reopened.delivery_notices(id(&f.run)).unwrap().len(), 1);
    }
}
#[test]
fn failed_completed_refresh_cannot_move_base_or_append_partial_checkpoint() {
    let f = setup();
    let (_, _, refresh) = terminal_fixture(&f);
    let before = f.store.repository_work(id(&f.run)).unwrap().unwrap();
    let outcome = json!({"state":"completed","toHead":refresh["resolutionWorkspace"]["toHead"]});
    let mut checkpoint = f.checkpoint.clone();
    checkpoint["revision"]["base"] = refresh["toBase"].clone();
    checkpoint["revision"]["head"] = outcome["toHead"].clone();
    let db = f.db();
    db.execute_batch("CREATE TRIGGER fail_refresh BEFORE UPDATE ON repository_work BEGIN SELECT RAISE(ABORT,'refresh interrupted'); END").unwrap();
    assert!(
        f.store
            .finish_target_refresh(id(&f.run), "refresh", &outcome, Some(&checkpoint))
            .is_err()
    );
    assert_eq!(
        f.store.repository_work(id(&f.run)).unwrap().unwrap(),
        before
    );
    db.execute_batch("DROP TRIGGER fail_refresh").unwrap();
    let reopened = Store::open(&f.root.path().join("workflow.sqlite")).unwrap();
    let saved = reopened
        .finish_target_refresh(id(&f.run), "refresh", &outcome, Some(&checkpoint))
        .unwrap();
    assert_eq!(saved["base"], refresh["toBase"]);
    assert_eq!(saved["targetRef"], refresh["targetRef"]);
    assert_eq!(saved["checkpoints"].as_array().unwrap().len(), 2);
    assert_eq!(
        reopened
            .finish_target_refresh(id(&f.run), "refresh", &outcome, Some(&checkpoint))
            .unwrap(),
        saved
    );
}
#[test]
fn failed_retry_intent_cannot_mark_the_previous_conflict_superseded() {
    let f = setup();
    terminal_fixture(&f);
    f.patch("coding", "run_id", id(&f.run), json!({"completions":[]}));
    let mut work = f.store.repository_work(id(&f.run)).unwrap().unwrap();
    work["refreshes"][0]["resolutionWorkspace"]["state"] = json!("operator_required");
    f.db()
        .execute(
            "UPDATE repository_work SET data=? WHERE run_id=?",
            params![work.to_string(), id(&f.run)],
        )
        .unwrap();
    let refresh = json!({"requestId":"retry","at":"fixture-time","state":"rebasing","sourceCandidateId":"source","fromHead":f.checkpoint["revision"]["head"],"fromBase":f.checkpoint["revision"]["base"],"targetRef":"refs/retry","toBase":"f".repeat(40),"supersedes":{"requestId":"refresh","reason":"Retry after proof"}});
    let db = f.db();
    db.execute_batch("CREATE TRIGGER fail_retry BEFORE UPDATE ON repository_work BEGIN SELECT RAISE(ABORT,'retry interrupted'); END").unwrap();
    assert!(f.store.begin_target_refresh(id(&f.run), &refresh).is_err());
    assert_eq!(f.store.repository_work(id(&f.run)).unwrap().unwrap(), work);
    db.execute_batch("DROP TRIGGER fail_retry").unwrap();
    let reopened = Store::open(&f.root.path().join("workflow.sqlite")).unwrap();
    let saved = reopened.begin_target_refresh(id(&f.run), &refresh).unwrap();
    assert_eq!(saved["refreshes"][0]["supersededBy"]["requestId"], "retry");
    assert_eq!(saved["refreshes"].as_array().unwrap().len(), 2);
}
