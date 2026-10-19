//! FOR-79: per-project setting overrides resolve project → workspace →
//! default, show their source, and never alter captured assignments.
mod support;
use crewroom_core::project_settings::{
    DEFAULT, PROJECT, WORKSPACE, effective_roles, overriding_projects, resolve,
};
use crewroom_core::store::Store;
use serde_json::{Value, json};

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    roles: Vec<Value>,
    principles: Vec<Value>,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let roles = serde_json::from_str(include_str!("../src/default-roles.json")).unwrap();
    let principles = vec![
        json!({"id":"small-changes","name":"Small changes","text":"Prefer small reviewable changes.","revision":1}),
        json!({"id":"no-network","name":"No network","text":"Do not use the network.","revision":1}),
    ];
    Fixture {
        root,
        store,
        roles,
        principles,
    }
}
impl Fixture {
    fn project(&self, name: &str) -> String {
        support::project(&self.store, self.root.path(), name)
    }
    fn save(&self, body: Value) -> crewroom_core::Result<Value> {
        self.store
            .save_project_settings(&body, &self.roles, &self.principles)
    }
}
fn saved_models() -> Value {
    json!({"revision":2,"budget":"medium","jobs":{"code":"workspace-code","judgment":"workspace-judgment","review":"workspace-review","fast":"workspace-fast"},"legacyDefault":false})
}

#[test]
fn a_setting_resolves_project_then_workspace_then_default_with_its_chain() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    let unsaved = json!({"revision":0,"budget":"large","jobs":{"code":""},"legacyDefault":false});
    let settings = f.store.project_settings().unwrap();
    let budget = resolve("models.budget", &settings, Some(&alpha), &unsaved, &f.roles);
    assert_eq!(
        (budget.value.clone(), budget.source),
        (json!("large"), DEFAULT)
    );
    let budget = resolve(
        "models.budget",
        &settings,
        Some(&alpha),
        &saved_models(),
        &f.roles,
    );
    assert_eq!(
        (budget.value.clone(), budget.source),
        (json!("medium"), WORKSPACE)
    );

    let settings = f
        .save(json!({"repositoryId":alpha,"set":{"models.budget":"small","models.jobs.code":"project-code","instructions":"Run the slow suite.","principles":["no-network"],"roles.reviewer":false,"capacity":3}}))
        .unwrap();
    let budget = resolve(
        "models.budget",
        &settings,
        Some(&alpha),
        &saved_models(),
        &f.roles,
    );
    assert_eq!(
        (budget.value.clone(), budget.source),
        (json!("small"), PROJECT)
    );
    assert_eq!(
        budget.to_json()["chain"],
        json!([{"source":"default","value":"large"},{"source":"workspace","value":"medium"},{"source":"project","value":"small"}])
    );
    // Beta has no override and inherits.
    let other = resolve(
        "models.budget",
        &settings,
        Some(&beta),
        &saved_models(),
        &f.roles,
    );
    assert_eq!(other.source, WORKSPACE);
    let reviewer = resolve(
        "roles.reviewer",
        &settings,
        Some(&alpha),
        &saved_models(),
        &f.roles,
    );
    assert_eq!((reviewer.value, reviewer.source), (json!(false), PROJECT));
    assert_eq!(f.store.project_capacity(&alpha).unwrap(), 3);
    assert_eq!(f.store.project_capacity(&beta).unwrap(), 1);
    assert_eq!(
        overriding_projects("models.budget", &settings),
        vec![alpha.clone()]
    );
    assert_eq!(
        overriding_projects("capacity", &settings),
        vec![alpha.clone()]
    );
    assert!(overriding_projects("models.jobs.fast", &settings).is_empty());

    // Reset inherits again; the other overrides stay.
    let settings = f
        .save(json!({"repositoryId":alpha,"reset":["models.budget","capacity"]}))
        .unwrap();
    let budget = resolve(
        "models.budget",
        &settings,
        Some(&alpha),
        &saved_models(),
        &f.roles,
    );
    assert_eq!(budget.source, WORKSPACE);
    assert_eq!(f.store.project_capacity(&alpha).unwrap(), 1);
    assert_eq!(
        settings["projects"][&alpha]["models.jobs.code"],
        "project-code"
    );
}

