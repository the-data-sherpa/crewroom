//! FOR-76: per-project run capacity (default 1), the optional workspace
//! ceiling, queued admission and the atomic ticket claim.
mod support;
use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    roles: Vec<Value>,
}
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let roles = serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    Fixture { root, store, roles }
}
fn launch() -> Value {
    json!({"team":[{"name":"Coordinator","roleId":"coordinator","coordinator":true,"input":"Inspect accepted work."}]})
}
impl Fixture {
    fn project(&self, name: &str) -> String {
        support::project(&self.store, self.root.path(), name)
    }
    fn task(&self, repo: &str, title: &str) -> String {
        let task = self
            .store
            .create_task(&json!({"title":title,"description":"","repositoryId":repo}))
            .unwrap();
        let task_id = id(&task).to_owned();
        self.store
            .refine(
                &task_id,
                &json!({"revision":1,"sourceRevision":1,"brief":"Keep behavior","criteria":["Retain evidence"]}),
            )
            .unwrap();
        support::accept_plan(&self.store, &task_id);
        task_id
    }
    fn queue(&self, task_id: &str, launch: Option<Value>) -> Value {
        let revision = self.store.get_task(task_id).unwrap()["revision"].clone();
        let mut body = json!({"revision":revision});
        if let Some(launch) = launch {
            body["launch"] = launch;
        }
        self.store
            .queue_with_roles(task_id, &body, &self.roles)
            .unwrap()
    }
    fn plan(&self, repo: &str, title: &str) -> Value {
        let task = self.task(repo, title);
        self.queue(&task, Some(launch()))
    }
    /// Admits the coordinator and claims its attempt: a live agent.
    fn execute(&self, run_id: &str) -> String {
        let role = self
            .roles
            .iter()
            .find(|r| r["id"] == "coordinator")
            .unwrap();
        let team = [
            json!({"name":"Coordinator","coordinator":true,"role":role,"model":{},"input":"Inspect evidence","brief":{"criteria":[],"dependsOn":[]}}),
        ];
        let admitted = self.store.admit_initial_team(run_id, &team).unwrap();
        self.store.set_desired_state(run_id, "running").unwrap();
        let attempt = admitted[0]["attempt"]["id"].as_str().unwrap().to_owned();
        self.store.claim_attempt(&attempt).unwrap();
        attempt
    }
    fn hold(&self, run_id: &str) -> Option<String> {
        self.store.coding_queue().unwrap()["plans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == run_id)
            .and_then(|p| p["holdReason"].as_str().map(str::to_owned))
    }
}

#[test]
fn each_project_admits_up_to_its_capacity_and_queues_the_rest_with_the_reason() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    let a1 = f.plan(&alpha, "Alpha one");
    let a2 = f.plan(&alpha, "Alpha two");
    let b1 = f.plan(&beta, "Beta one");
    // Default capacity 1 per project: one alpha and one beta run hold slots.
    let first = f.store.claim_next_coding_slot().unwrap().unwrap();
    let second = f.store.claim_next_coding_slot().unwrap().unwrap();
    assert_eq!(first["id"], a1["id"]);
    assert_eq!(second["id"], b1["id"]);
    assert!(f.store.claim_next_coding_slot().unwrap().is_none());
    assert_eq!(
        f.hold(id(&a2)).as_deref(),
        Some("Waiting for a slot in alpha: 1 of 1 running")
    );
    let queue = f.store.coding_queue().unwrap();
    assert_eq!(queue["slots"].as_array().unwrap().len(), 2);
    assert_eq!(queue["slot"]["runId"], a1["id"]);
    let projects = queue["capacity"]["projects"].as_array().unwrap();
    let alpha_view = projects
        .iter()
        .find(|p| p["repositoryId"] == alpha.as_str())
        .unwrap();
    assert_eq!(alpha_view["capacity"], 1);
    assert_eq!(alpha_view["occupied"], 1);
    assert_eq!(queue["capacity"]["occupied"], 2);
    // A manual start of the queued plan is refused by the store with the
    // same reason; the service turns this into a queued start request.
    let refused = f.store.claim_coding_slot(id(&a2), true).unwrap_err();
    assert_eq!(
        refused.to_string(),
        "Waiting for a slot in alpha: 1 of 1 running"
    );
    // Raising alpha's capacity admits the queued plan.
    f.store
        .set_execution_capacity(&json!({"repositoryId":alpha,"capacity":2}))
        .unwrap();
    assert_eq!(
        f.store.claim_next_coding_slot().unwrap().unwrap()["id"],
        a2["id"]
    );
}

