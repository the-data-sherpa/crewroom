use crewroom_core::choreography::{PackageState, Schedule};
use serde_json::{Value, json};

fn plan() -> Value {
    json!({"id":"plan","sha256":"hash","content":{"schemaVersion":1,"goal":"Bounded implementation","scope":["Backend behavior"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"exchange-api","name":"Backend","dependsOn":[],"outputs":["Code and docs"],"criteria":[0],"validation":["Edge-case check"]}]}})
}
fn schedule() -> Value {
    json!({"uiRequired":false,"diagnosisReason":null,"assignments":[
        {"name":"Writer","roleId":"implementer","responsibility":"writer","stage":0,"packages":["exchange-api"],"writes":true,"rationale":"Implement behavior","input":"Write and discuss edge cases"},
        {"name":"Navigator","roleId":"navigator","responsibility":"navigator","stage":0,"packages":["exchange-api"],"writes":false,"rationale":"Challenge behavior","input":"Inspect and challenge edge cases"},
        {"name":"Docs","roleId":"tech-writer","responsibility":"documentation","stage":1,"packages":["exchange-api"],"writes":true,"rationale":"Document behavior","input":"Update usage before freeze"},
        {"name":"Review","roleId":"reviewer","responsibility":"reviewer","stage":2,"packages":["exchange-api"],"writes":false,"rationale":"Independent review","input":"Review final candidate"}
    ]})
}
#[test]
fn lifecycle_labels_cannot_skip_execution_or_implicitly_admit_feedback() {
    use PackageState::*;
    for from in [Proposed, Active, Blocked, Complete] {
        for to in [Proposed, Active, Blocked, Complete] {
            let allowed = matches!(
                (from, to),
                (Proposed, Active) | (Active, Blocked) | (Active, Complete) | (Blocked, Active)
            );
            assert_eq!(from.permits(to, false), allowed, "{from:?} -> {to:?}");
        }
    }
    assert!(Complete.permits(Active, true));
    assert!(!Proposed.permits(Complete, true));
}
#[test]
fn backend_and_ui_select_different_staged_teams() {
    let backend = Schedule::parse(&plan(), &schedule()).unwrap();
    assert_eq!(backend.assignments.len(), 4);
    let mut ui = schedule();
    ui["uiRequired"] = json!(true);
    assert!(Schedule::parse(&plan(), &ui).is_err());
    ui["assignments"].as_array_mut().unwrap().push(json!({"name":"Interface","roleId":"ui-designer","responsibility":"ui","stage":0,"packages":["exchange-api"],"writes":false,"rationale":"Inspect rendered behavior","input":"Supply a rendered assessment"}));
    assert_eq!(Schedule::parse(&plan(), &ui).unwrap().assignments.len(), 5);
}
#[test]
fn schedule_rejects_skipped_obligations_unknown_packages_and_unsafe_authority() {
    let invalid = |mutate: fn(&mut Value)| {
        let mut value = schedule();
        mutate(&mut value);
        assert!(
            Schedule::parse(&plan(), &value).is_err(),
            "accepted {value}"
        );
    };
    invalid(|v| {
        v["assignments"][1]["writes"] = json!(true);
    });
    invalid(|v| {
        v["assignments"][3]["stage"] = json!(0);
    });
    invalid(|v| {
        v["assignments"][3]["name"] = json!("Navigator");
    });
    invalid(|v| {
        v["assignments"][2]["responsibility"] = json!("diagnosis");
    });
    invalid(|v| {
        v["assignments"][0]["packages"] = json!(["unknown"]);
    });
    invalid(|v| {
        v["assignments"][1]["stage"] = json!(1);
    });
    invalid(|v| {
        v["assignments"][2]["stage"] = json!(0);
    });
    invalid(|v| {
        v["diagnosisReason"] = json!("A failure needs diagnosis");
    });
    invalid(|v| {
        v["assignments"][1]["rationale"] = json!(" ");
    });
}
#[test]
fn package_dependencies_are_not_assignment_or_candidate_completion() {
    let mut p = plan();
    let mut dependent = p["content"]["workPackages"][0].clone();
    dependent["id"] = json!("dependent");
    dependent["dependsOn"] = json!(["exchange-api"]);
    p["content"]["workPackages"]
        .as_array_mut()
        .unwrap()
        .push(dependent);
    let mut s = schedule();
    for member in s["assignments"].as_array_mut().unwrap() {
        member["packages"] = json!(["exchange-api", "dependent"]);
    }
    assert!(Schedule::parse(&p, &s).is_err());
    let mut s = schedule();
    let mut later = s["assignments"].as_array().unwrap().clone();
    for member in &mut later {
        member["name"] = json!(format!("Later{}", member["name"].as_str().unwrap()));
        member["packages"] = json!(["dependent"]);
        member["stage"] = json!(member["stage"].as_u64().unwrap() + 2);
    }
    s["assignments"][3]["stage"] = json!(4);
    s["assignments"].as_array_mut().unwrap().extend(later);
    assert!(Schedule::parse(&p, &s).is_ok());
    p["content"]["workPackages"][0]["dependsOn"] = json!(["dependent"]);
    assert!(Schedule::parse(&p, &s).is_err());
}