#[test]
fn a_workspace_change_never_alters_a_project_override() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    f.save(json!({"repositoryId":alpha,"set":{"instructions":"Alpha only.","capacity":2}}))
        .unwrap();
    let settings = f
        .save(json!({"repositoryId":null,"set":{"instructions":"Everyone.","principles":["small-changes"],"capacity":4}}))
        .unwrap();
    assert_eq!(settings["projects"][&alpha]["instructions"], "Alpha only.");
    assert_eq!(settings["projects"][&alpha]["capacity"], 2);
    let none = json!({});
    let alpha_text = resolve("instructions", &settings, Some(&alpha), &none, &f.roles);
    assert_eq!(
        (alpha_text.value, alpha_text.source),
        (json!("Alpha only."), PROJECT)
    );
    let beta_text = resolve("instructions", &settings, Some(&beta), &none, &f.roles);
    assert_eq!(
        (beta_text.value, beta_text.source),
        (json!("Everyone."), WORKSPACE)
    );
    // The workspace default capacity applies only where no override exists.
    assert_eq!(f.store.project_capacity(&alpha).unwrap(), 2);
    assert_eq!(f.store.project_capacity(&beta).unwrap(), 4);
    // Resetting the workspace value leaves the override in place.
    let settings = f
        .save(json!({"repositoryId":null,"reset":["instructions","capacity"]}))
        .unwrap();
    assert_eq!(settings["projects"][&alpha]["instructions"], "Alpha only.");
    assert_eq!(f.store.project_capacity(&beta).unwrap(), 1);
    // The FOR-76 ceiling and capacity endpoint keep the overrides.
    f.store
        .set_execution_capacity(&json!({"workspaceCeiling":3}))
        .unwrap();
    assert_eq!(
        f.store.project_settings().unwrap()["projects"][&alpha]["instructions"],
        "Alpha only."
    );
}

#[test]
fn invalid_scopes_keys_and_values_are_refused() {
    let f = fixture();
    let alpha = f.project("alpha");
    for body in [
        json!({"repositoryId":alpha}),
        json!({"repositoryId":alpha,"set":{"unknown":1}}),
        json!({"repositoryId":alpha,"set":{"models.budget":"huge"}}),
        json!({"repositoryId":alpha,"set":{"models.jobs.code":"  "}}),
        json!({"repositoryId":alpha,"set":{"models.jobs.other":"x"}}),
        json!({"repositoryId":alpha,"set":{"roles.missing":true}}),
        json!({"repositoryId":alpha,"set":{"roles.reviewer":"no"}}),
        json!({"repositoryId":alpha,"set":{"principles":["missing"]}}),
        json!({"repositoryId":alpha,"set":{"capacity":0}}),
        json!({"repositoryId":alpha,"set":{"capacity":17}}),
        json!({"repositoryId":alpha,"set":{"capacity":2},"reset":["capacity"]}),
        json!({"repositoryId":alpha,"set":{"capacity":2},"extra":1}),
        // Models and roles are workspace settings with their own editors.
        json!({"repositoryId":null,"set":{"models.budget":"small"}}),
        json!({"repositoryId":null,"set":{"roles.reviewer":false}}),
        json!({"repositoryId":"missing","set":{"capacity":2}}),
        json!({"repositoryId":7,"set":{"capacity":2}}),
    ] {
        assert!(f.save(body.clone()).is_err(), "{body}");
    }
    assert_eq!(
        f.store.project_settings().unwrap(),
        json!({"workspace":{},"projects":{}})
    );
}

