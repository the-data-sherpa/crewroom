//! Domain receipts use synthetic runner identities; these are not live evaluation evidence.
use crewroom_core::{
    comparisons::{hash, validate_manifest},
    store::Store,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn manifest(kind: &str) -> Value {
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    let worker = roles
        .iter()
        .find(|r| r["id"] == "implementer")
        .unwrap()
        .clone();
    let mut worker = crewroom_core::roles::validate_role(&worker).unwrap();
    worker["revision"] = json!(1);
    let coordinator = roles
        .iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap()
        .clone();
    let coordinator = crewroom_core::roles::validate_role(&coordinator).unwrap();
    let model = json!({"resolved":{"selector":"fixture/model"}});
    let current = match kind {
        "role" => worker.clone(),
        "principle" => json!({"id":"care","name":"Care","revision":1,"text":"Check work."}),
        "playbook" => {
            json!({"id":"delivery","name":"Delivery","revision":1,"description":"Inspect then deliver.","steps":[{"id":"check","title":"Check work."}]})
        }
        _ => unreachable!(),
    };
    let mut proposed = current.clone();
    match kind {
        "role" => {
            proposed["instructions"] =
                json!("Run applicable checks and preserve incomplete results.")
        }
        "principle" => proposed["text"] = json!("Run current checks."),
        "playbook" => proposed["steps"][0]["title"] = json!("Check the final source revision."),
        _ => unreachable!(),
    }
    let mut inputs = json!([
        {"name":"Coordinator","coordinator":true,"input":"Convey the ordinary task and genuine questions.","role":coordinator,"model":model,"brief":{"criteria":[],"dependsOn":[]}},
        {"name":"Builder","coordinator":false,"input":"Complete the requested local behavior.","role":worker,"model":model,"brief":{"criteria":[],"dependsOn":[]}}
    ]);
    match kind {
        "principle" => inputs[1]["brief"]["principles"] = json!([current]),
        "playbook" => inputs[0]["brief"]["playbook"] = current.clone(),
        _ => (),
    }
    let mut changed = inputs.clone();
    match kind {
        "role" => changed[1]["role"] = proposed.clone(),
        "principle" => changed[1]["brief"]["principles"][0] = proposed.clone(),
        "playbook" => changed[0]["brief"]["playbook"] = proposed.clone(),
        _ => (),
    }
    let cases: Vec<_>=(1..=4).map(|n|json!({"id":format!("fixture-{n}"),"version":1,"prompt":"Complete the requested local behavior.","sourceHash":"a".repeat(64),"repository":{"tree":"b".repeat(40)},"requirements":{"id":format!("requirements-{n}"),"criteria":["Requested behavior works"]},"inputs":{"current":inputs,"proposed":changed}})).collect();
    json!({"version":1,"mode":"scripted","target":{"kind":kind,"id":current["id"],"baseRevision":current["revision"],"baseHash":hash(&current),"current":current,"proposed":proposed,"proposedHash":hash(&proposed),"draftId":"draft-one","draftVersion":1,"consumer":if kind=="playbook" {"coordinator"} else {"specialist"}},"cases":cases,"execution":{"model":model,"coordinatorModel":model,"runner":{"version":"fixture"},"tools":{"hash":"c".repeat(64)},"capabilities":["case-read"],"application":{"hash":"d".repeat(64)},"platform":{"os":"fixture"},"credentialReference":"fixture-no-credentials"},"design":{"repetitions":2,"variants":["current","proposed"],"scorer":{"version":1},"coordinator":{"policy":"ordinary-task-only"},"variantMapping":{"A":"current","B":"proposed"},"perCellActiveMs":180000,"totalActiveMs":2880000}})
}
fn setup() -> (tempfile::TempDir, Store, Value) {
    setup_for_kind("role")
}
fn setup_for_kind(kind: &str) -> (tempfile::TempDir, Store, Value) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), || 1000).unwrap();
    let value = store
        .create_comparison(&json!({"requestId":key(),"manifest":manifest(kind)}))
        .unwrap();
    (root, store, value)
}
fn start(store: &Store, value: &Value) -> Value {
    store
        .control_comparison(id(value), &json!({"requestId":key(),"action":"start"}))
        .unwrap();
    store
        .create_comparison_cell_run(id(value), id(&value["cells"][0]))
        .unwrap()
}
fn unavailable() -> Value {
    json!({"requestId":key(),"result":{"state":"unavailable","observed":false,"reason":"Synthetic provider unavailable","cleanup":{"state":"stopped","receipt":"fixture"},"usage":{"available":false,"reason":"No provider usage received"}}})
}

