use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};

fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn setup(research: bool) -> (tempfile::TempDir, Store, Value, Value) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let task = store
        .create_task(&json!({"title":"Outcome recovery","description":""}))
        .unwrap();
    let run = if research {
        store.refine(id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Explain a decision","criteria":["Explain findings"]})).unwrap();
        store
            .queue_research(
                id(&task),
                &json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()}),
            )
            .unwrap()
    } else {
        store
            .queue_refinement(id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap()
    };
    let role = serde_json::from_str::<Vec<Value>>(include_str!("../src/default-roles.json"))
        .unwrap()
        .remove(0);
    let admitted=store.admit_assignment(id(&run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Inspect recorded evidence"})).unwrap();
    let attempt = id(&admitted["attempt"]);
    store.claim_attempt(attempt).unwrap();
    // Stored identity fixture only: no PID is inspected and no process is started.
    store.record_attempt_identity(attempt,&json!({"hostPid":123,"ompPid":124,"sessionId":"fixture","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
    store.set_desired_state(id(&run), "running").unwrap();
    store.start_execution(id(&run)).unwrap();
    (root, store, run, admitted)
}
fn input() -> Value {
    json!({"brief":"Refined brief","criteria":["Explain findings"],"decisions":["Retain evidence"],"unresolvedQuestions":["Any limitations?"]})
}
fn decision() -> Value {
    json!({"decision":"accept","requestId":uuid::Uuid::new_v4().to_string()})
}
fn notes(store: &Store) -> Value {
    store.snapshot().unwrap()["notes"].clone()
}
fn artifact(run: &Value, admitted: &Value) -> Value {
    json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run["id"],"assignmentId":admitted["assignment"]["id"],"attemptId":admitted["attempt"]["id"],"toolCallId":uuid::Uuid::new_v4().to_string(),"title":"Decision","kind":"research","body":{"sha256":"fixture-unverified-body"},"requirementRevision":2,"citations":[],"coverage":[{"criterionId":0,"section":"Findings"}],"state":"verified","reasons":[],"limitations":"Store fixture, not verified source","createdAt":"1970-01-01T00:00:01.000Z"})
}
#[test]
fn second_note_failure_rolls_back_proposal_and_all_linked_notes() {
    let (root, store, _, admitted) = setup(false);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_note BEFORE INSERT ON notes WHEN json_extract(NEW.data,'$.kind')='unresolved_question' BEGIN SELECT RAISE(ABORT,'second note interrupted'); END").unwrap();
    let attempt = id(&admitted["attempt"]);
    assert!(
        store
            .propose_refinement(attempt, "proposal", &input())
            .is_err()
    );
    assert!(store.refinement_proposals(None).unwrap().is_empty());
    assert_eq!(notes(&store), json!([]));
    db.execute_batch("DROP TRIGGER fail_note").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let proposal = reopened
        .propose_refinement(attempt, "proposal", &input())
        .unwrap();
    assert_eq!(
        reopened
            .propose_refinement(attempt, "proposal", &input())
            .unwrap(),
        proposal
    );
    assert_eq!(notes(&reopened).as_array().unwrap().len(), 2);
}
#[test]
fn lost_acceptance_rolls_back_requirements_note_revisions_and_decision() {
    let (root, store, run, admitted) = setup(false);
    let proposal = store
        .propose_refinement(id(&admitted["attempt"]), "proposal", &input())
        .unwrap();
    let original_notes = notes(&store);
    let task_id = run["taskId"].as_str().unwrap();
    let original = store.get_task(task_id).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_decision BEFORE UPDATE ON refinement_proposals BEGIN SELECT RAISE(ABORT,'decision interrupted'); END").unwrap();
    let approval = decision();
    assert!(store.decide_refinement(id(&proposal), &approval).is_err());
    assert_eq!(store.get_task(task_id).unwrap(), original);
    assert!(store.requirement_revisions(task_id).unwrap().is_empty());
    assert_eq!(notes(&store), original_notes);
    assert_eq!(
        store.refinement_proposals(None).unwrap()[0]["status"],
        "pending"
    );
    db.execute_batch("DROP TRIGGER fail_decision").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 2000);
    let accepted = reopened
        .decide_refinement(id(&proposal), &approval)
        .unwrap();
    assert_eq!(
        reopened
            .decide_refinement(id(&proposal), &approval)
            .unwrap(),
        accepted
    );
    assert_eq!(reopened.get_run(id(&run)).unwrap()["taskRevision"], 1);
    assert_eq!(
        reopened.requirement_revisions(task_id).unwrap()[0]["proposalId"],
        proposal["id"]
    );
    assert!(
        notes(&reopened)
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["revision"] == 2)
    );
}
#[test]
fn discarded_reason_and_proposal_decision_commit_together() {
    let (root, store, _, admitted) = setup(false);
    let proposal = store
        .propose_refinement(id(&admitted["attempt"]), "proposal", &input())
        .unwrap();
    let original = notes(&store);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_decision BEFORE UPDATE ON refinement_proposals BEGIN SELECT RAISE(ABORT,'discard interrupted'); END").unwrap();
    let decision = json!({"decision":"discard","reason":"New direction","requestId":uuid::Uuid::new_v4().to_string()});
    assert!(store.decide_refinement(id(&proposal), &decision).is_err());
    assert_eq!(notes(&store), original);
    db.execute_batch("DROP TRIGGER fail_decision").unwrap();
    let discarded = store.decide_refinement(id(&proposal), &decision).unwrap();
    assert_eq!(discarded["status"], "discarded");
    assert_eq!(notes(&store).as_array().unwrap().len(), 3);
}
#[test]
fn failed_replacement_artifact_does_not_supersede_the_prior_record() {
    let (root, store, run, admitted) = setup(true);
    let original = artifact(&run, &admitted);
    store.save_research_artifact(&original).unwrap();
    let replacement = artifact(&run, &admitted);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_artifact BEFORE INSERT ON research_artifacts BEGIN SELECT RAISE(ABORT,'artifact interrupted'); END").unwrap();
    assert!(store.save_research_artifact(&replacement).is_err());
    assert_eq!(
        store.research_artifacts(id(&run)).unwrap(),
        vec![original.clone()]
    );
    db.execute_batch("DROP TRIGGER fail_artifact").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    reopened.save_research_artifact(&replacement).unwrap();
    reopened.save_research_artifact(&replacement).unwrap();
    let records = reopened.research_artifacts(id(&run)).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["supersededBy"], replacement["id"]);
    assert_eq!(records[1], replacement);
}
#[test]
fn interrupted_completion_restores_paused_run_and_pending_steering() {
    let (root, store, run, admitted) = setup(true);
    store
        .save_research_artifact(&artifact(&run, &admitted))
        .unwrap();
    let request = store
        .request_research_completion(id(&run), id(&admitted["attempt"]), "complete")
        .unwrap();
    store.begin_pause(id(&run), None).unwrap();
    store
        .record_ownership(id(&admitted["attempt"]), true, None)
        .unwrap();
    store.finish_pause(id(&run)).unwrap();
    let steering = store
        .steer(
            id(&run),
            &json!({"requestId":"late","body":"Retain direction"}),
        )
        .unwrap();
    let before = store.get_run(id(&run)).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_outcome BEFORE UPDATE ON runs WHEN json_extract(NEW.data,'$.researchOutcome') IS NOT NULL BEGIN SELECT RAISE(ABORT,'outcome interrupted'); END").unwrap();
    let request_id = request["requestId"].as_str().unwrap();
    assert!(store.complete_research(id(&run), request_id, 2).is_err());
    assert_eq!(store.get_run(id(&run)).unwrap(), before);
    assert_eq!(store.steering(id(&run)).unwrap(), vec![steering]);
    db.execute_batch("DROP TRIGGER fail_outcome").unwrap();
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let outcome = reopened.complete_research(id(&run), request_id, 2).unwrap();
    assert_eq!(outcome["status"], "completed");
    assert_eq!(
        reopened.complete_research(id(&run), request_id, 2).unwrap(),
        outcome
    );
    assert_eq!(
        reopened.steering(id(&run)).unwrap()[0]["delivery"],
        "not_delivered"
    );
}
#[test]
fn concurrent_acceptance_of_the_same_base_has_one_winner() {
    let (root, store, run, admitted) = setup(false);
    let proposals = ["first", "second"].map(|key| {
        store
            .propose_refinement(id(&admitted["attempt"]), key, &input())
            .unwrap()
    });
    let path = root.path().join("workflow.sqlite");
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let threads = proposals
        .into_iter()
        .map(|p| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                store.decide_refinement(id(&p), &decision())
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
    assert_eq!(
        reopened
            .requirement_revisions(run["taskId"].as_str().unwrap())
            .unwrap()
            .len(),
        1
    );
    let proposals = reopened.refinement_proposals(None).unwrap();
    assert_eq!(
        proposals
            .iter()
            .filter(|p| p["status"] == "accepted")
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