#[test]
fn admission_roles_apply_the_project_and_captured_assignments_keep_their_values() {
    let f = fixture();
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    f.save(json!({"repositoryId":null,"set":{"principles":["small-changes"]}}))
        .unwrap();
    let settings = f
        .save(json!({"repositoryId":alpha,"set":{"roles.reviewer":false,"roles.sre":true,"instructions":"Use the alpha fixtures.","models.jobs.code":"project-code","models.budget":"small"}}))
        .unwrap();
    let roles = effective_roles(&f.roles, &settings, &alpha);
    let role = |id: &str| roles.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(role("reviewer")["enabled"], false);
    let coordinator = role("coordinator");
    assert_eq!(
        coordinator["projectSettings"]["instructions"],
        json!({"text":"Use the alpha fixtures.","source":"project"})
    );
    let original = f.roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    assert_eq!(coordinator["instructions"], original["instructions"]);
    // Resolving twice changes nothing.
    assert_eq!(effective_roles(&roles, &settings, &alpha), roles);
    assert!(
        coordinator["principles"]
            .as_array()
            .unwrap()
            .contains(&json!("small-changes"))
    );
    assert_eq!(
        coordinator["projectSettings"]["models"],
        json!({"budget":"small","jobs":{"code":"project-code"}})
    );
    assert_eq!(coordinator["projectSettings"]["repositoryId"], alpha);
    // Beta inherits the workspace principles and nothing else.
    let beta_roles = effective_roles(&f.roles, &settings, &beta);
    let beta_reviewer = beta_roles.iter().find(|r| r["id"] == "reviewer").unwrap();
    let original = f.roles.iter().find(|r| r["id"] == "reviewer").unwrap();
    assert_eq!(beta_reviewer["enabled"], original["enabled"]);
    assert!(beta_reviewer["projectSettings"].get("models").is_none());
    assert_eq!(
        beta_reviewer["projectSettings"]["principles"]["source"],
        "workspace"
    );

    // Admit the coordinator with the resolved role, then change the override.
    let task = f
        .store
        .create_task(&json!({"title":"Alpha","description":"","repositoryId":alpha}))
        .unwrap();
    let task_id = task["id"].as_str().unwrap().to_owned();
    f.store
        .refine(
            &task_id,
            &json!({"revision":1,"sourceRevision":1,"brief":"Keep behavior","criteria":["Retain evidence"]}),
        )
        .unwrap();
    support::accept_plan(&f.store, &task_id);
    let run = f
        .store
        .queue_with_roles(
            &task_id,
            &json!({"revision":f.store.get_task(&task_id).unwrap()["revision"]}),
            &f.roles,
        )
        .unwrap();
    let run_id = run["id"].as_str().unwrap();
    let team = [
        json!({"name":"Coordinator","coordinator":true,"role":coordinator,"model":{},"input":"Inspect evidence","brief":{"criteria":[],"dependsOn":[]}}),
    ];
    f.store.admit_initial_team(run_id, &team).unwrap();
    let captured = f.store.runtime(run_id).unwrap()["assignments"][0]["role"].clone();
    f.save(json!({"repositoryId":alpha,"set":{"instructions":"Changed later."},"reset":["models.jobs.code"]}))
        .unwrap();
    f.save(json!({"repositoryId":null,"reset":["principles"]}))
        .unwrap();
    assert_eq!(
        f.store.runtime(run_id).unwrap()["assignments"][0]["role"],
        captured
    );
    assert_eq!(
        captured["projectSettings"]["instructions"]["text"],
        "Use the alpha fixtures."
    );
    assert_eq!(captured["enabled"], true);
}

#[test]
fn the_snapshot_reports_settings_and_capacity_with_the_workspace_default() {
    let f = fixture();
    let alpha = f.project("alpha");
    f.save(json!({"repositoryId":null,"set":{"capacity":2}}))
        .unwrap();
    let snapshot = f.store.snapshot().unwrap();
    assert_eq!(snapshot["projectSettings"]["workspace"]["capacity"], 2);
    let view = snapshot["queue"]["capacity"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["repositoryId"] == alpha)
        .unwrap()
        .clone();
    assert_eq!(view["capacity"], 2);
}

fn launch_with(role: &str) -> Value {
    json!({"team":[
        {"name":"Coordinator","roleId":"coordinator","coordinator":true,"input":"Inspect accepted work."},
        {"name":"Specialist","roleId":role,"coordinator":false,"input":"Assess the change."}
    ]})
}
impl Fixture {
    fn ready_task(&self, repo: &str, title: &str) -> String {
        let task = self
            .store
            .create_task(&json!({"title":title,"description":"","repositoryId":repo}))
            .unwrap();
        let task_id = task["id"].as_str().unwrap().to_owned();
        self.store
            .refine(
                &task_id,
                &json!({"revision":1,"sourceRevision":1,"brief":"Keep behavior","criteria":["Retain evidence"]}),
            )
            .unwrap();
        support::accept_plan(&self.store, &task_id);
        task_id
    }
    fn queue_launch(&self, task_id: &str, launch: Option<Value>) -> crewroom_core::Result<Value> {
        let revision = self.store.get_task(task_id).unwrap()["revision"].clone();
        let mut body = json!({"revision":revision});
        if let Some(launch) = launch {
            body["launch"] = launch;
        }
        self.store.queue_with_roles(task_id, &body, &self.roles)
    }
    fn change_launch(&self, run_id: &str, launch: Value) -> crewroom_core::Result<Value> {
        self.store.set_queue_launch(
            run_id,
            &json!({"requestId":uuid::Uuid::new_v4().to_string(),"launch":launch}),
            &self.roles,
        )
    }
}