#[test]
fn durable_finalization_fences_all_further_attempt_claims_after_reopen() {
    let (root, store, value) = setup();
    let run = start(&store, &value);
    let admitted = store
        .admit_initial_team(id(&run), run["evaluationInputs"].as_array().unwrap())
        .unwrap();
    let input =
        json!({"requestId":key(), "type":"finalization", "payload":{"source":{"sha256":"frozen"}}});
    let cell_id = id(&value["cells"][0]);
    let receipt = store.append_comparison_event(cell_id, &input).unwrap();
    drop(store);
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), || 1000).unwrap();
    assert_eq!(
        store.append_comparison_event(cell_id, &input).unwrap(),
        receipt
    );
    for member in admitted.as_array().unwrap() {
        assert!(store.claim_attempt(id(&member["attempt"])).is_err());
    }
    assert_eq!(
        store.runtime(id(&run)).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn attention_and_uncertain_ownership_project_held_separately_from_requested_state() {
    let (_root, store, value) = setup();
    let run = start(&store, &value);
    let admitted = store
        .admit_initial_team(id(&run), run["evaluationInputs"].as_array().unwrap())
        .unwrap();
    store
        .require_attention(id(&run), "Protected check cleanup is unresolved.")
        .unwrap();
    let held = store.get_comparison(id(&value)).unwrap();
    assert_eq!(held["state"], "held");
    assert_eq!(held["desiredState"], "running");
    assert_eq!(held["cells"][0]["state"], "held");
    assert_eq!(
        held["attentionReason"],
        "Protected check cleanup is unresolved."
    );
    store
        .record_ownership(
            id(&admitted[0]["attempt"]),
            false,
            Some("Owner stop could not be verified."),
        )
        .unwrap();
    store
        .control_comparison(id(&value), &json!({"requestId":key(),"action":"cancel"}))
        .unwrap();
    let cancelled = store.get_comparison(id(&value)).unwrap();
    assert_eq!(cancelled["state"], "held");
    assert_eq!(cancelled["desiredState"], "cancelled");
    assert_eq!(cancelled["cells"][0]["state"], "held");
    assert!(store.coding_slot().unwrap().is_some());
}

#[test]
fn creates_sixteen_counterbalanced_cells_without_tasks_runs_or_catalog_mutation_and_replays_after_reopen()
 {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let input = json!({"requestId":key(),"manifest":manifest("role")});
    let value = store.create_comparison(&input).unwrap();
    let order: Vec<_> = value["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["variant"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        vec![
            "current", "proposed", "proposed", "current", "current", "proposed", "proposed",
            "current", "proposed", "current", "current", "proposed", "proposed", "current",
            "current", "proposed"
        ]
    );
    assert_eq!(
        value["coverage"],
        json!({"planned":16,"observed":0,"finished":0,"remaining":16})
    );
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot["tasks"], json!([]));
    assert_eq!(snapshot["runs"], json!([]));
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.create_comparison(&input).unwrap(), value);
    let mut changed = input;
    changed["manifest"]["design"]["perCellActiveMs"] = json!(170000);
    assert!(store.create_comparison(&changed).is_err());
    assert_eq!(store.comparisons().unwrap().len(), 1);
}

#[test]
fn composition_exposes_only_selected_treatment_and_rejects_hidden_model_or_prompt_changes() {
    for kind in ["role", "principle", "playbook"] {
        let good = manifest(kind);
        validate_manifest(&good).unwrap();
        for field in ["input", "model"] {
            let mut changed = good.clone();
            changed["cases"][0]["inputs"]["proposed"][0][field] = json!("changed");
            assert!(validate_manifest(&changed).is_err(), "{kind}: {field}");
        }
        let mut omitted = good.clone();
        omitted["cases"][0]["inputs"]["proposed"] =
            omitted["cases"][0]["inputs"]["current"].clone();
        assert!(
            validate_manifest(&omitted).is_err(),
            "{kind}: treatment not exposed"
        );
        let mut hash_mismatch = good;
        hash_mismatch["target"]["proposedHash"] = json!("f".repeat(64));
        assert!(validate_manifest(&hash_mismatch).is_err());
    }
}

#[test]
fn strict_order_internal_projection_shared_slot_and_exact_admission_hold_before_process_launch() {
    let (_root, store, value) = setup();
    assert!(
        store
            .create_comparison_cell_run(id(&value), id(&value["cells"][0]))
            .is_err()
    );
    let run = start(&store, &value);
    let run_id = id(&run);
    assert_eq!(run["purpose"], "evaluation");
    assert_eq!(
        run["controlRevision"], 1,
        "Initial launch has a durable control fence"
    );
    assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
    assert_eq!(
        store
            .create_comparison_cell_run(id(&value), id(&value["cells"][0]))
            .unwrap(),
        run
    );
    assert!(
        store
            .create_comparison_cell_run(id(&value), id(&value["cells"][1]))
            .is_err()
    );
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot["tasks"], json!([]));
    assert_eq!(snapshot["dependencyStates"], json!({}));
    assert_eq!(snapshot["queue"]["plans"], json!([]));
    assert_eq!(
        snapshot["runs"][0]["id"], run["id"],
        "Daemon recovery still sees internal runs"
    );
    let input = run["evaluationInputs"][0].clone();
    let mut changed = input.clone();
    changed["input"] = json!("Later configuration must not leak in");
    assert!(store.admit_assignment(run_id, &changed).is_err());
    let team = run["evaluationInputs"].as_array().unwrap();
    store.admit_initial_team(run_id, team).unwrap();
    assert_eq!(
        store.runtime(run_id).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .set_active_limit(run_id, &json!({"activeLimitMs":3600000}))
            .is_err()
    );
    assert!(
        store
            .propose_operator_specialist(run_id, &json!({"requestId":key(),"roleId":team[1]["role"]["id"],"input":"Join work","dependsOn":[],"criteria":[]}), &team[1]["role"], &team[1]["model"])
            .is_err()
    );
}

