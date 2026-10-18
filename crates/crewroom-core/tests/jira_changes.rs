//! Jira re-acceptance retains the source text the operator last accepted (FOR-30, M13).
use crewroom_core::store::Store;
use serde_json::{Value, json};

fn imported(title: &str, description: &str) -> Value {
    json!({"siteUrl":"https://fixture.atlassian.net/","issues":[{"id":"100","key":"SIM-1","fields":{"summary":title,"description":description,"status":{"name":"Backlog"}}}]})
}

#[test]
fn jira_change_retains_last_accepted_title_and_description_until_reaccepted() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let task = store
        .import_jira(&imported("Fix sum", "Add numbers"))
        .unwrap()[0]
        .clone();
    let id = task["id"].as_str().unwrap();
    // A change before any brief is accepted has nothing to compare against.
    store
        .import_jira(&imported("Fix sums", "Add numbers"))
        .unwrap();
    assert!(store.get_task(id).unwrap().get("acceptedSource").is_none());
    store.refine(id,&json!({"revision":1,"sourceRevision":2,"brief":"Handle zero","criteria":["0 + 0 = 0"]})).unwrap();

    store
        .import_jira(&imported("Fix addition", "Add numbers"))
        .unwrap();
    let changed = store.get_task(id).unwrap();
    assert_eq!(changed["needsReacceptance"], true);
    assert_eq!(
        changed["acceptedSource"],
        json!({"title":"Fix sums","description":"Add numbers"})
    );
    // A further change before re-acceptance keeps the accepted text, not the intermediate one.
    store
        .import_jira(&imported("Fix addition", "Add integers"))
        .unwrap();
    assert_eq!(
        store.get_task(id).unwrap()["acceptedSource"],
        json!({"title":"Fix sums","description":"Add numbers"})
    );

    store.refine(id,&json!({"revision":2,"sourceRevision":4,"brief":"Handle zero","criteria":["0 + 0 = 0"]})).unwrap();
    let accepted = store.get_task(id).unwrap();
    assert_eq!(accepted["needsReacceptance"], false);
    assert!(accepted.get("acceptedSource").is_none());
}
