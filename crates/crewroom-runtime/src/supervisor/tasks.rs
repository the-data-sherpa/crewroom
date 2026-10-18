//! Driven FIFO work, independent per run. A cancelled request never cancels an
//! admitted operation. Shared replies retain the original error classification.
use crate::conflict;
use crewroom_core::{Error, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use uuid::Uuid;

pub type Reply = std::result::Result<Value, Arc<Error>>;
#[derive(Clone)]
pub struct Task(watch::Receiver<Option<Reply>>);
impl Task {
    pub async fn wait(mut self) -> Reply {
        loop {
            if let Some(reply) = self.0.borrow_and_update().clone() {
                return reply;
            }
            self.0.changed().await.map_err(|_| {
                Arc::new(conflict(
                    "Supervised operation was interrupted; reconcile saved ownership.",
                ))
            })?;
        }
    }
}
struct Job {
    run: String,
    key: Option<String>,
    task: Task,
}
#[derive(Default)]
struct State {
    closed: bool,
    jobs: HashMap<Uuid, Job>,
    tails: HashMap<String, (Uuid, Task)>,
}
#[derive(Clone, Default)]
pub struct RunTasks(Arc<Mutex<State>>);
impl RunTasks {
    /// Use the same short admission lock around the final synchronous launch.
    /// Lock order is admission, then store. Never await in this callback.
    pub fn with_open<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let state = self.0.lock().unwrap();
        if state.closed {
            return Err(conflict("Supervision is stopping."));
        }
        operation()
    }
    pub fn busy(&self, run: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .jobs
            .values()
            .any(|job| job.run == run)
    }
    pub fn keyed(&self, run: &str, key: &str) -> Option<Task> {
        self.0
            .lock()
            .unwrap()
            .jobs
            .values()
            .find(|j| j.run == run && j.key.as_deref() == Some(key))
            .map(|j| j.task.clone())
    }
    pub fn runs(&self) -> Vec<String> {
        let mut runs: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .jobs
            .values()
            .map(|job| job.run.clone())
            .collect();
        runs.sort();
        runs.dedup();
        runs
    }
    /// Registration and keyed coalescing happen before this function returns.
    pub fn enqueue<F, Fut>(&self, run: &str, key: Option<&str>, operation: F) -> Result<Task>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        let mut state = self.0.lock().unwrap();
        if state.closed {
            return Err(conflict("Supervision is stopping."));
        }
        if let Some(key) = key
            && let Some(job) = state
                .jobs
                .values()
                .find(|job| job.run == run && job.key.as_deref() == Some(key))
        {
            return Ok(job.task.clone());
        }
        let id = Uuid::new_v4();
        let (tx, rx) = watch::channel(None);
        let task = Task(rx);
        let previous = state.tails.insert(run.into(), (id, task.clone()));
        state.jobs.insert(
            id,
            Job {
                run: run.into(),
                key: key.map(String::from),
                task: task.clone(),
            },
        );
        let tasks = self.clone();
        let run = run.to_owned();
        tokio::spawn(async move {
            struct Finish {
                tasks: RunTasks,
                run: String,
                id: Uuid,
                tx: watch::Sender<Option<Reply>>,
            }
            impl Drop for Finish {
                fn drop(&mut self) {
                    let mut state = self.tasks.0.lock().unwrap();
                    if self.tx.borrow().is_none() {
                        self.tx.send_replace(Some(Err(Arc::new(conflict(
                            "Supervised worker stopped unexpectedly; reconcile its saved work.",
                        )))));
                    }
                    state.jobs.remove(&self.id);
                    if state
                        .tails
                        .get(&self.run)
                        .is_some_and(|(id, _)| *id == self.id)
                    {
                        state.tails.remove(&self.run);
                    }
                }
            }
            let finish = Finish { tasks, run, id, tx };
            if let Some((_, previous)) = previous {
                let _ = previous.wait().await;
            }
            finish.tx.send_replace(Some(operation().await));
        });
        Ok(task)
    }
    /// Fence first, publish stop intent for the affected runs, then drain.
    /// Cleanup itself does not need admission through this queue.
    pub fn fence(&self) {
        self.0.lock().unwrap().closed = true;
    }
    pub fn closed(&self) -> bool {
        self.0.lock().unwrap().closed
    }
    pub async fn drain(&self) {
        let tasks: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .jobs
            .values()
            .map(|job| job.task.clone())
            .collect();
        for task in tasks {
            let _ = task.wait().await;
        }
    }
    pub async fn drain_run(&self, run: &str) {
        let tasks: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .jobs
            .values()
            .filter(|job| job.run == run)
            .map(|job| job.task.clone())
            .collect();
        for task in tasks {
            let _ = task.wait().await;
        }
    }
}
