//! Short admission guards and driven cleanup for checked repository mutations.
//! A ticket retains controller authority even after its original waiter disappears.
use crate::{CheckedProcess, conflict};
use crewroom_core::{Result, authority::Authority};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone)]
pub(super) struct Tracked {
    pub descriptor: Option<CheckedProcess>,
    pub done: watch::Receiver<bool>,
}
struct Job {
    run: String,
    tracked: Tracked,
}
#[derive(Default)]
struct State {
    closed: bool,
    stopping: HashSet<String>,
    epochs: HashMap<String, u64>,
    jobs: HashMap<Uuid, Job>,
}
#[derive(Clone, Default)]
pub(super) struct Operations(Arc<Mutex<State>>);
struct Admission {
    operations: Operations,
    id: Uuid,
    run: String,
    epoch: u64,
    done: watch::Sender<bool>,
    authority: Arc<Authority>,
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.operations.0.lock().unwrap().jobs.remove(&self.id);
        self.done.send_replace(true);
    }
}
#[derive(Clone)]
pub(super) struct Ticket(Arc<Admission>);
impl Ticket {
    /// Registry precedes the workflow store lock. The caller must not await here.
    pub fn dispatch<T>(
        &self,
        descriptor: &CheckedProcess,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let mut state = self.0.operations.0.lock().unwrap();
        self.0.authority.assert_current()?;
        if state.closed
            || state.stopping.contains(&self.0.run)
            || state.epochs.get(&self.0.run).copied().unwrap_or(0) != self.0.epoch
        {
            return Err(conflict(
                "Repository operation was stopped before Git dispatch.",
            ));
        }
        state
            .jobs
            .get_mut(&self.0.id)
            .ok_or_else(|| conflict("Repository operation is no longer registered."))?
            .tracked
            .descriptor = Some(descriptor.clone());
        operation()
    }
}
impl Operations {
    pub fn begin(&self, run: &str, authority: Arc<Authority>) -> Result<Ticket> {
        authority.assert_current()?;
        let mut state = self.0.lock().unwrap();
        if state.closed || state.stopping.contains(run) {
            return Err(conflict("Repository mutations are stopping."));
        }
        let id = Uuid::new_v4();
        let (done, rx) = watch::channel(false);
        let ticket = Ticket(Arc::new(Admission {
            operations: self.clone(),
            id,
            run: run.into(),
            epoch: state.epochs.get(run).copied().unwrap_or(0),
            done,
            authority,
        }));
        state.jobs.insert(
            id,
            Job {
                run: run.into(),
                tracked: Tracked {
                    descriptor: None,
                    done: rx,
                },
            },
        );
        Ok(ticket)
    }
    pub fn fence(&self, run: Option<&str>) -> (Option<u64>, Vec<Tracked>) {
        let mut state = self.0.lock().unwrap();
        if let Some(run) = run {
            state.stopping.insert(run.into());
            *state.epochs.entry(run.into()).or_default() += 1;
        } else {
            state.closed = true;
        }
        let epoch = run.map(|id| state.epochs[id]);
        let pending = state
            .jobs
            .values()
            .filter(|job| run.is_none_or(|id| job.run == id))
            .map(|job| job.tracked.clone())
            .collect();
        (epoch, pending)
    }
    pub fn stopped(&self, run: Option<&str>, epoch: Option<u64>) {
        if let Some(run) = run {
            let mut state = self.0.lock().unwrap();
            if state.epochs.get(run).copied() == epoch {
                state.stopping.remove(run);
            }
        }
    }
    pub fn busy(&self, run: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .jobs
            .values()
            .any(|job| job.run == run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Arc<Authority>, Operations) {
        let root = tempfile::tempdir().unwrap();
        let authority = Arc::new(Authority::acquire_for_execution(root.path()).unwrap());
        (root, authority, Operations::default())
    }
    fn descriptor(authority: &Authority) -> CheckedProcess {
        CheckedProcess {
            version: 1,
            root: authority.root().join("unused"),
            token: Uuid::new_v4().to_string(),
            spec_sha256: "0".repeat(64),
            epoch: authority.epoch().into(),
            controller_root: authority.root().into(),
        }
    }
    #[test]
    fn overlapping_stops_cannot_reopen_newer_fence_or_old_ticket() {
        let (_root, authority, operations) = setup();
        let old = operations.begin("run", authority.clone()).unwrap();
        let (first, _) = operations.fence(Some("run"));
        let (second, _) = operations.fence(Some("run"));
        operations.stopped(Some("run"), first);
        assert!(operations.begin("run", authority.clone()).is_err());
        operations.stopped(Some("run"), second);
        let mut called = false;
        assert!(
            old.dispatch(&descriptor(&authority), || {
                called = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!called);
        let fresh = operations.begin("run", authority.clone()).unwrap();
        assert!(fresh.dispatch(&descriptor(&authority), || Ok(())).is_ok());
        operations.fence(None);
        assert!(
            fresh
                .dispatch::<()>(&descriptor(&authority), || panic!("closed dispatch"))
                .is_err()
        );
        assert!(operations.begin("other", authority).is_err());
    }
    #[tokio::test]
    async fn driven_ticket_retains_actual_controller_lease_after_caller_drops() {
        let (root, authority, operations) = setup();
        let ticket = operations.begin("run", authority.clone()).unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let _ticket = ticket;
            let _ = wait.await;
        });
        let (_, pending) = operations.fence(Some("run"));
        drop(authority);
        assert!(Authority::acquire_for_execution(root.path()).is_err());
        assert!(operations.busy("run"));
        assert!(!*pending[0].done.borrow());
        release.send(()).unwrap();
        worker.await.unwrap();
        assert!(*pending[0].done.borrow());
        assert!(!operations.busy("run"));
        assert!(Authority::acquire_for_execution(root.path()).is_ok());
    }
    #[tokio::test]
    async fn stop_during_actual_configuration_read_fences_dispatch_without_orphan_roots() {
        use super::super::{Git, Records};
        let (root, authority, _) = setup();
        let path = std::fs::canonicalize(root.path()).unwrap();
        let shim = path.join("config-probe");
        crewroom_test_support::install_executable(
            &shim,
            "#!/bin/sh\n: > blocked\nwhile [ ! -f release ]; do sleep 0.01; done\nexit 1\n",
        );
        let mut git = Git::new(&path, std::env::current_exe().unwrap(), authority).unwrap();
        git.executable = shim;
        let run = Uuid::new_v4().to_string();
        let (worker_git, worker_run, cwd) = (git.clone(), run.clone(), path.clone());
        let pending = tokio::spawn(async move {
            worker_git
                .mutate(&worker_run, &cwd, &["branch", "blocked"], |_| {
                    panic!("Stopped configuration preflight reached dispatch")
                })
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !path.join("blocked").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(git.busy(&run));
        let stop = git.stop(Some(&run), Records::default).unwrap();
        std::fs::write(path.join("release"), "").unwrap();
        assert!(
            pending
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("stopped before Git dispatch")
        );
        let result = stop.wait().await.unwrap();
        assert_eq!(result["stopped"], true, "{result}");
        assert!(result["operations"].as_array().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(git.jobs.join(run)).unwrap().count(), 0);
    }
}
