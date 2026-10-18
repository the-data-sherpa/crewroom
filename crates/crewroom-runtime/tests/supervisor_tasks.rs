use crewroom_core::Error;
use crewroom_runtime::supervisor::{journal::visit_events, tasks::RunTasks};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[tokio::test]
async fn per_run_fifo_shares_keyed_replies_without_blocking_other_runs() {
    let queue = RunTasks::default();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (release, gate) = oneshot::channel();
    let first = queue
        .enqueue("one", Some("start"), move || async move {
            gate.await.unwrap();
            Ok(json!("first"))
        })
        .unwrap();
    let duplicate = queue
        .enqueue("one", Some("start"), || async {
            panic!("duplicate executed")
        })
        .unwrap();
    let seen = order.clone();
    let second = queue
        .enqueue("one", None, move || async move {
            seen.lock().unwrap().push(2);
            Ok(json!(2))
        })
        .unwrap();
    let other = queue
        .enqueue("two", None, || async { Ok(json!("other")) })
        .unwrap();
    assert_eq!(other.wait().await.unwrap(), "other");
    assert!(queue.busy("one"));
    assert!(order.lock().unwrap().is_empty());
    release.send(()).unwrap();
    assert_eq!(first.wait().await.unwrap(), duplicate.wait().await.unwrap());
    assert_eq!(second.wait().await.unwrap(), 2);
    queue.drain().await;
    assert_eq!(*order.lock().unwrap(), vec![2]);
    assert!(!queue.busy("one"));
}
#[tokio::test]
async fn dropped_waiters_do_not_abandon_admitted_work_and_errors_keep_their_type() {
    let queue = RunTasks::default();
    let (release, gate) = oneshot::channel();
    let (finished, receipt) = oneshot::channel();
    let task = queue
        .enqueue("one", None, move || async move {
            gate.await.unwrap();
            finished.send(()).unwrap();
            Err(Arc::new(Error::Invalid("preserved".into())))
        })
        .unwrap();
    let reply = task.clone();
    let waiter = tokio::spawn(task.wait());
    waiter.abort();
    release.send(()).unwrap();
    receipt.await.unwrap();
    assert!(matches!(reply.wait().await.unwrap_err().as_ref(),Error::Invalid(s) if s=="preserved"));
    queue.drain().await;
}
#[tokio::test]
async fn fence_prevents_late_dispatch_and_drains_queued_and_panicking_workers() {
    let queue = RunTasks::default();
    let (release, gate) = oneshot::channel();
    let dispatch = queue.clone();
    let queued = queue
        .enqueue("one", None, move || async move {
            gate.await.unwrap();
            dispatch.with_open(|| Ok(json!("launch"))).map_err(Arc::new)
        })
        .unwrap();
    let panicked = queue
        .enqueue("one", None, || async {
            panic!("deliberate worker interruption")
        })
        .unwrap();
    let after = queue
        .enqueue("one", None, || async { Ok(json!("cleanup")) })
        .unwrap();
    queue.fence();
    assert!(
        queue
            .enqueue("two", None, || async { Ok(json!(0)) })
            .is_err()
    );
    release.send(()).unwrap();
    assert!(queued.wait().await.is_err());
    assert!(panicked.wait().await.is_err());
    assert_eq!(after.wait().await.unwrap(), "cleanup");
    queue.drain().await;
    assert!(queue.runs().is_empty());
}
#[test]
fn passive_event_recovery_ignores_only_an_unterminated_tail() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("events.jsonl");
    let mut seen = Vec::new();
    assert_eq!(
        visit_events(root, |_, _| panic!("absent journal")).unwrap(),
        0
    );
    std::fs::write(
        &path,
        b"{\"sequence\":1,\"event\":{\"type\":\"agent_start\"}}\n{partial",
    )
    .unwrap();
    assert_eq!(
        visit_events(root, |n, e| {
            seen.push((n, e.clone()));
            Ok(())
        })
        .unwrap(),
        1
    );
    assert_eq!(seen[0].1["type"], "agent_start");
    std::fs::write(&path, b"{\"sequence\":1,\"event\":{}}\n{broken}\n").unwrap();
    assert!(visit_events(root, |_, _| Ok(())).is_err());
    std::fs::write(&path, b"{\"sequence\":2,\"event\":{}}\n").unwrap();
    assert!(visit_events(root, |_, _| Ok(())).is_err());
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(root.join("missing"), &path).unwrap();
    assert!(visit_events(root, |_, _| Ok(())).is_err());
}
