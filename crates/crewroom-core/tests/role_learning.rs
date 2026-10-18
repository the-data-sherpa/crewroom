use crewroom_core::{learning::Learning, role_history, roles::Roles, store::Store};
use serde_json::{Value, json};
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn reviewer(roles: &Roles) -> Value {
    roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "reviewer")
        .unwrap()
}
#[test]
fn operator_proposals_keep_evidence_and_approve_once_without_a_fake_run() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let original = reviewer(&roles);
    let request = json!({"roleId":"reviewer","revision":1,"mode":"teach","change":"Check the exact revision. Quote results.","rationale":"Operator observed missing revision IDs.","requestId":uuid::Uuid::new_v4().to_string()});
    let p = Learning::new(&store, &roles)
        .propose_operator(&request)
        .unwrap();
    assert_eq!(p["origin"], "operator");
    assert!(p["runId"].is_null());
    assert_eq!(p["evidence"][0]["text"], request["rationale"]);
    assert!(
        store.snapshot().unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(reviewer(&roles), original);
    assert_eq!(
        Learning::new(&store, &roles)
            .propose_operator(&request)
            .unwrap(),
        p
    );
    let mut changed = request.clone();
    changed["mode"] = json!("ask");
    assert!(
        Learning::new(&store, &roles)
            .propose_operator(&changed)
            .is_err()
    );
    let decision = json!({"decision":"approve","reason":"Reviewed exact replacement","requestId":uuid::Uuid::new_v4().to_string()});
    let approved = Learning::new(&store, &roles)
        .decide(id(&p), &decision)
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    Learning::new(&store, &roles).reconcile().unwrap();
    assert_eq!(
        Learning::new(&store, &roles)
            .decide(id(&p), &decision)
            .unwrap(),
        approved
    );
    let saved = reviewer(&roles);
    assert_eq!(saved["revision"], 2);
    let history = role_history::history(&saved);
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0]["definition"]["instructions"],
        original["instructions"]
    );
    assert_eq!(history[1]["proposalId"], p["id"]);
    assert_eq!(
        role_history::diff(Some(&history[0]["definition"]), &history[1]["definition"])[0]["field"],
        "instructions"
    );
    let mut manual = saved.clone();
    manual["history"] = json!([]);
    manual["instructions"] = json!("Manual follow-up");
    roles.update("reviewer", &manual).unwrap();
    assert_eq!(role_history::history(&reviewer(&roles)).len(), 3);
    assert_eq!(role_history::history(&reviewer(&roles))[1], history[1]);
    let mut stale = request.clone();
    stale["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(
        Learning::new(&store, &roles)
            .propose_operator(&stale)
            .is_err()
    );
}
#[test]
fn old_role_history_starts_at_the_known_revision_and_api_cannot_forge_it() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let mut catalog = roles.read().unwrap();
    let role = catalog.iter_mut().find(|r| r["id"] == "reviewer").unwrap();
    role["revision"] = json!(12);
    std::fs::write(roles.path(), serde_json::to_vec(&catalog).unwrap()).unwrap();
    let mut role = reviewer(&roles);
    role["instructions"] = json!("Current edit");
    role["history"] = json!([{"revision":1,"origin":"invented"}]);
    roles.update("reviewer", &role).unwrap();
    let h = role_history::history(&reviewer(&roles));
    assert_eq!(h.len(), 2);
    assert_eq!(h[0]["revision"], 12);
    assert_eq!(h[0]["origin"], "imported");
    assert!(h[0]["at"].is_null());
    assert_eq!(h[1]["revision"], 13);
}
#[test]
fn drafting_validates_before_recording_and_never_saves_a_role_automatically() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let before = roles.read().unwrap();
    let request = json!({"description":"Review scientific charts","requestId":uuid::Uuid::new_v4().to_string()});
    let run = store.queue_role_draft(&request).unwrap();
    assert_eq!(store.queue_role_draft(&request).unwrap(), run);
    assert_eq!(run["activeLimitMs"], 300000);
    let role = before.iter().find(|r| r["id"] == "coordinator").unwrap();
    let a=store.admit_initial_team(id(&run),&[json!({"name":"Coordinator","coordinator":true,"role":role,"input":"Draft","model":{}})]).unwrap();
    let attempt = id(&a[0]["attempt"]);
    store.claim_attempt(attempt).unwrap();
    store.record_attempt_identity(attempt,&json!({"hostPid":101,"ompPid":102,"sessionId":"fixture","pane":"%1","hostUrl":"http://127.0.0.1:59001","runnerCwd":root.path()})).unwrap();
    store.set_desired_state(id(&run), "running").unwrap();
    store.start_execution(id(&run)).unwrap();
    let bad =
        json!({"brief":"Not JSON","criteria":["Fit"],"decisions":[],"unresolvedQuestions":[]});
    assert!(store.propose_refinement(attempt, "bad", &bad).is_err());
    assert!(store.refinement_proposals(None).unwrap().is_empty());
    assert_eq!(roles.read().unwrap(), before);
    let role = json!({"id":"attempted-id","name":"Chart reviewer","category":"Quality","purpose":"Check scientific plots","instructions":"Check axes and uncertainty.","job":"review","model":"injected/model"});
    let body = json!({"brief":role.to_string(),"criteria":["Fit"],"decisions":[],"unresolvedQuestions":[]});
    let p = store.propose_refinement(attempt, "good", &body).unwrap();
    assert_eq!(store.propose_refinement(attempt, "good", &body).unwrap(), p);
    assert_eq!(p["roleDraft"]["id"], run["roleDraft"]["roleId"]);
    assert_eq!(p["roleDraft"]["model"], "");
    assert_eq!(roles.read().unwrap(), before);
    assert!(
        store
            .decide_refinement(
                id(&p),
                &json!({"decision":"accept","requestId":uuid::Uuid::new_v4().to_string()})
            )
            .is_err()
    );
    let mut edited = p["roleDraft"].clone();
    edited["purpose"] = json!("Operator-refined purpose");
    roles.create_from_draft(&edited, id(&p)).unwrap();
    let saved = roles
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == edited["id"])
        .unwrap();
    assert_eq!(saved["history"][0]["proposalId"], p["id"]);
    assert_eq!(saved["purpose"], edited["purpose"]);
}

