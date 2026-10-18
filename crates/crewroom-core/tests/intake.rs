use crewroom_core::store::Store;
use serde_json::{Value, json};
use std::process::Command;
fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn fixture() -> (tempfile::TempDir, Store, Value, Value) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "fixture@example.test"],
        vec!["config", "user.name", "Fixture"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    std::fs::write(repo.join("AGENTS.md"), "Run the checks.\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "."])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["commit", "-m", "fixture"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let repository = store
        .create_repository(&json!({"name":"Fixture","path":repo,"targetBranch":"main"}))
        .unwrap();
    let task=store.create_task(&json!({"title":"Plan this","description":"Add behavior","repositoryId":id(&repository)})).unwrap();
    let task=store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Add behavior","criteria":["Works","Is checked"]})).unwrap();
    let source = store
        .save_plan_source(
            &store
                .prepare_plan_source(id(&task), 2)
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    (root, store, task, source)
}
fn content() -> Value {
    json!({"schemaVersion":1,"goal":"Add behavior","scope":["Implement"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"code","name":"Implement","dependsOn":[],"outputs":["Patch"],"criteria":[0],"validation":["Tests"]},{"id":"check","name":"Validate","dependsOn":["code"],"outputs":["Evidence"],"criteria":[1],"validation":["Review"]}]})
}
fn draft(source: &Value) -> Value {
    json!({"requestId":key(),"revision":2,"sourceId":source["id"],"content":content()})
}
fn decision(plan: &Value) -> Value {
    json!({"requestId":key(),"revision":plan["basis"]["taskRevision"],"sourceId":plan["basis"]["id"],"decision":"accept"})
}
#[test]
fn saved_idea_and_plan_drafts_never_start_runs_and_receipts_survive_reopen() {
    let (root, store, task, source) = fixture();
    let input = draft(&source);
    let plan = store.save_implementation_plan(id(&task), &input).unwrap();
    assert!(
        store.snapshot().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .current_implementation_plan(id(&task))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.save_implementation_plan(id(&task), &input).unwrap(),
        plan
    );
    let mut conflicting = input.clone();
    conflicting["content"]["goal"] = json!("Different");
    assert!(
        store
            .save_implementation_plan(id(&task), &conflicting)
            .is_err()
    );
    let approval = decision(&plan);
    let result = store
        .decide_implementation_plan(id(&plan), &approval)
        .unwrap();
    drop(store);
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        store
            .decide_implementation_plan(id(&plan), &approval)
            .unwrap(),
        result
    );
    assert_eq!(
        store
            .current_implementation_plan(id(&task))
            .unwrap()
            .unwrap()["id"],
        plan["id"]
    );
    assert!(
        store.snapshot().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn schema_rejects_cycles_dangling_ids_uncovered_and_duplicate_packages() {
    for mutate in 0..4 {
        let mut c = content();
        match mutate {
            0 => c["workPackages"][0]["dependsOn"] = json!(["check"]),
            1 => c["workPackages"][1]["dependsOn"] = json!(["missing"]),
            2 => c["workPackages"][1]["criteria"] = json!([0]),
            _ => c["workPackages"][1]["id"] = json!("code"),
        };
        assert!(
            crewroom_core::intake::plan::validate(&c, &[json!("Works"), json!("Is checked")])
                .is_err()
        );
    }
}
#[test]
fn source_and_requirement_drift_block_acceptance_but_notes_do_not() {
    let (_root, store, task, source) = fixture();
    let plan = store
        .save_implementation_plan(id(&task), &draft(&source))
        .unwrap();
    store
        .add_note(id(&task), &json!({"mode":"decision","body":"A note"}))
        .unwrap();
    store
        .decide_implementation_plan(id(&plan), &decision(&plan))
        .unwrap();
    assert!(
        store
            .current_implementation_plan(id(&task))
            .unwrap()
            .is_some()
    );
    store.refine(id(&task),&json!({"revision":2,"sourceRevision":1,"brief":"Different scope","criteria":["Works","Is checked"]})).unwrap();
    assert!(
        store
            .current_implementation_plan(id(&task))
            .unwrap()
            .is_none()
    );
    assert_eq!(store.implementation_plans(id(&task)).unwrap().len(), 1);
}
#[test]
fn atomic_requirements_adoption_binds_acceptance_to_the_new_revision() {
    let (root, store, task, source) = fixture();
    let mut input = draft(&source);
    input["requirements"] = json!({"brief":"Refined scope","criteria":["Works","Is checked"]});
    let plan = store.save_implementation_plan(id(&task), &input).unwrap();
    let db = rusqlite::Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_plan BEFORE INSERT ON plan_decisions BEGIN SELECT RAISE(ABORT,'lost acceptance'); END").unwrap();
    let approval = decision(&plan);
    assert!(
        store
            .decide_implementation_plan(id(&plan), &approval)
            .is_err()
    );
    assert_eq!(store.get_task(id(&task)).unwrap()["revision"], 2);
    db.execute_batch("DROP TRIGGER fail_plan").unwrap();
    store
        .decide_implementation_plan(id(&plan), &approval)
        .unwrap();
    let current = store
        .current_implementation_plan(id(&task))
        .unwrap()
        .unwrap();
    assert_eq!(current["decision"]["basis"]["taskRevision"], 3);
    assert_eq!(store.get_task(id(&task)).unwrap()["brief"], "Refined scope");
}
#[test]
fn migration_adds_only_empty_records_and_rejects_forged_source_basis() {
    let (root, store, task, source) = fixture();
    drop(store);
    let db = rusqlite::Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("DROP TABLE comparison_requests; DROP TABLE comparison_events; DROP TABLE comparison_cells; DROP TABLE comparisons; DROP TABLE choreography_revisions; DROP TABLE plan_decisions; DROP TABLE implementation_plans; DROP TABLE plan_sources; DROP TABLE intake_requests; PRAGMA user_version=27;").unwrap();
    drop(db);
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert!(store.implementation_plans(id(&task)).unwrap().is_empty());
    assert!(
        store.snapshot().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .save_implementation_plan(id(&task), &draft(&source))
            .is_err()
    );
}
#[test]
fn current_plan_skips_grill_unplanned_queue_starts_only_scoping() {
    let (_root, store, task, source) = fixture();
    let plan = store
        .save_implementation_plan(id(&task), &draft(&source))
        .unwrap();
    store
        .decide_implementation_plan(id(&plan), &decision(&plan))
        .unwrap();
    let run = store
        .queue(id(&task), &json!({"revision":2,"requestId":key()}))
        .unwrap();
    assert_eq!(run["purpose"], "coding");
    assert_eq!(run["implementationPlan"]["id"], plan["id"]);
    assert!(store.intakes().unwrap().is_empty());
    let (_root, store, task, _) = fixture();
    let input = json!({"revision":2,"requestId":key()});
    let run = store.queue(id(&task), &input).unwrap();
    assert_eq!(run["purpose"], "grill");
    assert_eq!(store.queue(id(&task), &input).unwrap()["id"], run["id"]);
    assert!(store.coding_slot().unwrap().is_none());
    assert!(
        store.runtime(id(&run)).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn handoff_is_atomic_replayable_and_cancellation_revokes_acceptance() {
    let (root, store, task, source) = fixture();
    let run = store
        .queue(id(&task), &json!({"revision":2,"requestId":key()}))
        .unwrap();
    let intake = store.active_intake(id(&task)).unwrap().unwrap();
    let plan = store
        .save_implementation_plan(id(&task), &draft(&source))
        .unwrap();
    store
        .decide_implementation_plan(id(&plan), &decision(&plan))
        .unwrap();
    let db = rusqlite::Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_handoff BEFORE INSERT ON runs WHEN json_extract(NEW.data,'$.purpose')='coding' BEGIN SELECT RAISE(ABORT,'handoff interrupted'); END").unwrap();
    assert!(store.handoff_intake(id(&intake), &[], &[]).is_err());
    assert_eq!(
        store.get_run(id(&run)).unwrap()["status"],
        "awaiting_runner"
    );
    db.execute_batch("DROP TRIGGER fail_handoff").unwrap();
    let coding = store.handoff_intake(id(&intake), &[], &[]).unwrap();
    drop(store);
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        store.handoff_intake(id(&intake), &[], &[]).unwrap()["id"],
        coding["id"]
    );
    assert_eq!(store.get_run(id(&run)).unwrap()["status"], "closed");
    assert_eq!(
        store.snapshot().unwrap()["runs"].as_array().unwrap().len(),
        2
    );
    let (_root, store, task, source) = fixture();
    let run = store
        .queue(id(&task), &json!({"revision":2,"requestId":key()}))
        .unwrap();
    let intake = store.active_intake(id(&task)).unwrap().unwrap();
    let plan = store
        .save_implementation_plan(id(&task), &draft(&source))
        .unwrap();
    store
        .cancel_intake(
            id(&intake),
            &json!({"revision":intake["revision"],"requestId":key()}),
        )
        .unwrap();
    // A manual draft remains usable for a future explicit request, but cannot release the revoked intake.
    store
        .decide_implementation_plan(id(&plan), &decision(&plan))
        .unwrap();
    assert!(store.handoff_intake(id(&intake), &[], &[]).is_err());
    assert_eq!(
        store.get_run(id(&run)).unwrap()["desiredState"],
        "cancelled"
    );
}
#[test]
fn stopped_question_wait_survives_restart_without_budget_charge_or_duplicate_answer() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let (root, store, task, source) = fixture();
    let time = Arc::new(AtomicI64::new(1000));
    let clock = time.clone();
    let store = store.with_clock(move || clock.load(Ordering::SeqCst));
    let run = store
        .queue(id(&task), &json!({"revision":2,"requestId":key()}))
        .unwrap();
    let role = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let admission=store.admit_assignment(id(&run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Scope"})).unwrap();
    let attempt = id(&admission["attempt"]);
    store.claim_attempt(attempt).unwrap();
    store.record_attempt_identity(attempt,&json!({"hostPid":123,"ompPid":124,"sessionId":"fixture","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
    store.set_desired_state(id(&run), "running").unwrap();
    store.start_execution(id(&run)).unwrap();
    let questions = json!({"sourceId":source["id"],"questions":[{"id":"choice","prompt":"Which behavior?","rationale":"Choose the scope","choices":["A","B"]}]});
    assert!(
        store
            .ask_intake_questions(attempt, "questions", &questions)
            .is_err()
    );
    store
        .record_intake_inspection(attempt, id(&source))
        .unwrap();
    time.store(2000, Ordering::SeqCst);
    let intake = store
        .ask_intake_questions(attempt, "questions", &questions)
        .unwrap();
    assert_eq!(
        store
            .ask_intake_questions(attempt, "questions", &questions)
            .unwrap(),
        intake
    );
    store.begin_pause(id(&run), Some("intake")).unwrap();
    assert_ne!(store.finish_pause(id(&run)).unwrap()["status"], "paused");
    time.store(3000, Ordering::SeqCst);
    store
        .record_ownership(attempt, true, Some("fixture stopped proof"))
        .unwrap();
    store.finish_pause(id(&run)).unwrap();
    let used = store.get_run(id(&run)).unwrap()["activeMs"].clone();
    drop(store);
    time.store(1_000_000, Ordering::SeqCst);
    let clock = time.clone();
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), move || {
        clock.load(Ordering::SeqCst)
    })
    .unwrap();
    assert_eq!(store.get_run(id(&run)).unwrap()["activeMs"], used);
    assert_eq!(
        store.intake_view(id(&task)).unwrap()["state"],
        "Waiting for you"
    );
    let answer = json!({"requestId":key(),"revision":intake["revision"],"sourceId":source["id"],"answers":[{"id":"choice","body":"A"}]});
    let result = store.answer_intake(id(&intake), &answer).unwrap();
    assert_eq!(store.answer_intake(id(&intake), &answer).unwrap(), result);
    assert_eq!(store.steering(id(&run)).unwrap().len(), 1);
    assert_eq!(store.steering(id(&run)).unwrap()[0]["delivery"], "pending");
    assert_eq!(store.get_run(id(&run)).unwrap()["activeMs"], used);
}

#[test]
fn pinned_source_and_instruction_movement_hold_new_admission_but_keep_owned_history() {
    let (root, store, task, source) = fixture();
    let plan = store
        .save_implementation_plan(id(&task), &draft(&source))
        .unwrap();
    store
        .decide_implementation_plan(id(&plan), &decision(&plan))
        .unwrap();
    let run = store.queue(id(&task), &json!({"revision":2})).unwrap();
    let role = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let assignment = json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Implement accepted plan"});
    let repo = root.path().join("repo");
    std::fs::write(repo.join("AGENTS.md"), "Changed project instructions\n").unwrap();
    for args in [
        ["add", "AGENTS.md"].as_slice(),
        ["commit", "-m", "Updated instructions"].as_slice(),
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let changed = store
        .save_plan_source(
            &store
                .prepare_plan_source(id(&task), 2)
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    assert_ne!(changed["instructionsHash"], source["instructionsHash"]);
    assert!(
        store
            .implementation_plan_hold(&run)
            .unwrap()
            .unwrap()
            .contains("source moved")
    );
    assert!(store.admit_assignment(id(&run), &assignment).is_err());
    assert!(store.claim_coding_slot(id(&run), false).is_err());
    assert!(
        store
            .decide_implementation_plan(
                id(&plan),
                &json!({"requestId":key(),"revision":2,"sourceId":source["id"],"decision":"accept"})
            )
            .is_err()
    );
    // Restore the original source, admit, then show later drift cannot reinterpret
    // the already-admitted assignment's historical plan.
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["checkout", "-B", "main", source["commit"].as_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    store
        .save_plan_source(
            &store
                .prepare_plan_source(id(&task), 2)
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    let admitted = store.admit_assignment(id(&run), &assignment).unwrap();
    store.refine(id(&task),&json!({"revision":2,"sourceRevision":1,"brief":"New scope","criteria":["Works","Is checked"]})).unwrap();
    assert!(
        store
            .implementation_plan_hold(&store.get_run(id(&run)).unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        admitted["assignment"]["implementationPlan"]["id"],
        plan["id"]
    );
}

#[test]
fn legacy_unadmitted_queue_is_held_until_explicit_acceptance_and_queue_order_survives_equal_timestamps()
 {
    let (root, store, task, source) = fixture();
    let store = store.with_clock(|| 1000);
    let grill = store.queue(id(&task), &json!({"revision":2})).unwrap();
    let intake = store.active_intake(id(&task)).unwrap().unwrap();
    let second = store
        .create_task(&json!({"title":"Later","description":"","repositoryId":task["repositoryId"]}))
        .unwrap();
    let second=store.refine(id(&second),&json!({"revision":1,"sourceRevision":1,"brief":"Second","criteria":["Works","Is checked"]})).unwrap();
    let basis = store
        .save_plan_source(
            &store
                .prepare_plan_source(id(&second), 2)
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    let p = store
        .save_implementation_plan(id(&second), &draft(&basis))
        .unwrap();
    store
        .decide_implementation_plan(id(&p), &decision(&p))
        .unwrap();
    let later = store.queue(id(&second), &json!({"revision":2})).unwrap();
    let p = store
        .save_implementation_plan(id(&task), &draft(&source))
        .unwrap();
    store
        .decide_implementation_plan(id(&p), &decision(&p))
        .unwrap();
    let coding = store.handoff_intake(id(&intake), &[], &[]).unwrap();
    let queue = store.queue_view().unwrap();
    assert_eq!(queue["plans"][0]["id"], coding["id"]);
    assert_eq!(queue["plans"][1]["id"], later["id"]);
    assert_eq!(store.get_run(id(&grill)).unwrap()["status"], "closed");
    let db = rusqlite::Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute(
        "UPDATE runs SET data=json_remove(data,'$.implementationPlan') WHERE id=?",
        [id(&later)],
    )
    .unwrap();
    let legacy = store.get_run(id(&later)).unwrap();
    assert!(
        store
            .implementation_plan_hold(&legacy)
            .unwrap()
            .unwrap()
            .contains("accepted implementation plan")
    );
    assert_eq!(
        store.intake_view(id(&second)).unwrap()["state"],
        "Needs plan"
    );
    let mut input = draft(&basis);
    input["predecessorId"] = p["id"].clone();
    // Select the second task's own latest immutable version.
    input["predecessorId"] = store
        .implementation_plans(id(&second))
        .unwrap()
        .last()
        .unwrap()["id"]
        .clone();
    let accepted = store.save_implementation_plan(id(&second), &input).unwrap();
    store
        .decide_implementation_plan(id(&accepted), &decision(&accepted))
        .unwrap();
    assert!(
        store
            .implementation_plan_hold(&store.get_run(id(&later)).unwrap())
            .unwrap()
            .is_none()
    );
}
