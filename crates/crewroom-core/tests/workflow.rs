use crewroom_core::store::Store;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use uuid::Uuid;

fn key() -> String {
    Uuid::new_v4().to_string()
}
fn setup() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    (root, store)
}
fn task(store: &Store, title: &str) -> Value {
    store
        .create_task(&json!({"title":title,"description":""}))
        .unwrap()
}
fn accept(store: &Store, task: &Value, criterion: &str) -> Value {
    store.refine(task["id"].as_str().unwrap(), &json!({"revision":task["revision"],"sourceRevision":task["sourceRevision"],"brief":"Investigate 日本語 🦀","criteria":[criterion]})).unwrap()
}
fn run_id(run: &Value) -> &str {
    run["id"].as_str().unwrap()
}

#[test]
fn dependency_decisions_are_optimistic_acyclic_and_independent_of_requirements() {
    let (root, store) = setup();
    let a = task(&store, "A");
    let b = task(&store, "B");
    let c = task(&store, "C");
    let decision = json!({"requestId":key(),"dependencyRevision":0,"dependsOn":[b["id"]]});
    let saved = store.set_dependencies(run_id(&a), &decision).unwrap();
    assert_eq!(saved["revision"], a["revision"]);
    let other = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        other.set_dependencies(run_id(&a), &decision).unwrap(),
        saved
    );
    for deps in [json!([a["id"]]), json!([key()]), json!([b["id"], b["id"]])] {
        assert!(
            other
                .set_dependencies(
                    run_id(&a),
                    &json!({"requestId":key(),"dependencyRevision":1,"dependsOn":deps})
                )
                .is_err()
        );
    }
    assert!(
        other
            .set_dependencies(
                run_id(&a),
                &json!({"requestId":key(),"dependencyRevision":0,"dependsOn":[]})
            )
            .is_err()
    );
    store
        .set_dependencies(
            run_id(&b),
            &json!({"requestId":key(),"dependencyRevision":0,"dependsOn":[c["id"]]}),
        )
        .unwrap();
    assert!(
        other
            .set_dependencies(
                run_id(&c),
                &json!({"requestId":key(),"dependencyRevision":0,"dependsOn":[a["id"]]})
            )
            .is_err()
    );
    assert!(
        store
            .get_task(run_id(&c))
            .unwrap()
            .get("dependsOn")
            .is_none()
    );
}

#[test]
fn alternate_plans_close_without_execution_and_replay_across_reopen() {
    let (root, store) = setup();
    let original = task(&store, "Explore");
    let request = json!({"requestId":key(),"revision":1,"purpose":"explore"});
    let run = store.queue_refinement(run_id(&original), &request).unwrap();
    assert!(run.get("position").is_none());
    assert!(
        store
            .queue_refinement(
                run_id(&original),
                &json!({"requestId":key(),"revision":1,"purpose":"grill"})
            )
            .is_err()
    );
    assert!(
        store
            .queue(run_id(&original), &json!({"revision":1}))
            .is_err()
    );
    let instruction = store
        .steer(
            run_id(&run),
            &json!({"requestId":"note","body":"Consider edge cases"}),
        )
        .unwrap();
    assert_eq!(instruction["delivery"], "pending");
    let closed = store.close_refinement(run_id(&run)).unwrap();
    assert_eq!(closed["status"], "closed");
    assert_eq!(
        store.snapshot().unwrap()["messages"][0]["delivery"],
        "not_delivered"
    );
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        reopened
            .queue_refinement(run_id(&original), &request)
            .unwrap(),
        closed
    );
    let accepted = accept(&reopened, &original, "Find alternatives");
    let research = reopened
        .queue_research(
            run_id(&original),
            &json!({"requestId":key(),"revision":accepted["revision"]}),
        )
        .unwrap();
    assert_eq!(research["status"], "awaiting_runner");
    assert!(research.get("researchSource").is_none());
    assert_eq!(reopened.queue_view().unwrap()["revision"], 0);
    assert_eq!(
        reopened.runtime(run_id(&research)).unwrap()["attempts"],
        json!([])
    );
}

