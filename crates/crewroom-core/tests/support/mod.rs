//! Explicit operator-accepted plan for tests exercising subsequent coding behavior.
use crewroom_core::store::Store;
use serde_json::json;
pub fn accept_plan(store: &Store, task_id: &str) {
    let task = store.get_task(task_id).unwrap();
    let source = store
        .save_plan_source(
            &store
                .prepare_plan_source(task_id, task["revision"].as_u64().unwrap())
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    let plan=store.save_implementation_plan(task_id,&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Exercise the accepted fixture behavior","scope":[task["brief"]],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Revision-bound evidence"],"criteria":(0..task["criteria"].as_array().unwrap().len()).collect::<Vec<_>>(),"validation":["The surrounding behavior test"]}]}})).unwrap();
    store.decide_implementation_plan(plan["id"].as_str().unwrap(),&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task["revision"],"sourceId":source["id"],"decision":"accept"})).unwrap();
}