#[test]
fn pause_fences_claim_and_cancel_retains_slot_until_result_commit_then_lost_reply_replays() {
    let (_root, store, value) = setup();
    let run = start(&store, &value);
    let run_id = id(&run);
    let admitted = store
        .admit_initial_team(run_id, run["evaluationInputs"].as_array().unwrap())
        .unwrap();
    let pause = json!({"requestId":key(),"action":"pause"});
    let paused = store.control_comparison(id(&value), &pause).unwrap();
    assert_eq!(
        store.control_comparison(id(&value), &pause).unwrap(),
        paused
    );
    assert!(store.claim_attempt(id(&admitted[0]["attempt"])).is_err());
    store
        .control_comparison(id(&value), &json!({"requestId":key(),"action":"cancel"}))
        .unwrap();
    assert!(store.coding_slot().unwrap().is_some());
    let result = unavailable();
    let cell_id = id(&value["cells"][0]);
    let receipt = store.record_comparison_result(cell_id, &result).unwrap();
    assert!(store.coding_slot().unwrap().is_none());
    assert_eq!(
        store.record_comparison_result(cell_id, &result).unwrap(),
        receipt
    );
    assert!(
        store
            .record_comparison_result(cell_id, &unavailable())
            .is_err()
    );
}

#[test]
fn cancel_without_attempts_remains_cancelling_until_result_and_slot_settle() {
    for kind in ["role", "principle", "playbook"] {
        let (root, store, value) = setup_for_kind(kind);
        let run = start(&store, &value);
        let cell_id = id(&value["cells"][0]);
        let runtime = store.runtime(id(&run)).unwrap();
        assert!(runtime["assignments"].as_array().unwrap().is_empty());
        assert!(runtime["attempts"].as_array().unwrap().is_empty());
        let paused = store
            .control_comparison(id(&value), &json!({"requestId":key(),"action":"pause"}))
            .unwrap();
        assert_eq!(
            paused["state"], "paused",
            "Pause keeps its existing semantics"
        );
        assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
        let cancelled = store
            .control_comparison(id(&value), &json!({"requestId":key(),"action":"cancel"}))
            .unwrap();
        assert_eq!(cancelled["desiredState"], "cancelled");
        assert_eq!(cancelled["cells"][0]["state"], "cancelling");
        assert!(cancelled["cells"][0]["result"].is_null());
        assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
        assert_eq!(
            cancelled["state"], "cancelling",
            "Empty attempt ownership cannot settle an admitted run"
        );
        assert!(
            store
                .admit_initial_team(id(&run), run["evaluationInputs"].as_array().unwrap())
                .is_err()
        );
        drop(store);
        let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), || 1000).unwrap();
        assert_eq!(
            store.get_comparison(id(&value)).unwrap()["state"],
            "cancelling"
        );
        let mut result = unavailable();
        result["result"]["state"] = json!("cancelled");
        result["result"]["reason"] = json!("Operator cancelled before agent admission.");
        let receipt = store.record_comparison_result(cell_id, &result).unwrap();
        assert!(store.coding_slot().unwrap().is_none());
        let settled = store.get_comparison(id(&value)).unwrap();
        assert_eq!(settled["state"], "cancelled");
        assert_eq!(settled["cells"][0]["result"]["state"], "cancelled");
        assert_eq!(settled["coverage"]["observed"], 0);
        assert_eq!(settled["activeMs"], 0);
        assert_eq!(
            store.record_comparison_result(cell_id, &result).unwrap(),
            receipt
        );
    }
}

#[test]
fn result_rejects_unstopped_attempts_and_fabricated_observation_and_seals_append_events() {
    let (_root, store, value) = setup();
    let run = start(&store, &value);
    let run_id = id(&run);
    let cell_id = id(&value["cells"][0]);
    let admitted = store
        .admit_initial_team(run_id, run["evaluationInputs"].as_array().unwrap())
        .unwrap();
    let attempt = id(&admitted[0]["attempt"]);
    store.claim_attempt(attempt).unwrap();
    assert!(
        store
            .record_comparison_result(cell_id, &unavailable())
            .is_err()
    );
    let event = json!({"requestId":key(),"type":"provider","attemptId":attempt,"payload":{"state":"unavailable"}});
    let event_receipt = store.append_comparison_event(cell_id, &event).unwrap();
    assert_eq!(
        store.append_comparison_event(cell_id, &event).unwrap(),
        event_receipt
    );
    store.record_ownership(attempt, true, None).unwrap();
    let mut fabricated = unavailable();
    fabricated["result"]["state"] = json!("completed");
    fabricated["result"]["observed"] = json!(true);
    fabricated["result"]["scores"] = json!({"correct":true});
    assert!(
        store
            .record_comparison_result(cell_id, &fabricated)
            .is_err()
    );
    store
        .record_comparison_result(cell_id, &unavailable())
        .unwrap();
    assert!(
        store
            .append_comparison_event(
                cell_id,
                &json!({"requestId":key(),"type":"repair","payload":{}})
            )
            .is_err()
    );
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn result_and_slot_release_roll_back_together_on_interrupted_aggregate_update() {
    let (root, store, value) = setup();
    let run = start(&store, &value);
    let cell_id = id(&value["cells"][0]);
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_comparison_receipt BEFORE INSERT ON comparison_requests WHEN NEW.operation='result' BEGIN SELECT RAISE(ABORT,'interrupted receipt'); END;").unwrap();
    let input = unavailable();
    assert!(store.record_comparison_result(cell_id, &input).is_err());
    assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
    assert!(store.get_comparison(id(&value)).unwrap()["cells"][0]["result"].is_null());
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["desiredState"],
        "running"
    );
    db.execute_batch("DROP TRIGGER fail_comparison_receipt")
        .unwrap();
    store.record_comparison_result(cell_id, &input).unwrap();
    assert!(store.coding_slot().unwrap().is_none());
    drop(store);
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), || 1000).unwrap();
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["desiredState"],
        "paused"
    );
    assert!(
        store
            .create_comparison_cell_run(id(&value), id(&value["cells"][1]))
            .is_err()
    );
    store.record_comparison_result(cell_id, &input).unwrap();
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["desiredState"],
        "paused"
    );
}

