use crewroom_core::{model_jobs, roles::Roles, store::Store};
use crewroom_runtime::model_policy::resolve_assignment;
use serde_json::{Value, json};
fn catalog() -> Vec<Value> {
    ["code","judge","review","fast","override"].iter().map(|id| json!({"provider":"fixture","id":id,"api":"test","efforts":["off","minimal","low","medium","high","xhigh","max"],"tools":true,"inputs":["text"]})).collect()
}
fn policy() -> Value {
    json!({"jobs":{"code":"fixture/code","judgment":"fixture/judge","review":"fixture/review","fast":"fixture/fast"},"budget":"large"})
}
#[test]
fn jobs_budget_overrides_fallbacks_and_availability_are_explicit() {
    let catalog = catalog();
    let mut role = json!({"enabled":true,"job":"code"});
    for (budget, expected) in [
        ("unlimited", ["xhigh", "max", "max", "medium"]),
        ("large", ["high", "xhigh", "xhigh", "low"]),
        ("medium", ["medium", "high", "high", "minimal"]),
        ("small", ["low", "medium", "medium", "minimal"]),
    ] {
        let mut policy = policy();
        policy["budget"] = json!(budget);
        for (job, effort) in model_jobs::JOBS.iter().zip(expected) {
            role["job"] = json!(job);
            let result = resolve_assignment(&role, &catalog, &policy, &json!({})).unwrap();
            assert_eq!(result["resolved"]["selector"], policy["jobs"][job]);
            assert_eq!(result["resolved"]["effort"], effort);
        }
    }
    role["model"] = json!("fixture/override");
    role["effort"] = json!("off");
    let resolved = resolve_assignment(&role, &catalog, &policy(), &json!({})).unwrap();
    assert_eq!(resolved["resolved"]["selector"], "fixture/override");
    assert_eq!(resolved["resolved"]["effort"], "off");
    role["model"] = json!("");
    role["effort"] = json!("default");
    for job in [Value::Null, json!("unknown")] {
        role["job"] = job;
        let r = resolve_assignment(&role, &catalog, &policy(), &json!({})).unwrap();
        assert_eq!(r["resolved"]["selector"], "fixture/judge");
        assert!(r["warning"].as_str().unwrap().contains("Judgment"));
    }
    role["model"] = json!("missing/model");
    assert!(resolve_assignment(&role, &catalog, &policy(), &json!({})).is_err());
    role["model"] = json!("fixture/override");
    role["effort"] = json!("impossible");
    assert!(resolve_assignment(&role, &catalog, &policy(), &json!({})).is_err());
}
#[test]
fn saved_job_changes_and_legacy_role_migration_preserve_admitted_selections() {
    let root = tempfile::tempdir().unwrap();
    let roles = Roles::new(root.path().join("roles.json"));
    let mut legacy = roles.read().unwrap();
    for r in &mut legacy {
        r.as_object_mut().unwrap().remove("job");
    }
    legacy[0]["model"] = json!("fixture/override");
    legacy[0]["effort"] = json!("off");
    std::fs::write(roles.path(), serde_json::to_vec(&legacy).unwrap()).unwrap();
    let migrated = roles.read().unwrap();
    assert_eq!(migrated[0]["model"], "fixture/override");
    assert_eq!(migrated[0]["effort"], "off");
    let role = migrated.iter().find(|r| r["id"] == "implementer").unwrap();
    assert_eq!(role["job"], "code");
    let path = root.path().join("model-jobs.json");
    let mut input = model_jobs::defaults(&policy());
    let first = model_jobs::save(&path, &policy(), &input).unwrap();
    let model = resolve_assignment(role, &catalog(), &first, &json!({})).unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Frozen model","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let admitted=store.admit_assignment(run["id"].as_str().unwrap(),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":model,"input":"Inspect"})).unwrap();
    input = first.clone();
    input["budget"] = json!("small");
    input["jobs"]["code"] = json!("fixture/override");
    let second = model_jobs::save(&path, &policy(), &input).unwrap();
    assert!(model_jobs::save(&path, &policy(), &input).is_err());
    let next = resolve_assignment(role, &catalog(), &second, &json!({})).unwrap();
    assert_eq!(next["resolved"]["effort"], "low");
    assert_eq!(next["resolved"]["selector"], "fixture/override");
    drop(store);
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    assert_eq!(
        store.runtime(run["id"].as_str().unwrap()).unwrap()["assignments"][0],
        admitted["assignment"]
    );
    assert_eq!(
        admitted["assignment"]["model"]["resolved"]["effort"],
        "high"
    );
}

#[test]
fn an_unavailable_configured_coordinator_is_not_silently_replaced_for_refinement() {
    use crewroom_runtime::supervisor::workflow::configuration::RuntimeConfig;
    let config=RuntimeConfig::parse(&json!({"root":"/tmp/job-fixture","socket":"/tmp/job-fixture/tmux.sock","ompBinary":"/fixture/omp","mail":{"atcDisabled":true,"retentionSeconds":600,"projectRoot":"/tmp/job-mail","url":"http://127.0.0.1:4123/mcp/","token":"synthetic"},"catalog":catalog(),"policy":policy(),"models":{"providers":{}}})).unwrap();
    let coordinator = json!({"id":"coordinator","enabled":true,"job":"judgment","model":"missing/model","effort":"default"});
    let other = json!({"id":"other","enabled":true,"job":"code"});
    let team = config
        .refinement_team(&json!({"purpose":"explore"}), &[coordinator.clone(), other])
        .unwrap();
    assert_eq!(team["team"][0]["roleId"], "coordinator");
    assert!(config.assignment_model(&coordinator).is_err());
}