#[test]
fn saving_launch_details_checks_the_projects_roles_in_both_directions() {
    let mut f = fixture();
    // The workspace disables sre; alpha enables it and disables reviewer.
    for role in &mut f.roles {
        if role["id"] == "sre" {
            role["enabled"] = json!(false);
        }
    }
    let alpha = f.project("alpha");
    let beta = f.project("beta");
    f.save(json!({"repositoryId":alpha,"set":{"roles.sre":true,"roles.reviewer":false}}))
        .unwrap();

    // Queue with a launch (POST /api/tasks/:id/runs).
    let task = f.ready_task(&alpha, "Alpha sre");
    let run = f.queue_launch(&task, Some(launch_with("sre"))).unwrap();
    assert_eq!(run["launch"]["team"][1]["roleId"], "sre");
    let task = f.ready_task(&alpha, "Alpha reviewer");
    let refused = f
        .queue_launch(&task, Some(launch_with("reviewer")))
        .unwrap_err();
    assert!(refused.to_string().contains("unavailable"), "{refused}");
    // Beta inherits the workspace: sre stays unavailable, reviewer is allowed.
    let task = f.ready_task(&beta, "Beta sre");
    let refused = f.queue_launch(&task, Some(launch_with("sre"))).unwrap_err();
    assert!(refused.to_string().contains("unavailable"), "{refused}");
    let run = f
        .queue_launch(&task, Some(launch_with("reviewer")))
        .unwrap();
    assert_eq!(run["launch"]["team"][1]["roleId"], "reviewer");

    // Change launch details on a queued plan (POST /api/queue/:id/launch).
    let task = f.ready_task(&alpha, "Alpha later");
    let queued = f.queue_launch(&task, None).unwrap();
    let run_id = queued["id"].as_str().unwrap();
    let saved = f.change_launch(run_id, launch_with("sre")).unwrap();
    assert_eq!(saved["launch"]["team"][1]["roleId"], "sre");
    let refused = f
        .change_launch(run_id, launch_with("reviewer"))
        .unwrap_err();
    assert!(refused.to_string().contains("unavailable"), "{refused}");
}

#[test]
fn a_principle_that_settings_apply_cannot_be_deleted() {
    let f = fixture();
    let alpha = f.project("alpha");
    let roles = crewroom_core::roles::Roles::new(f.root.path().join("roles.json"));
    let catalog = crewroom_core::principles::Principles::beside_roles(roles.path());
    for p in &f.principles {
        let mut p = p.clone();
        p["revision"] = json!(0);
        catalog.save(&p).unwrap();
    }
    f.save(json!({"repositoryId":null,"set":{"principles":["small-changes"]}}))
        .unwrap();
    f.save(json!({"repositoryId":alpha,"set":{"principles":["no-network"]}}))
        .unwrap();
    let settings = f.store.project_settings().unwrap();
    let refused = catalog
        .delete("small-changes", 1, &roles, &settings)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("workspace settings"), "{refused}");
    let refused = catalog
        .delete("no-network", 1, &roles, &settings)
        .unwrap_err()
        .to_string();
    assert!(refused.contains(&format!("project {alpha}")), "{refused}");
    assert_eq!(catalog.read().unwrap().len(), 2);

    // Once no setting applies it, the principle can go.
    f.save(json!({"repositoryId":alpha,"reset":["principles"]}))
        .unwrap();
    let settings = f.store.project_settings().unwrap();
    catalog.delete("no-network", 1, &roles, &settings).unwrap();
}