#[test]
fn approval_receipt_recovers_from_history_after_a_later_manual_edit() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let p=Learning::new(&store,&roles).propose_operator(&json!({"roleId":"reviewer","revision":1,"mode":"ask","change":"Approved replacement","rationale":"Operator evidence","requestId":uuid::Uuid::new_v4().to_string()})).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_receipt BEFORE UPDATE ON learning_proposals WHEN json_extract(OLD.data,'$.decision.pendingRoleSave')=1 AND json_extract(NEW.data,'$.decision.pendingRoleSave') IS NULL BEGIN SELECT RAISE(ABORT,'lost receipt'); END").unwrap();
    assert!(Learning::new(&store,&roles).decide(id(&p),&json!({"decision":"approve","reason":"Reviewed","requestId":uuid::Uuid::new_v4().to_string()})).is_err());
    let mut role = reviewer(&roles);
    assert_eq!(role["revision"], 2);
    role["instructions"] = json!("Later manual edit");
    roles.update("reviewer", &role).unwrap();
    db.execute_batch("DROP TRIGGER fail_receipt").unwrap();
    Learning::new(&store, &roles).reconcile().unwrap();
    let p = store.get_learning_proposal(id(&p)).unwrap();
    assert_eq!(p["status"], "approved");
    assert_eq!(p["decision"]["appliedVersion"], 2);
    assert_eq!(reviewer(&roles)["revision"], 3);
}
#[test]
fn schema_26_learning_rows_survive_nullable_operator_link_migration() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let task = store
        .create_task(&json!({"title":"Prior evidence","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(id(&task), &json!({"revision":1,"purpose":"explore"}))
        .unwrap();
    let p=store.propose_learning(&json!({"runId":run["id"],"attemptId":"prior","toolCallId":"prior","target":{"kind":"role","roleId":"reviewer"},"baseVersion":1,"change":"Prior proposal","rationale":"Prior evidence","evidence":[{"kind":"steering","id":"prior"}]})).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE learning_old (id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id),attempt_id TEXT NOT NULL,tool_call_id TEXT NOT NULL,data TEXT NOT NULL,UNIQUE(attempt_id,tool_call_id)); INSERT INTO learning_old SELECT * FROM learning_proposals; DROP TABLE learning_proposals; ALTER TABLE learning_old RENAME TO learning_proposals; DROP TABLE comparison_requests; DROP TABLE comparison_events; DROP TABLE comparison_cells; DROP TABLE comparisons; DROP TABLE choreography_revisions; DROP TABLE plan_decisions; DROP TABLE implementation_plans; DROP TABLE intake_requests; DROP TABLE plan_sources; PRAGMA user_version=26;").unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.get_learning_proposal(id(&p)).unwrap(), p);
    let roles = Roles::new(root.path().join("roles.json"));
    Learning::new(&store,&roles).propose_operator(&json!({"roleId":"reviewer","revision":1,"mode":"ask","change":"Operator correction","rationale":"Operator evidence","requestId":uuid::Uuid::new_v4().to_string()})).unwrap();
}
#[test]
fn stale_operator_approval_points_to_agent_roles_not_the_coordinator() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let p = Learning::new(&store, &roles)
        .propose_operator(&json!({"roleId":"reviewer","revision":1,"mode":"ask","change":"Replacement instructions.","rationale":"Operator correction","requestId":uuid::Uuid::new_v4().to_string()}))
        .unwrap();
    let mut role = reviewer(&roles);
    role["instructions"] = json!("Manual edit after the proposal");
    role["revision"] = json!(1);
    roles.update("reviewer", &role).unwrap();
    let error = Learning::new(&store, &roles)
        .decide(
            id(&p),
            &json!({"decision":"approve","reason":"Approve anyway","requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Submit a new proposal from Agent roles."),
        "{error}"
    );
    assert!(!error.contains("coordinator"), "{error}");
}
