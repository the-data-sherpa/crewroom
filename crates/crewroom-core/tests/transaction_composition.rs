use crewroom_core::{Error, Result, store::Store};
use serde_json::json;
#[test]
fn composed_public_transactions_roll_back_together_and_reopen_without_partial_receipts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let result:Result<()>=store.transaction(|s|{let task=s.create_task(&json!({"title":"Atomic evidence","description":""}))?;s.refine(task["id"].as_str().unwrap(),&json!({"revision":1,"sourceRevision":1,"brief":"No partial receipts","criteria":["Atomic"]}))?;Err(Error::Conflict("fixture final receipt failure".into()))});
    assert!(result.is_err());
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.snapshot().unwrap()["tasks"], json!([]));
    let result = store
        .transaction(|s| {
            let first = s.create_task(&json!({"title":"Retained","description":""}))?;
            let failed: Result<()> = s.transaction(|s| {
                s.create_task(&json!({"title":"Rolled back","description":""}))?;
                Err(Error::Conflict("fixture inner failure".into()))
            });
            assert!(failed.is_err());
            s.create_task(&json!({"title":"Also retained","description":""}))?;
            Ok(first)
        })
        .unwrap();
    assert_eq!(
        store.get_task(result["id"].as_str().unwrap()).unwrap()["title"],
        "Retained"
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    let tasks = store.snapshot().unwrap()["tasks"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(tasks.len(), 2);
    assert!(!tasks.iter().any(|t| t["title"] == "Rolled back"));
}
