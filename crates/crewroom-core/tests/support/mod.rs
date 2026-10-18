//! Explicit operator-accepted plan for tests exercising subsequent coding behavior.
use crewroom_core::store::Store;
use serde_json::json;
#[allow(dead_code)]
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

/// A configured project on a real Git checkout under `root`, with no ticket
/// source confirmed yet. Returns its id.
#[allow(dead_code)]
pub fn project(store: &Store, root: &std::path::Path, name: &str) -> String {
    let checkout = std::fs::canonicalize(root)
        .unwrap()
        .join(name.replace('/', "-"));
    std::fs::create_dir_all(&checkout).unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    store
        .create_repository(&json!({"name":name,"path":checkout,"targetBranch":"main","checks":[]}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Confirms GitHub Issues `repository` as a project's ticket source (FOR-47)
/// against a synthetic github.com remote observation; nothing is contacted.
#[allow(dead_code)]
pub fn confirm_github(store: &Store, project: &str, repository: &str) {
    let observed =
        crewroom_core::project_source::observe(Some(&format!("git@github.com:{repository}.git")));
    store
        .confirm_ticket_source(
            project,
            &json!({"tickets":"github","repository":repository}),
            &observed,
        )
        .unwrap();
}

/// A project whose confirmed ticket source is GitHub Issues `repository`.
#[allow(dead_code)]
pub fn github_project(store: &Store, root: &std::path::Path, repository: &str) -> String {
    let id = project(store, root, repository);
    confirm_github(store, &id, repository);
    id
}

/// Confirms Jira `site` as a project's ticket source (FOR-47) against a
/// synthetic bitbucket.org remote observation; nothing is contacted.
#[allow(dead_code)]
pub fn confirm_jira(store: &Store, project: &str, site: &str) {
    let observed =
        crewroom_core::project_source::observe(Some("git@bitbucket.org:fixture/repo.git"));
    store
        .confirm_ticket_source(project, &json!({"tickets":"jira","site":site}), &observed)
        .unwrap();
}

/// A project whose confirmed ticket source is the Jira site `site`.
#[allow(dead_code)]
pub fn jira_project(store: &Store, root: &std::path::Path, name: &str, site: &str) -> String {
    let id = project(store, root, name);
    confirm_jira(store, &id, site);
    id
}