use crewroom_core::store::Store;
use rusqlite::{Connection, params};
fn setup() -> (tempfile::TempDir, Store, String, String) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let task = store
        .create_task(&json!({"title":"Choreography","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let admitted=store.admit_assignment(run["id"].as_str().unwrap(),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{},"input":"Coordinate packages"})).unwrap();
    let attempt = admitted["attempt"]["id"].as_str().unwrap().to_owned();
    // Seed the captured MH5 inputs; these core tests do not claim Git admission.
    let mut captured = plan();
    captured["decision"] =
        json!({"decision":"accept","basis":{"taskRevision":run["taskRevision"]}});
    let db = Connection::open(path).unwrap();
    db.execute("UPDATE runs SET data=json_set(data,'$.purpose','coding','$.status','running','$.desiredState','running','$.executionPolicy',1,'$.implementationPlan',json(?)) WHERE id=?",params![captured.to_string(),run["id"].as_str().unwrap()]).unwrap();
    db.execute("UPDATE attempts SET data=json_set(data,'$.ownership','held','$.state','idle','$.sessionId','coordinator-session') WHERE id=?",[&attempt]).unwrap();
    (root, store, run["id"].as_str().unwrap().into(), attempt)
}
#[test]
fn proposal_replay_cas_reopen_and_fault_rollback_start_no_processes() {
    let (root, store, run, coordinator) = setup();
    let input = json!({"revision":0,"schedule":schedule()});
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON queue_requests BEGIN SELECT RAISE(ABORT,'lost transaction'); END;").unwrap();
    assert!(
        store
            .propose_choreography(&coordinator, "proposal", &input)
            .is_err()
    );
    assert!(store.choreography(&run).unwrap().is_none());
    db.execute_batch("DROP TRIGGER fail_receipt").unwrap();
    let saved = store
        .propose_choreography(&coordinator, "proposal", &input)
        .unwrap();
    assert_eq!(saved["revision"], 1);
    assert_eq!(
        store.runtime(&run).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store.runtime(&run).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .propose_choreography(&coordinator, "stale", &input)
            .is_err()
    );
    let mut changed = input.clone();
    changed["schedule"]["assignments"][0]["rationale"] = json!("Changed decision");
    assert!(
        store
            .propose_choreography(&coordinator, "proposal", &changed)
            .is_err()
    );
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        reopened
            .propose_choreography(&coordinator, "proposal", &input)
            .unwrap(),
        saved
    );
    assert_eq!(reopened.choreography_history(&run).unwrap(), vec![saved]);
    assert!(!reopened.choreography_blockers(&run).unwrap().is_empty());
    let action = json!({"revision":1,"packageId":"exchange-api","kind":"start","summary":"Start selected work","artifacts":[]});
    assert!(
        reopened
            .record_package(&coordinator, "premature", &action, &json!({}))
            .is_err()
    );
    let mut skip = action.clone();
    skip["kind"] = json!("complete");
    assert!(
        reopened
            .record_package(&coordinator, "skip", &skip, &json!({}))
            .is_err()
    );
    assert_eq!(reopened.choreography_history(&run).unwrap().len(), 1);
}
#[test]
fn schema28_upgrade_preserves_admitted_legacy_without_pair_history() {
    let (root, store, run, _) = setup();
    drop(store);
    let path = root.path().join("workflow.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE comparison_requests; DROP TABLE comparison_events; DROP TABLE comparison_cells; DROP TABLE comparisons; DROP TABLE choreography_revisions; PRAGMA user_version=28;")
        .unwrap();
    db.execute(
        "UPDATE runs SET data=json_remove(data,'$.executionPolicy') WHERE id=?",
        [&run],
    )
    .unwrap();
    let before: String = db
        .query_row("SELECT data FROM runs WHERE id=?", [&run], |r| r.get(0))
        .unwrap();
    drop(db);
    let restored = Store::open(&path).unwrap();
    assert!(restored.choreography(&run).unwrap().is_none());
    assert!(restored.choreography_blockers(&run).unwrap().is_empty());
    drop(restored);
    let db = Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("SELECT data FROM runs WHERE id=?", [&run], |r| r
            .get::<_, String>(0))
            .unwrap(),
        before
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        crewroom_core::store::SCHEMA_VERSION
    );
}
fn seed_member(
    root: &tempfile::TempDir,
    run: &str,
    name: &str,
    role: &str,
    retired: bool,
) -> (String, String) {
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    let aid = uuid::Uuid::new_v4().to_string();
    let pid = uuid::Uuid::new_v4().to_string();
    let mut assignment = json!({"id":aid,"runId":run,"name":name,"role":{"id":role},"coordinator":false,"currentAttemptId":pid});
    if retired {
        assignment["retiredAt"] = json!("fixture stopped");
    }
    db.execute(
        "INSERT INTO assignments VALUES(?,?,?,?)",
        params![aid, run, name, assignment.to_string()],
    )
    .unwrap();
    db.execute("INSERT INTO attempts VALUES(?,?,?)",params![pid,aid,json!({"id":pid,"assignmentId":aid,"runId":run,"state":"idle","ownership":"held","sessionId":format!("session-{name}")}).to_string()]).unwrap();
    (aid, pid)
}
#[test]
fn first_schedule_after_retiring_a_supplemental_consultant_has_no_predecessor_to_reopen() {
    let (root, store, run, coordinator) = setup();
    seed_member(&root, &run, "EarlyConsultant", "tech-writer", true);
    let state = store
        .propose_choreography(
            &coordinator,
            "initial",
            &json!({"revision":0,"schedule":schedule()}),
        )
        .unwrap();
    assert_eq!(state["revision"], 1);
    assert!(state["predecessorRevision"].is_null());
    assert_eq!(
        store.runtime(&run).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
fn package_action(store: &Store, run: &str, attempt: &str, kind: &str, doc: bool) -> Value {
    let mut action = json!({"revision":store.choreography(run).unwrap().unwrap()["revision"],"packageId":"exchange-api","kind":kind,"summary":"Concrete source and test disposition","artifacts":[]});
    if doc {
        action["documentation"] = json!({"disposition":"no_change","reason":"Internal-only change; no documented interface changes."});
    }
    store
        .record_package(
            attempt,
            &uuid::Uuid::new_v4().to_string(),
            &action,
            &json!({"head":"source","worktreeHash":"files"}),
        )
        .unwrap()
}
fn receive(store: &Store, exchange: &Value, transport: u64) {
    let id = exchange["message"]["id"].as_str().unwrap();
    store.record_message_stored(id, transport).unwrap();
    store.accept_message(id).unwrap();
    store.record_dispatch(id, true).unwrap();
}
#[test]
fn addressed_exchange_requires_receipts_response_and_resolution_before_closure() {
    let (root, store, run, coordinator) = setup();
    store
        .propose_choreography(
            &coordinator,
            "plan",
            &json!({"revision":0,"schedule":schedule()}),
        )
        .unwrap();
    let (writer_id, writer) = seed_member(&root, &run, "Writer", "implementer", false);
    let (navigator_id, navigator) = seed_member(&root, &run, "Navigator", "navigator", false);
    package_action(&store, &run, &coordinator, "start", false);
    assert!(
        store
            .assert_choreography_writer(&navigator, Some(&json!({})))
            .is_err()
    );
    let exchange = |who: &str,
                    kind: &str,
                    to: &str,
                    challenge: Option<&str>,
                    response: Option<&str>| {
        let mut input = json!({"revision":store.choreography(&run).unwrap().unwrap()["revision"],"packageId":"exchange-api","kind":kind,"to":to,"body":"Zero is a valid boundary: rejecting zero drops the first entry.","artifact":"source.rs:42","proposedChange":"Add the zero-input regression and allow zero."});
        if let Some(v) = challenge {
            input["challengeId"] = json!(v);
        }
        if let Some(v) = response {
            input["responseId"] = json!(v);
        }
        store.prepare_pair_exchange(
            who,
            &uuid::Uuid::new_v4().to_string(),
            &input,
            &json!({"head":"source"}),
        )
    };
    let approach = exchange(&writer, "approach", "Navigator", None, None).unwrap();
    assert!(exchange(&navigator, "challenge", "Writer", None, None).is_err());
    receive(&store, &approach, 1);
    let challenge = exchange(&navigator, "challenge", "Writer", None, None).unwrap();
    receive(&store, &challenge, 2);
    assert_eq!(challenge["message"]["from"], navigator_id);
    assert_eq!(challenge["message"]["to"], writer_id);
    let response = exchange(
        &writer,
        "response",
        "Navigator",
        challenge["exchange"]["id"].as_str(),
        None,
    )
    .unwrap();
    receive(&store, &response, 3);
    let resolution = exchange(
        &coordinator,
        "resolution",
        "Navigator",
        challenge["exchange"]["id"].as_str(),
        response["exchange"]["id"].as_str(),
    )
    .unwrap();
    package_action(&store, &run, &writer, "report", false);
    package_action(&store, &run, &navigator, "report", false);
    assert!(
        store
            .validate_choreography_team(&run, &[], &[json!({"assignmentId":writer_id})])
            .is_err()
    );
    receive(&store, &resolution, 4);
    assert!(
        store
            .validate_choreography_team(&run, &[], &[json!({"assignmentId":writer_id})])
            .is_ok()
    );
    let (_, docs) = seed_member(&root, &run, "Docs", "tech-writer", false);
    package_action(&store, &run, &docs, "report", true);
    package_action(&store, &run, &coordinator, "complete", false);
    assert!(store.choreography_blockers(&run).unwrap().is_empty());
    assert!(
        store
            .assert_choreography_reviewer(&navigator_id, Some(&navigator))
            .is_err()
    );
    let (review_id, review) = seed_member(&root, &run, "Review", "reviewer", false);
    assert!(
        store
            .assert_choreography_reviewer(&review_id, Some(&review))
            .is_ok()
    );
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute(
        "UPDATE attempts SET data=json_set(data,'$.sessionId','session-Navigator') WHERE id=?",
        [&review],
    )
    .unwrap();
    assert!(
        store
            .assert_choreography_reviewer(&review_id, Some(&review))
            .is_err()
    );
    assert!(store.assert_package_edit(&writer).is_err());
    let reopen = json!({"revision":store.choreography(&run).unwrap().unwrap()["revision"],"packageId":"exchange-api","kind":"reopen","summary":"A PR comment alone cannot authorize work","artifacts":[]});
    assert!(
        store
            .record_package(&coordinator, "no-auto-feedback", &reopen, &json!({}))
            .is_err()
    );
    let prior = store.choreography(&run).unwrap().unwrap();
    let mut successor = schedule();
    for member in successor["assignments"].as_array_mut().unwrap() {
        member["name"] = json!(format!("Next{}", member["name"].as_str().unwrap()));
    }
    let proposal = json!({"revision":prior["revision"],"schedule":successor,"reopenPackages":["exchange-api"]});
    assert!(
        store
            .propose_choreography(&coordinator, "early-successor", &proposal)
            .is_err()
    );
    db.execute("UPDATE assignments SET data=json_set(data,'$.retiredAt','stopped') WHERE json_extract(data,'$.coordinator')=0",[]).unwrap();
    let next = store
        .propose_choreography(&coordinator, "successor", &proposal)
        .unwrap();
    assert_eq!(next["reports"], prior["reports"]);
    assert_eq!(next["packages"][0]["state"], "complete");
    assert_eq!(next["predecessorRevision"], prior["revision"]);
    assert_eq!(
        store
            .propose_choreography(&coordinator, "successor", &proposal)
            .unwrap(),
        next
    );
    assert!(
        store
            .assert_choreography_writer(&writer, Some(&json!({})))
            .is_err()
    );
    // A new intent has no execution authority and needs fresh pair reports.
    assert!(!store.choreography_blockers(&run).unwrap().is_empty());
}
#[test]
fn exchange_receipt_failure_rolls_back_mail_intent_and_stale_senders_are_denied() {
    let (root, store, run, coordinator) = setup();
    store
        .propose_choreography(
            &coordinator,
            "plan",
            &json!({"revision":0,"schedule":schedule()}),
        )
        .unwrap();
    let (_, writer) = seed_member(&root, &run, "Writer", "implementer", false);
    seed_member(&root, &run, "Navigator", "navigator", false);
    package_action(&store, &run, &coordinator, "start", false);
    let input = json!({"revision":2,"packageId":"exchange-api","kind":"approach","to":"Navigator","body":"Inspect the boundary handling","artifact":"source.rs","proposedChange":"Handle the zero input"});
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_exchange BEFORE INSERT ON choreography_revisions BEGIN SELECT RAISE(ABORT,'fixture crash'); END;").unwrap();
    assert!(
        store
            .prepare_pair_exchange(&writer, "exchange", &input, &json!({}))
            .is_err()
    );
    assert!(
        store.runtime(&run).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    db.execute_batch("DROP TRIGGER fail_exchange").unwrap();
    let saved = store
        .prepare_pair_exchange(&writer, "exchange", &input, &json!({"head":"observed"}))
        .unwrap();
    drop(store);
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        store
            .prepare_pair_exchange(&writer, "exchange", &input, &json!({"head":"later"}))
            .unwrap(),
        saved
    );
    db.execute("UPDATE assignments SET data=json_set(data,'$.retiredAt','stopped') WHERE json_extract(data,'$.name')='Writer'",[]).unwrap();
    assert!(
        store
            .prepare_pair_exchange(&writer, "late", &input, &json!({}))
            .is_err()
    );
    assert_eq!(
        store.runtime(&run).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