#[test]
fn frozen_total_budget_charges_run_elapsed_once_and_cannot_be_extended() {
    let root = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(1000));
    let observed = clock.clone();
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), move || {
        observed.load(Ordering::SeqCst)
    })
    .unwrap();
    let mut frozen = manifest("role");
    frozen["design"]["totalActiveMs"] = json!(180000);
    let value = store
        .create_comparison(&json!({"requestId":key(),"manifest":frozen}))
        .unwrap();
    let run = start(&store, &value);
    store.start_execution(id(&run)).unwrap();
    clock.store(181000, Ordering::SeqCst);
    store.account_time(id(&run)).unwrap();
    store
        .record_comparison_result(id(&value["cells"][0]), &unavailable())
        .unwrap();
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["activeMs"],
        180000
    );
    assert!(
        store
            .create_comparison_cell_run(id(&value), id(&value["cells"][1]))
            .is_err()
    );
}

#[test]
fn adoption_binds_exact_hash_and_revision_and_reconciles_duplicate_receipts() {
    let (_root, store, value) = setup();
    let target = &value["manifest"]["target"];
    let input = json!({"requestId":key(),"baseRevision":target["baseRevision"],"proposedHash":target["proposedHash"],"adoptedRevision":target["baseRevision"].as_u64().unwrap()+1});
    let receipt = store
        .record_comparison_adoption(id(&value), &input)
        .unwrap();
    assert_eq!(
        store
            .record_comparison_adoption(id(&value), &input)
            .unwrap(),
        receipt
    );
    let mut duplicate = input.clone();
    duplicate["requestId"] = json!(key());
    assert_eq!(
        store
            .record_comparison_adoption(id(&value), &duplicate)
            .unwrap(),
        receipt
    );
    let mut stale = input;
    stale["requestId"] = json!(key());
    stale["proposedHash"] = json!("e".repeat(64));
    assert!(
        store
            .record_comparison_adoption(id(&value), &stale)
            .is_err()
    );
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["state"],
        "planned",
        "Adoption association does not execute a comparison"
    );
}

#[test]
fn pending_proposal_identity_is_exact_but_committed_creation_replays_after_rejection() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let roles = crewroom_core::roles::Roles::new(root.path().join("roles.json"));
    let mut frozen = manifest("role");
    let proposal = crewroom_core::learning::Learning::new(&store, &roles)
        .propose_operator(&json!({"requestId":key(),"roleId":"implementer","revision":1,"mode":"teach","change":frozen["target"]["proposed"]["instructions"],"rationale":"Compare a bounded instruction proposal"})).unwrap();
    frozen["target"]["proposalId"] = proposal["id"].clone();
    let mut mismatched = frozen.clone();
    mismatched["target"]["proposed"]["instructions"] = json!("Different unsaved proposal");
    assert!(
        store
            .validate_comparison_proposal(&mismatched["target"])
            .is_err()
    );
    let input = json!({"requestId":key(),"manifest":frozen});
    let created = store.create_comparison(&input).unwrap();
    store.decide_learning(id(&proposal), &json!({"requestId":key(),"status":"rejected","reason":"Keep the existing instruction"})).unwrap();
    assert!(
        store
            .validate_comparison_proposal(&input["manifest"]["target"])
            .is_err()
    );
    assert_eq!(
        store.comparison_create_replay(&input).unwrap(),
        Some(created.clone())
    );
    assert_eq!(store.create_comparison(&input).unwrap(), created);
    let mut fresh = input;
    fresh["requestId"] = json!(key());
    assert!(store.create_comparison(&fresh).is_err());
}

