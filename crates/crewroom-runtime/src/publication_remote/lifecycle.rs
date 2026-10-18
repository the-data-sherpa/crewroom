//! Registered work outlives its waiter. Fencing and launch authorization share
//! one short lock; no registry lock is held while stopping or awaiting a child.
use crate::{conflict, publication_git::PublicationGit};
use crewroom_core::Result;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};
use uuid::Uuid;

struct Job {
    run: String,
    done: watch::Receiver<bool>,
    git: Option<PublicationGit>,
}
#[derive(Default)]
struct State {
    closed: bool,
    epochs: HashMap<String, u64>,
    jobs: HashMap<Uuid, Job>,
    queues: HashMap<String, (Uuid, watch::Receiver<bool>)>,
    lanes: HashMap<String, Weak<AsyncMutex<()>>>,
}
#[derive(Clone, Default)]
pub(super) struct Operations(Arc<Mutex<State>>);
#[derive(Clone)]
pub(crate) struct Ticket {
    operations: Operations,
    id: Uuid,
    run: String,
    epoch: u64,
}
impl Ticket {
    pub fn with_current<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let state = self.operations.0.lock().unwrap();
        if state.closed || state.epochs.get(&self.run).copied().unwrap_or(0) != self.epoch {
            return Err(conflict(
                "Publication operation was stopped. Retry under current control state.",
            ));
        }
        operation()
    }
    pub fn check(&self) -> Result<()> {
        self.with_current(|| Ok(()))
    }
    pub(super) fn attach(
        &self,
        create: impl FnOnce() -> Result<PublicationGit>,
    ) -> Result<PublicationGit> {
        let mut state = self.operations.0.lock().unwrap();
        if state.closed || state.epochs.get(&self.run).copied().unwrap_or(0) != self.epoch {
            return Err(conflict("Publication transport is stopping."));
        }
        let job = state
            .jobs
            .get_mut(&self.id)
            .ok_or_else(|| conflict("Publication operation is no longer registered."))?;
        if job.git.is_some() {
            return Err(conflict("Publication operation already owns a transport."));
        }
        let git = create()?;
        job.git = Some(git.clone());
        Ok(git)
    }
    pub(super) fn detached(&self) {
        if let Some(job) = self.operations.0.lock().unwrap().jobs.get_mut(&self.id) {
            job.git = None;
        }
    }
}
impl Operations {
    pub fn lane(&self, key: &str) -> Arc<AsyncMutex<()>> {
        let mut state = self.0.lock().unwrap();
        state.lanes.retain(|_, v| v.strong_count() > 0);
        if let Some(lane) = state.lanes.get(key).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(AsyncMutex::new(()));
        state.lanes.insert(key.into(), Arc::downgrade(&lane));
        lane
    }
    pub async fn run<T, F, Fut>(&self, run: &str, order: Option<&str>, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Ticket) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.start(run, order, operation)?.await.map_err(|_| {
            conflict("Publication worker was interrupted. Reconcile retained ownership.")
        })?
    }
    /// Register synchronously so a batch reserves every run's epoch before it
    /// waits on the first run. Dropping the receiver never cancels the worker.
    pub fn start<T, F, Fut>(
        &self,
        run: &str,
        order: Option<&str>,
        operation: F,
    ) -> Result<oneshot::Receiver<Result<T>>>
    where
        T: Send + 'static,
        F: FnOnce(Ticket) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.0.lock().unwrap();
            if state.closed {
                return Err(conflict("Publication transport is stopping."));
            }
            let id = Uuid::new_v4();
            let (done, receiver) = watch::channel(false);
            let order = order.map(String::from);
            let previous = order
                .as_ref()
                .and_then(|key| state.queues.insert(key.clone(), (id, receiver.clone())))
                .map(|(_, rx)| rx);
            let ticket = Ticket {
                operations: self.clone(),
                id,
                run: run.into(),
                epoch: state.epochs.get(run).copied().unwrap_or(0),
            };
            state.jobs.insert(
                id,
                Job {
                    run: run.into(),
                    done: receiver,
                    git: None,
                },
            );
            tokio::spawn(async move {
                struct Finish(Ticket, watch::Sender<bool>, Option<String>);
                impl Drop for Finish {
                    fn drop(&mut self) {
                        let mut state = self.0.operations.0.lock().unwrap();
                        // Unverified transport ownership remains visible to close.
                        if state.jobs.get(&self.0.id).is_some_and(|j| j.git.is_none()) {
                            state.jobs.remove(&self.0.id);
                        }
                        if let Some(key) = &self.2
                            && state
                                .queues
                                .get(key)
                                .is_some_and(|(id, _)| *id == self.0.id)
                        {
                            state.queues.remove(key);
                        }
                        self.1.send_replace(true);
                    }
                }
                let _finish = Finish(ticket.clone(), done, order);
                let result = async {
                    if let Some(mut previous) = previous {
                        while !*previous.borrow_and_update() {
                            previous.changed().await.map_err(|_| {
                                conflict("Previous publication operation was interrupted.")
                            })?;
                        }
                    }
                    ticket.check()?;
                    operation(ticket).await
                }
                .await;
                let _ = tx.send(result);
            });
        }
        Ok(rx)
    }
    /// Fencing is synchronous with registration. The drain itself is driven so
    /// an interrupted stop/close caller cannot abandon verified child cleanup.
    pub async fn stop(&self, run: Option<&str>) -> Result<()> {
        let jobs = {
            let mut state = self.0.lock().unwrap();
            if let Some(run) = run {
                *state.epochs.entry(run.into()).or_default() += 1;
            } else {
                state.closed = true;
            }
            state
                .jobs
                .iter()
                .filter(|(_, j)| run.is_none_or(|r| j.run == r))
                .map(|(id, j)| (*id, j.git.clone(), j.done.clone()))
                .collect::<Vec<_>>()
        };
        let this = self.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut failure = None;
            for (_, git, _) in &jobs {
                if let Some(git) = git
                    && let Err(e) = git.close().await
                {
                    failure = Some(e);
                }
            }
            for (id, _, mut done) in jobs {
                while !*done.borrow_and_update() {
                    if done.changed().await.is_err() {
                        failure = Some(conflict("Publication worker cleanup is unverified."));
                        break;
                    }
                }
                let git = this
                    .0
                    .lock()
                    .unwrap()
                    .jobs
                    .get(&id)
                    .and_then(|j| j.git.clone());
                if let Some(git) = git {
                    match git.close().await {
                        Ok(()) => {
                            this.0.lock().unwrap().jobs.remove(&id);
                        }
                        Err(e) => failure = Some(e),
                    }
                }
            }
            let _ = tx.send(failure.map_or(Ok(()), Err));
        });
        rx.await
            .map_err(|_| conflict("Publication shutdown was interrupted."))?
    }
}