#[test]
fn the_workspace_ceiling_bounds_all_projects() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    f.plan(&alpha, "Alpha one");
    let b1 = f.plan(&beta, "Beta one");
    f.store
        .set_execution_capacity(&json!({"workspaceCeiling":1}))
        .unwrap();
    assert!(f.store.claim_next_coding_slot().unwrap().is_some());
    assert!(f.store.claim_next_coding_slot().unwrap().is_none());
    assert_eq!(
        f.hold(id(&b1)).as_deref(),
        Some("Waiting for a workspace slot: 1 of 1 running")
    );
    f.store
        .set_execution_capacity(&json!({"workspaceCeiling":null}))
        .unwrap();
    assert_eq!(
        f.store.claim_next_coding_slot().unwrap().unwrap()["id"],
        b1["id"]
    );
}

#[test]
fn capacity_settings_are_validated() {
    let f = fixture();
    let alpha = f.project("alpha");
    for body in [
        json!({"repositoryId":alpha,"capacity":0}),
        json!({"repositoryId":alpha,"capacity":17}),
        json!({"repositoryId":alpha}),
        json!({"workspaceCeiling":0}),
        json!({"workspaceCeiling":"2"}),
        json!({}),
        json!({"other":1}),
    ] {
        assert!(f.store.set_execution_capacity(&body).is_err(), "{body}");
    }
    assert!(
        f.store
            .set_execution_capacity(&json!({"repositoryId":"missing","capacity":2}))
            .is_err()
    );
    let saved = f
        .store
        .set_execution_capacity(&json!({"repositoryId":alpha,"capacity":3,"workspaceCeiling":4}))
        .unwrap();
    assert_eq!(saved["projects"][&alpha], 3);
    assert_eq!(saved["workspaceCeiling"], 4);
    // Capacity 1 is the default and is not stored.
    let reset = f
        .store
        .set_execution_capacity(&json!({"repositoryId":alpha,"capacity":1}))
        .unwrap();
    assert!(reset["projects"].get(&alpha).is_none());
    assert_eq!(f.store.project_capacity(&alpha).unwrap(), 1);
}

#[test]
fn a_freed_slot_admits_the_next_queued_plan_after_a_restart() {
    let f = fixture();
    let alpha = f.project("alpha");
    let a1 = f.plan(&alpha, "Alpha one");
    let a2 = f.plan(&alpha, "Alpha two");
    f.store.claim_next_coding_slot().unwrap().unwrap();
    assert!(f.store.claim_next_coding_slot().unwrap().is_none());
    // The holder is cancelled: the slot is freed durably.
    f.store.set_desired_state(id(&a1), "cancelled").unwrap();
    let path = f.root.path().join("workflow.sqlite");
    let Fixture { root, store, .. } = f;
    drop(store);
    let reopened = Store::open(&path).unwrap().with_clock(|| 1000);
    assert!(reopened.coding_slot().unwrap().is_none());
    assert_eq!(
        reopened.claim_next_coding_slot().unwrap().unwrap()["id"],
        a2["id"]
    );
    drop(root);
}

#[test]
fn a_paused_run_with_unconfirmed_ownership_keeps_its_slot() {
    let f = fixture();
    let alpha = f.project("alpha");
    let a1 = f.plan(&alpha, "Alpha one");
    let a2 = f.plan(&alpha, "Alpha two");
    let run = f.store.claim_next_coding_slot().unwrap().unwrap();
    let attempt = f.execute(id(&run));
    f.store
        .record_ownership(&attempt, false, Some("probe"))
        .unwrap();
    f.store.begin_pause(id(&a1), Some("recovery")).unwrap();
    f.store.finish_pause(id(&a1)).unwrap();
    // Even if its slot row were released, unconfirmed ownership occupies it.
    let db = Connection::open(f.root.path().join("workflow.sqlite")).unwrap();
    db.execute("DELETE FROM coding_slots", []).unwrap();
    assert_eq!(
        f.hold(id(&a2)).as_deref(),
        Some("Waiting for a slot in alpha: 1 of 1 running")
    );
    assert!(f.store.claim_next_coding_slot().unwrap().is_none());
    f.store.record_ownership(&attempt, true, None).unwrap();
    assert_eq!(
        f.store.claim_next_coding_slot().unwrap().unwrap()["id"],
        a2["id"]
    );
}