#[test]
fn only_evaluated_worker_identity_counts_coverage_and_internal_tasks_cannot_be_requeued() {
    let (_root, store, value) = setup();
    let run = start(&store, &value);
    let team = store
        .admit_initial_team(id(&run), run["evaluationInputs"].as_array().unwrap())
        .unwrap();
    let result = json!({"requestId":key(),"result":{"state":"completed","observed":true,"scores":{"correctness":false,"falseCompletion":true},"cleanup":{"state":"stopped"},"usage":{"available":false,"reason":"Synthetic domain fixture"}}});
    for (index, actor) in team.as_array().unwrap().iter().enumerate() {
        let attempt = id(&actor["attempt"]);
        store.claim_attempt(attempt).unwrap();
        store.record_attempt_identity(attempt, &json!({"hostPid":100+index,"ompPid":200+index,"sessionId":format!("fixture-{index}"),"hostUrl":"http://127.0.0.1:1","pane":format!("fixture-{index}")})).unwrap();
        store.record_ownership(attempt, true, None).unwrap();
        if index == 0 {
            assert!(
                store
                    .record_comparison_result(id(&value["cells"][0]), &result)
                    .is_err(),
                "Coordinator activity is additional to worker coverage"
            );
        }
    }
    let saved = store
        .record_comparison_result(id(&value["cells"][0]), &result)
        .unwrap();
    assert_eq!(saved["result"]["scores"]["correctness"], false);
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["coverage"]["observed"],
        1
    );
    let task_id = run["taskId"].as_str().unwrap();
    for purpose in ["explore", "grill"] {
        assert!(
            store
                .queue_refinement(task_id, &json!({"revision":1,"purpose":purpose}))
                .unwrap_err()
                .to_string()
                .contains("Internal comparison")
        );
    }
    assert!(
        store
            .queue(task_id, &json!({"revision":1}))
            .unwrap_err()
            .to_string()
            .contains("Internal comparison")
    );
}

#[test]
fn schema_29_upgrade_retains_active_stopped_and_history_bytes_and_adds_empty_comparisons() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    drop(store);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE comparison_requests; DROP TABLE comparison_events; DROP TABLE comparison_cells; DROP TABLE comparisons; PRAGMA user_version=29;").unwrap();
    let mut expected = vec![];
    for (i, status) in ["running", "paused", "completed"].iter().enumerate() {
        let task = format!("t-{i}");
        let run = format!("r-{i}");
        let assignment = format!("a-{i}");
        db.execute(
            "INSERT INTO tickets VALUES(?,NULL,NULL,?,?)",
            params![
                task,
                json!({"id":task}).to_string(),
                json!({"revision":1}).to_string()
            ],
        )
        .unwrap();
        let value =
            json!({"id":run,"taskId":task,"status":status,"purpose":"coding","activeMs":1234});
        expected.push(value.clone());
        db.execute(
            "INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",
            params![run, task, value.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO assignments VALUES(?,?,?,?)",
            params![
                assignment,
                run,
                "Retained",
                json!({"id":assignment,"runId":run}).to_string()
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)",
            params![
                format!("attempt-{i}"),
                assignment,
                json!({"state":"idle","ownership":if i==0 {"held"}else{"stopped"}}).to_string()
            ],
        )
        .unwrap();
    }
    drop(db);
    let store = Store::open(&path).unwrap();
    assert!(store.comparisons().unwrap().is_empty());
    for value in expected {
        assert_eq!(store.get_run(id(&value)).unwrap(), value);
    }
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        31
    );
}

fn active_comparison_team(store: &Store, value: &Value) -> (Value, Value) {
    let run = start(store, value);
    let team = store
        .admit_initial_team(id(&run), run["evaluationInputs"].as_array().unwrap())
        .unwrap();
    for (index, actor) in team.as_array().unwrap().iter().enumerate() {
        let attempt = id(&actor["attempt"]);
        store.claim_attempt(attempt).unwrap();
        store.record_attempt_identity(attempt, &json!({"hostPid":100+index,"ompPid":200+index,"sessionId":format!("terminal-fixture-{index}"),"hostUrl":"http://127.0.0.1:1","pane":format!("fixture-{index}")})).unwrap();
    }
    store.start_execution(id(&run)).unwrap();
    (run, team)
}
fn terminal_handoff(team: &Value, status: &str) -> Value {
    let files = json!([{"path":"source.txt","sha256":"a".repeat(64)}]);
    json!({"requestId":key(),"type":"handoff","attemptId":team[1]["attempt"]["id"],"payload":{"arguments":{"status":status,"summary":"Synthetic final assignment status."},"receipt":{"status":status,"summary":"Synthetic final assignment status.","source":{"sha256":hash(&files),"files":files}}}})
}