#[test]
fn adoption_requires_latest_nonempty_acceptance_and_is_atomic_on_failure() {
    let (root, store) = setup();
    let original = accept(&store, &task(&store, "Research"), "Original");
    let run = store
        .queue_research(
            run_id(&original),
            &json!({"requestId":key(),"revision":original["revision"]}),
        )
        .unwrap();
    let accepted = accept(&store, &original, "Revised");
    let request = json!({"requestId":key(),"revision":accepted["revision"]});
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    // A real second-write failure must roll the first assignment edit back too.
    let assignment = json!({"id":key(),"runId":run["id"],"name":"Reader","coordinator":false,"brief":{"criteria":[{"index":0,"criterion":"Original"}]}});
    db.execute(
        "INSERT INTO assignments VALUES(?,?,?,?)",
        params![
            assignment["id"].as_str(),
            run_id(&run),
            "Reader",
            assignment.to_string()
        ],
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER fail_run BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
    assert!(store.adopt_requirements(run_id(&run), &request).is_err());
    assert_eq!(
        store.runtime(run_id(&run)).unwrap()["assignments"][0],
        assignment
    );
    assert_eq!(store.get_run(run_id(&run)).unwrap(), run);
    db.execute_batch("DROP TRIGGER fail_run").unwrap();
    let adopted = store.adopt_requirements(run_id(&run), &request).unwrap();
    assert_eq!(adopted["taskSnapshot"], accepted);
    assert_eq!(
        store.runtime(run_id(&run)).unwrap()["assignments"][0]["brief"],
        assignment["brief"]
    );
    assert_eq!(
        store.runtime(run_id(&run)).unwrap()["assignments"][0]["rebriefRequired"]["criterionIndices"],
        json!([0])
    );
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        reopened.adopt_requirements(run_id(&run), &request).unwrap(),
        adopted
    );
    assert!(reopened.adopt_requirements(run_id(&run),&json!({"reason":"Different","requestId":request["requestId"],"revision":accepted["revision"]})).is_err());
    assert_eq!(
        reopened
            .requirement_revisions(run_id(&original))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn research_pins_target_commit_without_mutating_checkout_and_retries_without_git() {
    let (root, store) = setup();
    let checkout = root.path().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&checkout)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
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
        "base",
    ]);
    let expected = git(&["rev-parse", "HEAD"]);
    let repo = store
        .create_repository(&json!({"name":"Fixture","path":checkout,"targetBranch":"main"}))
        .unwrap();
    let original = store
        .create_task(&json!({"title":"Research target","description":"","repositoryId":repo["id"]}))
        .unwrap();
    let accepted = accept(&store, &original, "Read pinned source");
    git(&["checkout", "-b", "operator"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "operator work",
    ]);
    let head = git(&["rev-parse", "HEAD"]);
    let request = json!({"requestId":key(),"revision":accepted["revision"]});
    let run = store.queue_research(run_id(&original), &request).unwrap();
    assert_eq!(run["researchSource"]["commit"], expected);
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    assert_eq!(git(&["branch", "--show-current"]), "operator");
    std::fs::remove_dir_all(&checkout).unwrap();
    assert_eq!(
        store.queue_research(run_id(&original), &request).unwrap(),
        run
    );
}

#[test]
fn completed_research_reopens_only_without_another_open_run() {
    let (root, store) = setup();
    let original = accept(&store, &task(&store, "Research"), "Original");
    let run = store
        .queue_research(
            run_id(&original),
            &json!({"requestId":key(),"revision":original["revision"]}),
        )
        .unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    let mut completed = run.clone();
    completed["status"] = json!("completed");
    completed["desiredState"] = json!("cancelled");
    completed["researchCompletion"] = json!({"requestId":"historical"});
    db.execute(
        "UPDATE runs SET data=? WHERE id=?",
        params![completed.to_string(), run_id(&run)],
    )
    .unwrap();
    let accepted = accept(&store, &original, "Revised");
    let another = store
        .queue_refinement(
            run_id(&original),
            &json!({"revision":accepted["revision"],"purpose":"grill"}),
        )
        .unwrap();
    let request = json!({"requestId":key(),"revision":accepted["revision"]});
    assert!(store.adopt_requirements(run_id(&run), &request).is_err());
    store.close_refinement(run_id(&another)).unwrap();
    let adopted = store.adopt_requirements(run_id(&run), &request).unwrap();
    assert_eq!(adopted["status"], "paused");
    assert_eq!(adopted["desiredState"], "paused");
    assert!(adopted.get("researchCompletion").is_none());
    assert_eq!(store.snapshot().unwrap()["connections"]["runner"], false);
}