#[test]
fn a_paused_run_with_stopped_ownership_frees_its_slot_and_stays_resumable() {
    let f = fixture();
    let alpha = f.project("alpha");
    let a1 = f.plan(&alpha, "Alpha one");
    let a2 = f.plan(&alpha, "Alpha two");
    let a3 = f.plan(&alpha, "Alpha three");
    let run = f.store.claim_next_coding_slot().unwrap().unwrap();
    assert_eq!(run["id"], a1["id"]);
    let attempt = f.execute(id(&a1));
    // An operator pause: still holding while the stop is unconfirmed.
    f.store.request_pause(id(&a1)).unwrap();
    f.store.begin_pause(id(&a1), None).unwrap();
    assert!(!f.store.release_paused_coding_slot(id(&a1)).unwrap());
    assert!(f.store.holds_slot(id(&a1)).unwrap());
    f.store.record_ownership(&attempt, true, None).unwrap();
    let paused = f.store.finish_pause(id(&a1)).unwrap();
    assert_eq!(paused["status"], "paused");
    assert_eq!(paused["pauseReason"], "operator");
    assert_eq!(paused["slotRelease"]["reason"], "paused");
    assert!(!f.store.holds_slot(id(&a1)).unwrap());
    // The next queued ticket in the project is admitted.
    assert_eq!(
        f.store.claim_next_coding_slot().unwrap().unwrap()["id"],
        a2["id"]
    );
    // Resume while the project is full queues the Resume ahead of new plans.
    assert!(f.store.queue_review_resume(id(&a1)).unwrap());
    assert!(f.store.get_run(id(&a1)).unwrap()["resumeRequest"].is_object());
    assert!(f.store.reclaim_coding_slot(id(&a1), false).is_err());
    f.store.set_desired_state(id(&a2), "cancelled").unwrap();
    assert!(
        f.store.claim_next_coding_slot().unwrap().is_none(),
        "The queued Resume keeps priority over {}",
        id(&a3)
    );
    assert!(!f.store.queue_review_resume(id(&a1)).unwrap());
    f.store.reclaim_coding_slot(id(&a1), false).unwrap();
    assert!(f.store.holds_slot(id(&a1)).unwrap());
    assert!(
        f.store
            .get_run(id(&a1))
            .unwrap()
            .get("slotRelease")
            .is_none()
    );
}

#[test]
fn a_transient_or_review_pause_keeps_its_slot() {
    let f = fixture();
    let alpha = f.project("alpha");
    let a1 = f.plan(&alpha, "Alpha one");
    f.plan(&alpha, "Alpha two");
    f.store.claim_next_coding_slot().unwrap().unwrap();
    let attempt = f.execute(id(&a1));
    f.store.record_ownership(&attempt, true, None).unwrap();
    // A stop while the run should keep running (recovery, rebrief) keeps it.
    f.store.begin_pause(id(&a1), Some("recovery")).unwrap();
    let stopped = f.store.finish_pause(id(&a1)).unwrap();
    assert_eq!(stopped["status"], "paused");
    assert_eq!(stopped["desiredState"], "running");
    assert!(f.store.holds_slot(id(&a1)).unwrap());
    // The human review gate has its own release.
    f.store.set_desired_state(id(&a1), "paused").unwrap();
    f.store.begin_pause(id(&a1), Some("validation")).unwrap();
    f.store.finish_pause(id(&a1)).unwrap();
    assert!(!f.store.release_paused_coding_slot(id(&a1)).unwrap());
    assert!(f.store.holds_slot(id(&a1)).unwrap());
    assert!(f.store.claim_next_coding_slot().unwrap().is_none());
}