#[test]
fn terminal_handoff_atomically_seals_complete_and_incomplete_sources_and_replays_after_stop() {
    for (kind, status) in ["role", "principle", "playbook"]
        .into_iter()
        .flat_map(|kind| ["complete", "incomplete"].map(|status| (kind, status)))
    {
        let (root, store, value) = setup_for_kind(kind);
        let (run, team) = active_comparison_team(&store, &value);
        let cell_id = id(&value["cells"][0]);
        let input = terminal_handoff(&team, status);
        let event = store.record_comparison_handoff(cell_id, &input).unwrap();
        let observed = store.get_comparison(id(&value)).unwrap();
        let events = observed["cells"][0]["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], event);
        assert_eq!(events[1]["type"], "finalization");
        assert_eq!(
            events[1]["payload"]["source"],
            input["payload"]["receipt"]["source"]
        );
        assert_eq!(events[1]["payload"]["handoffEventId"], event["id"]);
        assert!(store.start_execution(id(&run)).is_err());
        assert!(
            store
                .replace_attempt(id(&team[1]["assignment"]), &json!({"automatic":false}))
                .is_err()
        );
        store
            .control_comparison(id(&value), &json!({"requestId":key(),"action":"cancel"}))
            .unwrap();
        for actor in team.as_array().unwrap() {
            store
                .record_ownership(id(&actor["attempt"]), true, None)
                .unwrap();
        }
        drop(store);
        let store =
            Store::open_with_clock(&root.path().join("workflow.sqlite"), || 999999).unwrap();
        assert_eq!(
            store.record_comparison_handoff(cell_id, &input).unwrap(),
            event
        );
        assert_eq!(
            store.append_comparison_event(cell_id, &input).unwrap(),
            event
        );
        let mut changed = input.clone();
        changed["payload"]["arguments"]["summary"] = json!("Changed retry");
        assert!(store.record_comparison_handoff(cell_id, &changed).is_err());
        changed = input;
        changed["requestId"] = json!(key());
        assert!(store.record_comparison_handoff(cell_id, &changed).is_err());
        assert_eq!(
            store.get_comparison(id(&value)).unwrap()["cells"][0]["events"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn terminal_handoff_finalization_insert_failure_rolls_back_handoff_request_and_clock() {
    let (root, store, value) = setup();
    let (run, team) = active_comparison_team(&store, &value);
    let store = store.with_clock(|| 2000);
    let cell_id = id(&value["cells"][0]);
    let input = terminal_handoff(&team, "complete");
    let before = store.get_run(id(&run)).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER interrupt_terminal_seal BEFORE INSERT ON comparison_events WHEN json_extract(NEW.data,'$.type')='finalization' BEGIN SELECT RAISE(ABORT,'interrupted final source seal'); END;").unwrap();
    assert!(store.record_comparison_handoff(cell_id, &input).is_err());
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["events"],
        json!([])
    );
    assert_eq!(store.get_run(id(&run)).unwrap(), before);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM comparison_requests WHERE request_id=?",
            [input["requestId"].as_str().unwrap()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER interrupt_terminal_seal")
        .unwrap();
    store.record_comparison_handoff(cell_id, &input).unwrap();
    assert_eq!(store.get_run(id(&run)).unwrap()["activeMs"], 1000);
}

#[test]
fn terminal_handoff_rejects_coordinator_stale_and_unowned_attempts_and_nonrunning_control() {
    for invalid_state in [
        "coordinator",
        "stale",
        "stopped",
        "uncertain",
        "paused",
        "cancelled",
        "attention",
    ] {
        let (root, store, value) = setup();
        let (run, team) = active_comparison_team(&store, &value);
        let cell_id = id(&value["cells"][0]);
        let mut input = terminal_handoff(&team, "complete");
        match invalid_state {
            "coordinator" => input["attemptId"] = team[0]["attempt"]["id"].clone(),
            "stale" => {
                let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
                db.execute(
                    "UPDATE assignments SET data=json_set(data,'$.currentAttemptId',?) WHERE id=?",
                    params![key(), id(&team[1]["assignment"])],
                )
                .unwrap();
            }
            "stopped" => store
                .record_ownership(id(&team[1]["attempt"]), true, None)
                .unwrap(),
            "uncertain" => store
                .record_ownership(
                    id(&team[1]["attempt"]),
                    false,
                    Some("Synthetic unknown owner"),
                )
                .unwrap(),
            "paused" | "cancelled" => {
                store.control_comparison(id(&value), &json!({"requestId":key(),"action":if invalid_state=="paused" {"pause"} else {"cancel"}})).unwrap();
            }
            "attention" => {
                store.require_attention(id(&run), "Synthetic hold").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            store.record_comparison_handoff(cell_id, &input).is_err(),
            "{invalid_state}"
        );
        assert_eq!(
            store.get_comparison(id(&value)).unwrap()["cells"][0]["events"],
            json!([])
        );
    }
}

#[test]
fn terminal_handoff_accounts_elapsed_budget_before_admission_and_replay_survives_later_expiry() {
    for elapsed in [179999, 180000, 180001] {
        let (_root, store, value) = setup();
        let (run, team) = active_comparison_team(&store, &value);
        let store = store.with_clock(move || 1000 + elapsed);
        let input = terminal_handoff(&team, "complete");
        let result = store.record_comparison_handoff(id(&value["cells"][0]), &input);
        assert_eq!(result.is_ok(), elapsed < 180000);
        if elapsed < 180000 {
            assert_eq!(store.get_run(id(&run)).unwrap()["activeMs"], elapsed);
            let store = store.with_clock(|| 999999);
            store.account_time(id(&run)).unwrap();
            assert!(
                store
                    .record_comparison_result(id(&value["cells"][0]), &unavailable())
                    .unwrap_err()
                    .to_string()
                    .contains("sealed source")
            );
            assert_eq!(
                store
                    .record_comparison_handoff(id(&value["cells"][0]), &input)
                    .unwrap(),
                result.unwrap()
            );
        } else {
            assert_eq!(
                store.get_comparison(id(&value)).unwrap()["cells"][0]["events"],
                json!([])
            );
        }
    }
}

#[test]
fn terminal_handoff_rejects_nonterminal_or_inconsistent_receipts_and_malformed_source() {
    let (_root, store, value) = setup();
    let (_run, team) = active_comparison_team(&store, &value);
    let cell_id = id(&value["cells"][0]);
    let good = terminal_handoff(&team, "complete");
    let mut cases = vec![];
    let mut bad = terminal_handoff(&team, "working");
    cases.push(bad.clone());
    bad = good.clone();
    bad["payload"]["receipt"]["status"] = json!("incomplete");
    cases.push(bad.clone());
    bad = good.clone();
    bad["payload"]["receipt"]["source"]["sha256"] = json!("b".repeat(64));
    cases.push(bad.clone());
    for files in [
        json!([]),
        json!([{"path":"../outside","sha256":"a".repeat(64)}]),
        json!([{"path":"same","sha256":"a".repeat(64)},{"path":"same","sha256":"b".repeat(64)}]),
        json!([{"path":"source.txt","sha256":"invalid"}]),
    ] {
        bad = good.clone();
        bad["payload"]["receipt"]["source"] = json!({"sha256":hash(&files),"files":files});
        cases.push(bad.clone());
    }
    for bad in cases {
        assert!(store.append_comparison_event(cell_id, &bad).is_err());
    }
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["events"],
        json!([])
    );
    store.record_comparison_handoff(cell_id, &good).unwrap();
}

#[test]
fn sealed_handoff_rejects_new_mutation_events_and_stale_results_but_preserves_prior_replay_and_assessment()
 {
    let (_root, store, value) = setup();
    let (run, team) = active_comparison_team(&store, &value);
    let cell_id = id(&value["cells"][0]);
    let edit = json!({"requestId":key(),"attemptId":team[1]["attempt"]["id"],"type":"edit","payload":{"receipt":{"hash":"old"}}});
    let old_edit = store.append_comparison_event(cell_id, &edit).unwrap();
    let input = terminal_handoff(&team, "incomplete");
    let handoff = store.record_comparison_handoff(cell_id, &input).unwrap();
    for kind in [
        "edit",
        "repair",
        "intervention",
        "launch",
        "playbook",
        "finalization",
        "check",
    ] {
        assert!(
            store
                .append_comparison_event(
                    cell_id,
                    &json!({"requestId":key(),"type":kind,"payload":{}})
                )
                .is_err(),
            "{kind}"
        );
    }
    assert_eq!(
        store.append_comparison_event(cell_id, &edit).unwrap(),
        old_edit
    );
    let mut second_handoff = input.clone();
    second_handoff["requestId"] = json!(key());
    assert!(
        store
            .record_comparison_handoff(cell_id, &second_handoff)
            .is_err()
    );
    store
        .append_comparison_event(
            cell_id,
            &json!({"requestId":key(),"type":"check","payload":{"receipt":{"oracle":true}}}),
        )
        .unwrap();
    for kind in ["ownership", "usage"] {
        store
            .append_comparison_event(
                cell_id,
                &json!({"requestId":key(),"type":kind,"payload":{"state":"stopped"}}),
            )
            .unwrap();
    }
    for actor in team.as_array().unwrap() {
        store
            .record_ownership(id(&actor["attempt"]), true, None)
            .unwrap();
    }
    for state in ["incomplete", "cancelled", "unavailable"] {
        let mut stale = unavailable();
        stale["result"]["state"] = json!(state);
        assert!(
            store.record_comparison_result(cell_id, &stale).is_err(),
            "{state}"
        );
        assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
    }
    let mut result = json!({"requestId":key(),"result":{"state":"completed","observed":true,"source":{},"scores":{"completionClaim":false,"successful":false},"cleanup":{"state":"stopped"},"usage":{"available":false,"reason":"Synthetic domain fixture"}}});
    assert!(store.record_comparison_result(cell_id, &result).is_err());
    result["result"]["source"] = input["payload"]["receipt"]["source"].clone();
    store.record_comparison_result(cell_id, &result).unwrap();
    assert!(store.coding_slot().unwrap().is_none());
    assert_eq!(
        store.record_comparison_handoff(cell_id, &input).unwrap(),
        handoff
    );
}

#[test]
fn sealed_handoff_denies_new_peer_intents_and_dispatch_but_keeps_exact_replay_and_received_reconciliation()
 {
    let (_root, store, value) = setup();
    let (_run, team) = active_comparison_team(&store, &value);
    let attempt = id(&team[0]["attempt"]);
    let args = json!({"to":"Builder","body":"Synthetic coordination before terminal handoff"});
    let message_key = key();
    let message = store.prepare_message(attempt, &message_key, &args).unwrap();
    store.record_message_stored(id(&message), 1).unwrap();
    let accepted = store.accept_message(id(&message)).unwrap();
    store
        .record_comparison_handoff(id(&value["cells"][0]), &terminal_handoff(&team, "complete"))
        .unwrap();
    assert_eq!(
        store.prepare_message(attempt, &message_key, &args).unwrap(),
        accepted
    );
    assert!(store.prepare_message(attempt, &key(), &args).is_err());
    assert!(store.record_dispatch(id(&message), false).is_err());
    store.record_dispatch(id(&message), true).unwrap();
    assert_eq!(
        store.get_peer_message(id(&message)).unwrap()["disposition"],
        "received"
    );
}

#[test]
fn legacy_handoff_only_journal_fences_continuation_and_recovers_only_its_captured_source() {
    let (root, store, value) = setup();
    let (run, team) = active_comparison_team(&store, &value);
    let cell_id = id(&value["cells"][0]);
    let input = terminal_handoff(&team, "complete");
    let event = store.record_comparison_handoff(cell_id, &input).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    // Reconstruct the previous binary's durable handoff-only shape. This is a
    // synthetic legacy journal fixture, not an interruption of the atomic API.
    db.execute("DELETE FROM comparison_events WHERE cell_id=? AND json_extract(data,'$.type')='finalization'",[cell_id]).unwrap();
    db.execute(
        "DELETE FROM comparison_requests WHERE request_id=?",
        [format!("finalization:{}", id(&run))],
    )
    .unwrap();
    drop(store);
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), || 1000).unwrap();
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["state"],
        "finalizing"
    );
    assert_eq!(
        store.record_comparison_handoff(cell_id, &input).unwrap(),
        event
    );
    assert!(store.start_execution(id(&run)).is_err());
    assert!(
        store
            .prepare_message(
                id(&team[0]["attempt"]),
                &key(),
                &json!({"to":"Builder","body":"No new turn"})
            )
            .is_err()
    );
    assert!(
        store
            .append_comparison_event(
                cell_id,
                &json!({"requestId":key(),"type":"edit","payload":{}})
            )
            .is_err()
    );
    assert!(
        store
            .record_comparison_result(cell_id, &unavailable())
            .is_err()
    );
    let mut sealing = json!({"requestId":format!("finalization:{}",id(&run)),"type":"finalization","payload":{"source":{"sha256":"changed"}}});
    assert!(store.append_comparison_event(cell_id, &sealing).is_err());
    sealing["payload"]["source"] = input["payload"]["receipt"]["source"].clone();
    let sealed = store.append_comparison_event(cell_id, &sealing).unwrap();
    assert_eq!(
        store.append_comparison_event(cell_id, &sealing).unwrap(),
        sealed
    );
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn legacy_multiple_handoffs_seal_the_last_captured_source_and_hold_other_sources() {
    let (root, store, value) = setup();
    let (run, team) = active_comparison_team(&store, &value);
    let cell_id = id(&value["cells"][0]);
    let input = terminal_handoff(&team, "complete");
    let first = store.record_comparison_handoff(cell_id, &input).unwrap();
    let db = Connection::open(root.path().join("workflow.sqlite")).unwrap();
    // The previous runtime allowed additional handoffs before its later seal.
    db.execute("DELETE FROM comparison_events WHERE cell_id=? AND json_extract(data,'$.type')='finalization'",[cell_id]).unwrap();
    db.execute(
        "DELETE FROM comparison_requests WHERE request_id=?",
        [format!("finalization:{}", id(&run))],
    )
    .unwrap();
    let mut last = first.clone();
    last["id"] = json!(key());
    last["requestId"] = json!(key());
    let files = json!([{"path":"source.txt","sha256":"b".repeat(64)}]);
    last["payload"]["receipt"]["source"] = json!({"sha256":hash(&files),"files":files});
    db.execute(
        "INSERT INTO comparison_events VALUES(?,?,?,?)",
        params![id(&last), id(&value), cell_id, last.to_string()],
    )
    .unwrap();
    drop(store);
    let store = Store::open_with_clock(&root.path().join("workflow.sqlite"), || 1000).unwrap();
    let mut sealing = json!({"requestId":format!("finalization:{}",id(&run)),"type":"finalization","payload":{"source":first["payload"]["receipt"]["source"]}});
    assert!(store.append_comparison_event(cell_id, &sealing).is_err());
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    sealing["payload"]["source"] = last["payload"]["receipt"]["source"].clone();
    let sealed = store.append_comparison_event(cell_id, &sealing).unwrap();
    assert_eq!(
        sealed["payload"]["source"],
        last["payload"]["receipt"]["source"]
    );
    assert_eq!(
        store.get_comparison(id(&value)).unwrap()["cells"][0]["events"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn late_coordinator_policy_rejection_keeps_terminal_source_but_requires_owned_actor_cleanup() {
    let (_root, store, value) = setup();
    let (run, team) = active_comparison_team(&store, &value);
    let cell_id = id(&value["cells"][0]);
    let input = terminal_handoff(&team, "complete");
    let handoff = store.record_comparison_handoff(cell_id, &input).unwrap();
    let sealed = store.get_comparison(id(&value)).unwrap()["cells"][0]["events"].clone();
    let reason = "Synthetic late native model receipt rejection";
    let event = json!({"type":"model_policy_rejected","code":"codex_contract","reason":reason});
    store
        .record_agent_event(id(&team[0]["attempt"]), 1, &event)
        .unwrap();
    let runtime = store.runtime(id(&run)).unwrap();
    let held_run = store.get_run(id(&run)).unwrap();
    assert_eq!(held_run["status"], "attention_required");
    assert_eq!(held_run["attentionReason"], reason);
    assert_eq!(runtime["attempts"].as_array().unwrap().len(), 2);
    assert!(
        runtime["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|attempt| attempt["ownership"] == "held")
    );
    assert_eq!(
        store.get_attempt(id(&team[0]["attempt"])).unwrap()["state"],
        "failed"
    );
    let comparison = store.get_comparison(id(&value)).unwrap();
    assert_eq!(comparison["cells"][0]["state"], "held");
    assert_eq!(comparison["cells"][0]["events"], sealed);
    assert!(comparison["cells"][0]["result"].is_null());
    assert_eq!(
        store.record_comparison_handoff(cell_id, &input).unwrap(),
        handoff
    );
    assert_eq!(store.coding_slot().unwrap().unwrap()["id"], run["id"]);
    // This establishes the reachable domain state, not process shutdown. The
    // runtime must still stop both owned actors before assessing the sealed source.
}
