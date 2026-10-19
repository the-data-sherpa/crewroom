use crewroom_core::{
    principles::{Principles, snapshot},
    roles::Roles,
    store::Store,
};
use serde_json::{Value, json};
#[test]
fn revisions_references_and_assignment_copies_survive_edit_delete_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let principles = Principles::beside_roles(roles.path());
    let first = principles
        .save(
            &json!({"id":"evidence","name":"Evidence","text":"Cite exact evidence.","revision":0}),
        )
        .unwrap();
    let mut role = roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap();
    role["revision"] = json!(1);
    role["principles"] = json!(["evidence"]);
    roles.update("coordinator", &role).unwrap();
    let brief = snapshot(
        &json!({"criteria":[],"dependsOn":[]}),
        &role,
        &principles.read().unwrap(),
    )
    .unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Frozen principles","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let assignment=store.admit_assignment(run["id"].as_str().unwrap(),&json!({"name":"Coordinator","coordinator":true,"role":role,"brief":brief,"model":{},"input":"Inspect"})).unwrap();
    let mut updated = first.clone();
    updated["text"] = json!("Name the tested revision too.");
    let second = principles.save(&updated).unwrap();
    assert_eq!(second["revision"], 2);
    assert!(principles.save(&updated).is_err());
    let next = snapshot(&brief, &role, &principles.read().unwrap()).unwrap();
    assert_eq!(next["principles"][0]["text"], second["text"]);
    assert!(
        principles
            .delete("evidence", 2, &roles, &json!({}))
            .unwrap_err()
            .to_string()
            .contains("Detach")
    );
    let mut detached = roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap();
    detached["principles"] = json!([]);
    roles.update("coordinator", &detached).unwrap();
    principles
        .delete("evidence", 2, &roles, &json!({}))
        .unwrap();
    assert!(principles.read().unwrap().is_empty());
    assert!(snapshot(&brief, &role, &principles.read().unwrap()).is_err());
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let retained = &reopened.runtime(run["id"].as_str().unwrap()).unwrap()["assignments"][0];
    assert_eq!(*retained, assignment["assignment"]);
    assert_eq!(retained["brief"]["principles"][0], first);
    let mut invalid = detached;
    invalid["revision"] = json!(3);
    invalid["principles"] = json!(["missing"]);
    assert!(roles.update("coordinator", &invalid).is_err());
}
#[test]
fn inserted_specialists_capture_current_principles_once_when_the_proposal_is_applied() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let mut catalog = roles.read().unwrap();
    let writer = catalog
        .iter_mut()
        .find(|r| r["id"] == "tech-writer")
        .unwrap();
    writer["principles"] = json!(["evidence"]);
    let writer = writer.clone();
    let task = store
        .create_task(&json!({"title":"Principle insertion","description":""}))
        .unwrap();
    store.refine(task["id"].as_str().unwrap(), &json!({"revision":1,"sourceRevision":1,"brief":"Inspect evidence","criteria":["Explain findings"]})).unwrap();
    let run = store
        .queue_research(
            task["id"].as_str().unwrap(),
            &json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap();
    let run_id = run["id"].as_str().unwrap();
    let coordinator = catalog.iter().find(|r| r["id"] == "coordinator").unwrap();
    let admitted=store.admit_initial_team(run_id,&[json!({"name":"Coordinator","coordinator":true,"role":coordinator,"model":{},"input":"Inspect"})]).unwrap();
    let attempt = admitted[0]["attempt"]["id"].as_str().unwrap();
    store.claim_attempt(attempt).unwrap();
    store.record_attempt_identity(attempt,&json!({"hostPid":101,"ompPid":102,"sessionId":"fixture","hostUrl":"http://127.0.0.1:59999","pane":"%1"})).unwrap();
    store.set_desired_state(run_id, "running").unwrap();
    store.start_execution(run_id).unwrap();
    let proposal = store
        .propose_team(
            attempt,
            "principle-insertion",
            &[json!({"name":"Writer","role":writer,"model":{},"input":"Inspect"})],
            &[],
        )
        .unwrap();
    store.begin_pause(run_id, None).unwrap();
    store.record_ownership(attempt, true, None).unwrap();
    store.finish_pause(run_id).unwrap();
    catalog
        .iter_mut()
        .find(|r| r["id"] == "tech-writer")
        .unwrap()["job"] = json!("code");
    let current =
        json!({"id":"evidence","name":"Evidence","text":"Current text at admission","revision":2});
    let applied = store
        .apply_team_proposal_with_principles(
            proposal["id"].as_str().unwrap(),
            &catalog,
            std::slice::from_ref(&current),
        )
        .unwrap();
    assert_eq!(applied["status"], "applied");
    let runtime = store.runtime(run_id).unwrap();
    let inserted = runtime["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Writer")
        .unwrap();
    assert_eq!(inserted["brief"]["principles"], json!([current]));
    assert_eq!(
        inserted["role"]["job"], "judgment",
        "A pending role job edit cannot relabel an already selected proposal model"
    );
    let before: Value = runtime.clone();
    store
        .apply_team_proposal_with_principles(proposal["id"].as_str().unwrap(), &catalog, &[])
        .unwrap();
    assert_eq!(store.runtime(run_id).unwrap(), before);
}