#[test]
fn a_queued_start_request_admits_no_agent_until_a_slot_frees() {
    let f = fixture();
    let alpha = f.project("alpha");
    let a1 = f.plan(&alpha, "Alpha one");
    let task = f.task(&alpha, "Alpha manual");
    let manual = f.queue(&task, None);
    assert_eq!(f.hold(id(&manual)).as_deref(), Some("Needs launch details"));
    f.store.claim_next_coding_slot().unwrap().unwrap();
    let queued = f
        .store
        .queue_start_request(id(&manual), &json!({"team":launch()["team"]}))
        .unwrap();
    assert_eq!(
        queued["holdReason"],
        "Waiting for a slot in alpha: 1 of 1 running"
    );
    assert_eq!(queued["status"], "awaiting_runner");
    let runtime = f.store.runtime(id(&manual)).unwrap();
    assert!(runtime["assignments"].as_array().unwrap().is_empty());
    // Not displayed as executing: no activity, no slot.
    let snapshot = f.store.snapshot().unwrap();
    assert!(
        !snapshot["activity"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["runId"] == manual["id"])
    );
    assert!(!f.store.can_dispatch_start_request(id(&manual)).unwrap());
    f.store.set_desired_state(id(&a1), "cancelled").unwrap();
    let claimed = f.store.claim_next_coding_slot().unwrap().unwrap();
    assert_eq!(claimed["id"], manual["id"]);
    assert_eq!(claimed["startRequest"]["input"]["team"], launch()["team"]);
    assert!(f.store.can_dispatch_start_request(id(&manual)).unwrap());
}

#[test]
fn a_ticket_never_has_two_live_coordinators_even_after_a_crash() {
    let f = fixture();
    let alpha = f.project("alpha");
    let task = f.task(&alpha, "Alpha one");
    // Capacity is not what refuses the second run below.
    f.store
        .set_execution_capacity(&json!({"repositoryId":alpha,"capacity":4}))
        .unwrap();
    let first = f.queue(&task, Some(launch()));
    f.store.claim_coding_slot(id(&first), false).unwrap();
    let attempt = f.execute(id(&first));
    // A crash leaves the first run cancelled with its agent's ownership
    // unconfirmed (the service never confirmed the process stopped).
    f.store
        .record_ownership(&attempt, false, Some("lost"))
        .unwrap();
    let db = Connection::open(f.root.path().join("workflow.sqlite")).unwrap();
    let mut cancelled = f.store.get_run(id(&first)).unwrap();
    cancelled["status"] = json!("cancelled");
    cancelled["desiredState"] = json!("cancelled");
    db.execute(
        "UPDATE runs SET data=? WHERE id=?",
        rusqlite::params![cancelled.to_string(), id(&first)],
    )
    .unwrap();
    db.execute("DELETE FROM coding_slots", []).unwrap();
    let second = f.queue(&task, Some(launch()));
    let reason = f.hold(id(&second)).unwrap();
    assert!(
        reason.contains("already has a live coordinator"),
        "{reason}"
    );
    assert!(f.store.claim_next_coding_slot().unwrap().is_none());
    let refused = f.store.claim_coding_slot(id(&second), true).unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("already has a live coordinator")
    );
    // Forcing the slot past the queue still cannot claim an attempt.
    db.execute(
        "INSERT INTO coding_slots(run_id,task_id,project) VALUES(?,?,?)",
        rusqlite::params![id(&second), task, alpha],
    )
    .unwrap();
    let role = f.roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let team = [
        json!({"name":"Coordinator","coordinator":true,"role":role,"model":{},"input":"Inspect evidence","brief":{"criteria":[],"dependsOn":[]}}),
    ];
    let admitted = f.store.admit_initial_team(id(&second), &team).unwrap();
    f.store.set_desired_state(id(&second), "running").unwrap();
    let error = f
        .store
        .claim_attempt(admitted[0]["attempt"]["id"].as_str().unwrap())
        .unwrap_err();
    assert!(
        error.to_string().contains("already has a live coordinator"),
        "{error}"
    );
    // The slot table itself refuses a second holder for one ticket.
    assert!(
        db.execute(
            "INSERT INTO coding_slots(run_id,task_id,project) VALUES(?,?,?)",
            rusqlite::params![id(&first), task, alpha],
        )
        .is_err()
    );
    // Once the lost agent is confirmed stopped, recovery may proceed.
    f.store.record_ownership(&attempt, true, None).unwrap();
    f.store
        .claim_attempt(admitted[0]["attempt"]["id"].as_str().unwrap())
        .unwrap();
}