#[test]
fn dependency_delivery_requires_a_recorded_merge_not_jira_or_pr_readiness() {
    let (root, store) = setup();
    let upstream = task(&store, "Upstream");
    // Retained provider states are fixtures; this test performs no provider writes.
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    let run = json!({"id":key(),"taskId":upstream["id"],"purpose":"coding","status":"paused"});
    db.execute(
        "INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",
        params![run["id"].as_str(), upstream["id"].as_str(), run.to_string()],
    )
    .unwrap();
    let mut publication = json!({"runId":run["id"],"pr":{"state":"confirmed","providerState":"OPEN"},"push":{"state":"confirmed"},"ready":true});
    for (provider, state) in [
        ("OPEN", "open"),
        ("DECLINED", "closed"),
        ("SUPERSEDED", "closed"),
        ("MERGED", "unknown"),
    ] {
        publication["pr"]["providerState"] = json!(provider);
        db.execute(
            "INSERT OR REPLACE INTO publications VALUES(?,?)",
            params![run["id"].as_str(), publication.to_string()],
        )
        .unwrap();
        assert_eq!(
            store.dependency_states().unwrap()[run_id(&upstream)]["state"],
            state
        );
    }
    publication["merged"] =
        json!({"commit":"verified-merge","observedAt":"2026-10-02T00:00:00.000Z"});
    db.execute(
        "UPDATE publications SET data=? WHERE run_id=?",
        params![publication.to_string(), run["id"].as_str()],
    )
    .unwrap();
    assert_eq!(
        store.dependency_states().unwrap()[run_id(&upstream)]["state"],
        "merged"
    );
}

#[test]
fn retained_ownership_and_terminal_publications_block_unsafe_decisions() {
    let (root, store) = setup();
    let original = accept(&store, &task(&store, "Retained"), "Original");
    let run = store
        .queue_refinement(
            run_id(&original),
            &json!({"purpose":"explore","revision":2}),
        )
        .unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    let assignment = json!({"id":key(),"runId":run["id"],"coordinator":true});
    let attempt =
        json!({"id":key(),"assignmentId":assignment["id"],"runId":run["id"],"state":"starting"});
    db.execute(
        "INSERT INTO assignments VALUES(?,?,?,?)",
        params![
            assignment["id"].as_str(),
            run_id(&run),
            "Coordinator",
            assignment.to_string()
        ],
    )
    .unwrap();
    db.execute(
        "INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)",
        params![
            attempt["id"].as_str(),
            assignment["id"].as_str(),
            attempt.to_string()
        ],
    )
    .unwrap();
    assert!(
        store
            .close_refinement(run_id(&run))
            .unwrap_err()
            .to_string()
            .contains("ownership")
    );
    let accepted = accept(&store, &original, "New");
    let request = json!({"requestId":key(),"revision":accepted["revision"]});
    assert!(
        store
            .adopt_requirements(run_id(&run), &request)
            .unwrap_err()
            .to_string()
            .contains("reason")
    );
    assert_eq!(store.get_run(run_id(&run)).unwrap(), run);
    let mut ended = run.clone();
    ended["status"] = json!("cancelled");
    ended["desiredState"] = json!("cancelled");
    db.execute(
        "UPDATE runs SET data=? WHERE id=?",
        params![ended.to_string(), run_id(&run)],
    )
    .unwrap();
    assert!(
        store
            .adopt_requirements(run_id(&run), &request)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    db.execute(
        "UPDATE runs SET data=? WHERE id=?",
        params![run.to_string(), run_id(&run)],
    )
    .unwrap();
    db.execute(
        "INSERT INTO publications VALUES(?,?)",
        params![
            run_id(&run),
            json!({"runId":run["id"],"merged":{"commit":"fixture"}}).to_string()
        ],
    )
    .unwrap();
    assert!(
        store
            .adopt_requirements(run_id(&run), &request)
            .unwrap_err()
            .to_string()
            .contains("merged")
    );
}
