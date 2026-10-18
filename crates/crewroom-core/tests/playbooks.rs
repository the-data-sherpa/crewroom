mod support;
use crewroom_core::{playbooks::Playbooks, roles::Roles, store::Store};
use serde_json::{Value, json};
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn catalog(root: &std::path::Path) -> (Roles, Playbooks, Value) {
    let roles = Roles::new(root.join("roles.json"));
    let p = Playbooks::beside_roles(&roles);
    let v=p.save(&json!({"id":"delivery","revision":0,"name":"Delivery","description":"For changes","steps":[{"id":"inspect","title":"Inspect"},{"id":"verify","title":"Verify"}]})).unwrap();
    (roles, p, v)
}
fn task(s: &Store) -> Value {
    let repo_root = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo_root.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let repository = s
        .create_repository(&json!({"name":"Fixture","path":repo_root.path(),"targetBranch":"main"}))
        .unwrap();

    let t = s
        .create_task(
            &json!({"title":"Playbook proof","description":"","repositoryId":repository["id"]}),
        )
        .unwrap();
    let task=s.refine(id(&t),&json!({"revision":1,"sourceRevision":1,"brief":"A bounded change","criteria":["Exact evidence"]})).unwrap();
    support::accept_plan(s, id(&task));
    task
}
fn active(s: &Store, run: &Value, roles: &Roles, coordinator: bool) -> String {
    let role = roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| {
            r["id"]
                == if coordinator {
                    "coordinator"
                } else {
                    "reviewer"
                }
        })
        .unwrap();
    let a=s.admit_assignment(id(run),&json!({"name":if coordinator{"Coordinator"}else{"Reviewer"},"coordinator":coordinator,"role":role,"input":"Work","model":{}})).unwrap();
    let attempt = id(&a["attempt"]);
    s.claim_attempt(attempt).unwrap();
    s.record_attempt_identity(attempt,&json!({"hostPid":101,"ompPid":102,"sessionId":"fixture","pane":"%1","hostUrl":"http://127.0.0.1:59001"})).unwrap();
    s.set_desired_state(id(run), "running").unwrap();
    s.start_execution(id(run)).unwrap();
    attempt.to_owned()
}
#[test]
fn queue_copies_catalog_and_replay_keeps_old_steps_after_edit_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let (roles, p, first) = catalog(root.path());
    let path = root.path().join("workflow.sqlite");
    let s = Store::open(&path).unwrap();
    let t = task(&s);
    let body =
        json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string(),"playbookId":"delivery"});
    let r = s
        .queue_with_playbooks(id(&t), &body, &roles.read().unwrap(), &p.read().unwrap())
        .unwrap();
    assert_eq!(r["playbook"]["steps"][0]["state"], "pending");
    let mut changed = first.clone();
    changed["steps"][0]["title"] = json!("New instruction");
    p.save(&changed).unwrap();
    assert!(p.save(&changed).is_err());
    assert_eq!(
        s.queue_with_playbooks(id(&t), &body, &roles.read().unwrap(), &p.read().unwrap())
            .unwrap(),
        r
    );
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.get_run(id(&r)).unwrap()["playbook"], r["playbook"]);
    assert!(s.steering(id(&r)).unwrap().is_empty());
    // Queue identity remains the original selection after a run-level change.
    s.select_playbook(
        id(&r),
        &json!({"playbookId":"", "revision":1,
        "reason":"Use an ad hoc plan", "requestId":uuid::Uuid::new_v4().to_string()}),
        &[],
    )
    .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DELETE FROM queue_requests", []).unwrap();
    assert_eq!(
        s.queue_with_playbooks(id(&t), &body, &[], &[]).unwrap()["id"],
        r["id"]
    );
    let mut mismatched = body.clone();
    mismatched["playbookId"] = json!("");
    assert!(
        s.queue_with_playbooks(id(&t), &mismatched, &[], &[])
            .is_err()
    );
    mismatched.as_object_mut().unwrap().remove("requestId");
    assert!(
        s.queue_with_playbooks(id(&t), &mismatched, &[], &[])
            .is_err()
    );
    mismatched["playbookId"] = json!("delivery");
    assert_eq!(
        s.queue_with_playbooks(id(&t), &mismatched, &[], &[])
            .unwrap()["id"],
        r["id"]
    );
}
#[test]
fn coordinator_proposal_progress_and_operator_steering_are_advisory_and_replayable() {
    let root = tempfile::tempdir().unwrap();
    let (roles, p, _) = catalog(root.path());
    let path = root.path().join("workflow.sqlite");
    let s = Store::open(&path).unwrap();
    let t = task(&s);
    let run = s
        .queue_with_roles(id(&t), &json!({"revision":2}), &roles.read().unwrap())
        .unwrap();
    let a = active(&s, &run, &roles, true);
    let before = s.get_run(id(&run)).unwrap();
    let proposal = s
        .propose_playbook(
            &a,
            "proposal",
            &json!({"playbookId":"delivery","rationale":"Matches the accepted change"}),
            &p.read().unwrap(),
        )
        .unwrap();
    assert!(s.get_run(id(&run)).unwrap()["playbook"].is_null());
    let selection = json!({"proposalId":proposal["id"],"revision":0,"reason":"Operator selected it","requestId":uuid::Uuid::new_v4().to_string()});
    let saved = s
        .select_playbook(id(&run), &selection, &p.read().unwrap())
        .unwrap();
    assert_eq!(s.select_playbook(id(&run), &selection, &[]).unwrap(), saved);
    assert_eq!(s.steering(id(&run)).unwrap().len(), 1);
    assert_eq!(s.steering(id(&run)).unwrap()[0]["delivery"], "pending");
    assert!(
        s.record_playbook_step(
            &a,
            "invalid",
            &json!({"version":1,"stepId":"verify","state":"skipped"})
        )
        .is_err()
    );
    let step = json!({"version":1,"stepId":"verify","state":"skipped","reason":"No relevant external environment exists"});
    let result = s.record_playbook_step(&a, "skip", &step).unwrap();
    assert_eq!(s.record_playbook_step(&a, "skip", &step).unwrap(), result);
    s.record_playbook_step(
        &a,
        "done",
        &json!({"version":1,"stepId":"inspect","state":"done"}),
    )
    .unwrap();
    let after = s.get_run(id(&run)).unwrap();
    for field in ["status", "desiredState", "taskRevision", "stages"] {
        assert_eq!(before[field], after[field]);
    }
    assert!(s.coding_record(id(&run)).unwrap().is_none());
    assert!(s.publication(id(&run)).unwrap().is_none());
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.get_run(id(&run)).unwrap()["playbook"], after["playbook"]);
    assert_eq!(s.record_playbook_step(&a, "skip", &step).unwrap(), result);
    s.select_playbook(id(&run),&json!({"playbookId":"delivery","revision":1,"reason":"Restart checklist","requestId":uuid::Uuid::new_v4().to_string()}),&p.read().unwrap()).unwrap();
    assert!(s.record_playbook_step(&a, "stale", &step).is_err());
    assert_eq!(
        s.get_run(id(&run)).unwrap()["playbook"]["steps"][0]["state"],
        "pending"
    );
    assert!(s.select_playbook(id(&run),&json!({"playbookId":"delivery","revision":1,"reason":"Stale selection","requestId":uuid::Uuid::new_v4().to_string()}),&p.read().unwrap()).is_err());
}
#[test]
fn specialists_and_stopped_coordinators_cannot_report_progress() {
    let root = tempfile::tempdir().unwrap();
    let (roles, p, _) = catalog(root.path());
    let s = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let t = task(&s);
    let run = s
        .queue_with_roles(id(&t), &json!({"revision":2}), &roles.read().unwrap())
        .unwrap();
    let a = active(&s, &run, &roles, false);
    let input = json!({"playbookId":"delivery","rationale":"Test"});
    assert!(
        s.propose_playbook(&a, "specialist", &input, &p.read().unwrap())
            .is_err()
    );
    s.record_ownership(&a, true, None).unwrap();
    assert!(
        s.propose_playbook(&a, "stopped", &input, &p.read().unwrap())
            .is_err()
    );
}
#[test]
fn failed_steering_rolls_back_the_checklist_change() {
    let root = tempfile::tempdir().unwrap();
    let (roles, p, _) = catalog(root.path());
    let path = root.path().join("workflow.sqlite");
    let s = Store::open(&path).unwrap();
    let t = task(&s);
    let run = s
        .queue_with_roles(id(&t), &json!({"revision":2}), &roles.read().unwrap())
        .unwrap();
    active(&s, &run, &roles, true);
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_message BEFORE INSERT ON steering BEGIN SELECT RAISE(ABORT,'interrupted steering'); END").unwrap();
    let request = json!({"playbookId":"delivery","revision":0,"reason":"Switch","requestId":uuid::Uuid::new_v4().to_string()});
    assert!(
        s.select_playbook(id(&run), &request, &p.read().unwrap())
            .is_err()
    );
    assert!(s.get_run(id(&run)).unwrap()["playbook"].is_null());
    db.execute_batch("DROP TRIGGER fail_message").unwrap();
    s.select_playbook(id(&run), &request, &p.read().unwrap())
        .unwrap();
    assert_eq!(s.steering(id(&run)).unwrap().len(), 1);
}