#[test]
fn concurrent_claims_fill_each_project_once() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    for i in 0..2 {
        f.plan(&alpha, &format!("Alpha {i}"));
        f.plan(&beta, &format!("Beta {i}"));
    }
    let barrier = Arc::new(Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let path = f.root.path().join("workflow.sqlite");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap().with_clock(|| 1000);
                barrier.wait();
                store.claim_next_coding_slot().unwrap()
            })
        })
        .collect();
    let claimed: Vec<_> = threads
        .into_iter()
        .filter_map(|t| t.join().unwrap())
        .collect();
    assert_eq!(claimed.len(), 2);
    let slots = f.store.coding_slots().unwrap();
    let mut projects: Vec<_> = slots
        .iter()
        .map(|r| {
            r["taskSnapshot"]["repositoryId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    projects.sort();
    let mut expected = vec![alpha, beta];
    expected.sort();
    assert_eq!(projects, expected);
}

#[test]
fn two_projects_execute_concurrently_and_a_third_run_waits() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    let a1 = f
        .store
        .claim_coding_slot(id(&f.plan(&alpha, "A1")), false)
        .unwrap();
    let b1 = f
        .store
        .claim_coding_slot(id(&f.plan(&beta, "B1")), false)
        .unwrap();
    f.execute(id(&a1));
    f.execute(id(&b1));
    let executing = f.store.executing_runs().unwrap();
    assert_eq!(executing.as_array().unwrap().len(), 2);
    // A third alpha run forced past the queue is refused at attempt claim.
    let a2 = f.plan(&alpha, "A2");
    let db = Connection::open(f.root.path().join("workflow.sqlite")).unwrap();
    db.execute(
        "INSERT INTO coding_slots(run_id,task_id,project) VALUES(?,?,?)",
        rusqlite::params![id(&a2), a2["taskId"].as_str().unwrap(), alpha],
    )
    .unwrap();
    let role = f.roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let team = [
        json!({"name":"Coordinator","coordinator":true,"role":role,"model":{},"input":"Inspect evidence","brief":{"criteria":[],"dependsOn":[]}}),
    ];
    let admitted = f.store.admit_initial_team(id(&a2), &team).unwrap();
    f.store.set_desired_state(id(&a2), "running").unwrap();
    let error = f
        .store
        .claim_attempt(admitted[0]["attempt"]["id"].as_str().unwrap())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Waiting for a slot in alpha: 1 of 1 running"
    );
}

#[test]
fn a_retained_single_slot_migrates_to_its_project() {
    let f = fixture();
    let alpha = f.project("alpha");
    let a1 = f.plan(&alpha, "A1");
    let path = f.root.path().join("workflow.sqlite");
    let Fixture { root, store, .. } = f;
    drop(store);
    // Rebuild the version-35 single-slot table holding a1.
    let db = Connection::open(&path).unwrap();
    db.execute_batch(&format!(
        "DROP TABLE coding_slots; DROP TABLE execution_settings; CREATE TABLE coding_slot (id INTEGER PRIMARY KEY CHECK(id=1), run_id TEXT REFERENCES runs(id)); INSERT INTO coding_slot VALUES(1,'{}'); PRAGMA user_version=35;",
        id(&a1)
    ))
    .unwrap();
    drop(db);
    let reopened = Store::open(&path).unwrap().with_clock(|| 1000);
    let slots = reopened.coding_slots().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["id"], a1["id"]);
    assert_eq!(
        reopened
            .capacity_hold(&reopened.get_run(id(&a1)).unwrap())
            .unwrap(),
        None
    );
    assert_eq!(
        reopened.execution_settings().unwrap()["workspaceCeiling"],
        Value::Null
    );
    drop(root);
}
