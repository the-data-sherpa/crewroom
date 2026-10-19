//! Driven workflow composition. Durable control intent precedes asynchronous
//! work; process ownership, message receipts and quality evidence stay separate.
mod catalog;
pub mod configuration;
pub mod context;
mod continuation;
mod delivery;
pub mod evaluation;
pub mod preparation;
mod recovery;
mod start;
mod stop;
mod tools;
mod transitions;

use super::{
    HostManager, HostOptions,
    tasks::{RunTasks, Task},
};
use crate::{
    conflict,
    evidence::{Checks, Evidence, Research},
    github_delivery::GitHubDelivery,
    jira_delivery::JiraDelivery,
    mail::MailClient,
    messaging::{Messaging, StoreAccess},
    publication::Publication,
    repository::Repository,
};
use configuration::RuntimeConfig;
use crewroom_core::{Result, authority::Authority, roles::Roles, store::Store};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub struct WorkflowOptions {
    pub runtime: RuntimeConfig,
    pub helper: PathBuf,
    pub host: PathBuf,
    pub tmux: PathBuf,
    pub backend: String,
    pub roles: PathBuf,
    pub environment: Value,
}
#[derive(Clone)]
pub struct Services<S: StoreAccess> {
    pub repository: Repository<S>,
    pub evidence: Evidence<S>,
    pub checks: Option<Checks<S>>,
    pub research: Research<S>,
    pub publication: Option<Publication<S>>,
    pub jira: Option<JiraDelivery<S>>,
    /// GitHub issue delivery for GitHub-sourced tickets (FOR-48).
    pub github: Option<GitHubDelivery<S>>,
}
/// Which ticket delivery an assignment may use: Jira for Jira and other
/// non-GitHub tickets (unchanged), GitHub only for GitHub-sourced tickets.
pub(crate) fn delivery_flags(run: &Value, jira: bool, github: bool) -> (bool, bool) {
    let from_github = crewroom_core::ticket_source::provider(&run["taskSnapshot"])
        == Some(crewroom_core::ticket_source::GITHUB);
    (jira && !from_github, github && from_github)
}
#[derive(Default)]
struct State {
    closing: bool,
    admitting: HashMap<String, usize>,
    scheduled: HashSet<String>,
    dirty: HashSet<String>,
    errors: HashMap<String, String>,
    ports: HashMap<String, u16>,
}
struct Inner<S: StoreAccess> {
    store: S,
    authority: Arc<Authority>,
    config: RuntimeConfig,
    hosts: HostManager<S>,
    services: Services<S>,
    roles: Roles,
    environment: Value,
    tasks: RunTasks,
    cleanup: RunTasks,
    state: Mutex<State>,
    messages: Mutex<HashMap<String, Messaging<S>>>,
    timer: CancellationToken,
    timer_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    changed: Arc<dyn Fn(&str) + Send + Sync>,
}
#[derive(Clone)]
pub struct Workflow<S: StoreAccess>(Arc<Inner<S>>);
struct Reservation<S: StoreAccess> {
    workflow: Workflow<S>,
    run: String,
}
impl<S: StoreAccess> Drop for Reservation<S> {
    fn drop(&mut self) {
        let mut state = self.workflow.0.state.lock().unwrap();
        if let Some(count) = state.admitting.get_mut(&self.run) {
            *count -= 1;
            if *count == 0 {
                state.admitting.remove(&self.run);
            }
        }
        drop(state);
        (self.workflow.0.changed)(&self.run);
    }
}
impl<S: StoreAccess> Workflow<S> {
    /// Resolve only public model metadata; never project provider configuration.
    pub fn role_model(&self, role: &Value) -> Result<Value> {
        self.0.config.preview_model(role)
    }

    pub fn model_job_policy(&self) -> Value {
        let p = &self.0.config.policy;
        json!({"default":{"model":p["default"]["model"],"effort":p["default"]["effort"]},"jobs":p["jobs"],"budget":p["budget"]})
    }
    pub fn model_catalog(&self) -> Value {
        json!(self.0.config.catalog)
    }

    pub fn new(
        store: S,
        authority: Arc<Authority>,
        mut options: WorkflowOptions,
        services: Services<S>,
        changed: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<Self> {
        authority.assert_current()?;
        options.runtime.jobs_path = Some(options.roles.with_file_name("model-jobs.json"));
        let changed = Arc::new(changed);
        let catalog_changed = changed.clone();
        options.runtime.discovery = Some(crate::model_catalog::store::Catalog::open_with_login(
            &options.runtime.root.join("model-catalog"),
            options
                .runtime
                .discovery_sources(&options.helper, &options.environment),
            // FOR-73: the operator's OMP login in the service's HOME.
            crate::model_catalog::login::home(&options.environment),
            move || catalog_changed(""),
        )?);
        let callback = Arc::new(OnceLock::<Weak<Inner<S>>>::new());
        let host_callback = callback.clone();
        let hosts = HostManager::new(
            store.clone(),
            authority.clone(),
            HostOptions {
                root: options.runtime.root.clone(),
                socket: options.runtime.socket.clone(),
                helper: options.helper,
                host: options.host,
                tmux: options.tmux,
                backend: options.backend,
                command_engine: options
                    .runtime
                    .engine
                    .as_ref()
                    .map(|v| text(v, "socket").map(PathBuf::from))
                    .transpose()?,
            },
            move |run| {
                if let Some(inner) = host_callback.get().and_then(Weak::upgrade) {
                    Workflow(inner).notify(run);
                }
            },
        )?;
        let result = Self(Arc::new(Inner {
            store,
            authority,
            config: options.runtime,
            hosts,
            services,
            roles: Roles::new(options.roles),
            environment: options.environment,
            tasks: RunTasks::default(),
            cleanup: RunTasks::default(),
            state: Mutex::default(),
            messages: Mutex::default(),
            timer: CancellationToken::new(),
            timer_task: Mutex::default(),
            changed,
        }));
        let _ = callback.set(Arc::downgrade(&result.0));
        Ok(result)
    }
    fn store<T>(&self, operation: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        self.0.authority.assert_current()?;
        self.0.store.with_store(operation)
    }
    fn check_open(&self) -> Result<()> {
        if self.0.state.lock().unwrap().closing {
            Err(conflict("Workflow supervision is stopping."))
        } else {
            Ok(())
        }
    }
    fn current(&self, run: &str, revision: u64) -> Result<Value> {
        self.check_open()?;
        self.store(|s| {
            let current = s.get_run(run)?;
            if current["desiredState"] != "running"
                || current["controlRevision"].as_u64() != Some(revision)
                || present(&current, "sourceConflict")
            {
                return Err(conflict(
                    "A newer control or requirements decision superseded this operation.",
                ));
            }
            Ok(current)
        })
    }
    fn reserve(&self, run: &str) -> Result<Reservation<S>> {
        let mut state = self.0.state.lock().unwrap();
        if state.closing {
            return Err(conflict("Workflow supervision is stopping."));
        }
        if !state.admitting.contains_key(run) {
            self.store(|s| {
                let coding =
                    s.repository_work(run)?.is_some() || s.get_run(run)?["purpose"] == "evaluation";
                if coding
                    && (array(&s.executing_runs()?)
                        .iter()
                        .any(|r| r["runId"] != run && r["coding"] == true)
                        || state
                            .admitting
                            .keys()
                            .filter(|id| id.as_str() != run)
                            .any(|id| {
                                s.get_run(id).map_or(true, |v| v["purpose"] == "evaluation")
                                    || s.repository_work(id).map_or(true, |v| v.is_some())
                            }))
                {
                    return Err(conflict(
                        "Another coding ticket is executing. Wait until its agents stop.",
                    ));
                }
                Ok(())
            })?;
        }
        *state.admitting.entry(run.into()).or_default() += 1;
        Ok(Reservation {
            workflow: self.clone(),
            run: run.into(),
        })
    }
    pub fn busy(&self, run: &str) -> bool {
        self.0.state.lock().unwrap().admitting.contains_key(run)
            || self.0.tasks.busy(run)
            || self.0.cleanup.busy(run)
            || self.0.hosts.busy(run)
    }
    pub fn observes(&self, attempt: &str) -> bool {
        self.0.hosts.observes(attempt)
    }
    pub fn pane_exited(&self, pane: &str) -> Result<()> {
        self.0.hosts.pane_exited(pane)
    }
    pub async fn terminal(&self, run: &str, attempt: &str) -> Result<Value> {
        self.0.hosts.terminal(run, attempt).await
    }
    pub fn snapshot(&self, run: &str) -> Result<Value> {
        let mut value = self.store(|s| {
            let mut v = s.runtime(run)?;
            v["teamProposals"] = json!(s.team_proposals(run)?);
            Ok(v)
        })?;
        let ids: Vec<_> = active(&value)
            .iter()
            .filter_map(|a| a["currentAttemptId"].as_str())
            .map(String::from)
            .collect();
        value["observedAttemptIds"] = json!(
            ids.iter()
                .filter(|id| self.0.hosts.observes(id))
                .collect::<Vec<_>>()
        );
        value["degradedAttemptIds"] = json!(
            array(&self.0.hosts.observation(run))
                .iter()
                .filter(|v| v["observation"]["state"] == "degraded"
                    && ids.iter().any(|id| v["attemptId"] == *id))
                .map(|v| v["attemptId"].clone())
                .collect::<Vec<_>>()
        );
        if let Some(error) = self.0.state.lock().unwrap().errors.get(run) {
            value["error"] = json!(error);
        }
        Ok(value)
    }
    fn failure(&self, run: &str, error: impl std::fmt::Display) {
        self.0
            .state
            .lock()
            .unwrap()
            .errors
            .insert(run.into(), error.to_string());
        (self.0.changed)(run);
    }
    fn messages(&self, run: &str) -> Result<Messaging<S>> {
        let mut cache = self.0.messages.lock().unwrap();
        if let Some(client) = cache.get(run) {
            return Ok(client.clone());
        }
        let mail = &self.0.config.mail;
        let project = Path::new(text(mail, "projectRoot")?).join(run);
        private_directory(&project)?;
        let duration = Duration::try_from_secs_f64(mail["retentionSeconds"].as_f64().unwrap())
            .map_err(|_| conflict("Invalid Mail retention."))?;
        let client = Messaging::new(
            self.0.store.clone(),
            MailClient::new(text(mail, "url")?, text(mail, "token")?.into())
                .map_err(|_| conflict("External Mail configuration is unavailable."))?,
            project.to_string_lossy().into_owned(),
            duration,
        )?;
        cache.insert(run.into(), client.clone());
        Ok(client)
    }
    pub fn operator_transition<F, Fut>(&self, run: &str, operation: F) -> Result<Task>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        self.check_open()?;
        let this = self.clone();
        self.0.tasks.enqueue(run, None, move || async move {
            this.check_open().map_err(Arc::new)?;
            operation().await.map_err(Arc::new)
        })
    }
    pub fn notify(&self, run: &str) {
        let mut state = self.0.state.lock().unwrap();
        if state.closing {
            return;
        }
        if !state.scheduled.insert(run.into()) {
            state.dirty.insert(run.into());
            return;
        }
        drop(state);
        let this = self.clone();
        let id = run.to_owned();
        let queued = self.0.tasks.enqueue(run, None, move || async move {
            let result = this.reconcile(&id).await;
            if let Err(error) = &result {
                this.failure(&id, error);
            }
            let mut state = this.0.state.lock().unwrap();
            state.scheduled.remove(&id);
            let again = state.dirty.remove(&id);
            drop(state);
            (this.0.changed)(&id);
            if again {
                this.notify(&id);
            }
            result.map_err(Arc::new)
        });
        if let Err(error) = queued {
            self.0.state.lock().unwrap().scheduled.remove(run);
            self.failure(run, error);
        }
    }
    fn start_timer(&self) {
        let mut saved = self.0.timer_task.lock().unwrap();
        if saved.is_some() || self.0.timer.is_cancelled() {
            return;
        }
        let weak = Arc::downgrade(&self.0);
        let cancel = self.0.timer.clone();
        *saved = Some(tokio::spawn(async move {
            loop {
                tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(1500))=>{}}
                let Some(inner) = weak.upgrade() else { break };
                let workflow = Workflow(inner);
                let runs: Vec<_> = workflow
                    .0
                    .messages
                    .lock()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect();
                for run in runs {
                    // A contained evaluation check runs on the workflow lane.
                    // Fence and stop actors independently when its active budget
                    // expires, so that a slow check cannot defer the timer.
                    if workflow
                        .store(|s| s.get_run(&run))
                        .is_ok_and(|r| r["purpose"] == "evaluation" && r["status"] == "running")
                        && workflow
                            .store(|s| s.account_time(&run))
                            .is_ok_and(|r| r["pauseReason"] == "budget")
                    {
                        let _ = workflow.0.hosts.stop_run(&run);
                    }
                    if workflow.store(|s| s.get_run(&run)).is_ok_and(|r| {
                        matches!(
                            r["status"].as_str(),
                            Some("running" | "pausing" | "awaiting_runner")
                        )
                    }) {
                        workflow.notify(&run);
                    }
                }
            }
        }));
    }
}

fn active(runtime: &Value) -> Vec<&Value> {
    array(&runtime["assignments"])
        .iter()
        .filter(|a| !present(a, "retiredAt") && !present(a, "rebriefRequired"))
        .collect()
}
fn revision(run: &Value) -> Result<u64> {
    run["controlRevision"]
        .as_u64()
        .ok_or_else(|| conflict("Missing control revision."))
}
async fn wait(task: Result<Task>) -> Result<Value> {
    task?.wait().await.map_err(|e| conflict(&e.to_string()))
}
fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if path
        .symlink_metadata()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        if let Some(parent) = path.parent() {
            if parent.try_exists()? {
                crate::canonical(parent)?;
            } else {
                private_directory(parent)?;
            }
        }
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    crate::canonical(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_dir()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(conflict(
            "Workflow directory must be canonical, private and owned.",
        ));
    }
    Ok(())
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| conflict("Workflow input requires a text field."))
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn present(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|v| !v.is_null())
}
fn remove(value: &mut Value, keys: &[&str]) {
    if let Some(object) = value.as_object_mut() {
        for key in keys {
            object.remove(*key);
        }
    }
}

mod choreography;

#[cfg(test)]
mod delivery_flag_tests {
    use super::delivery_flags;
    use serde_json::json;

    #[test]
    fn github_tickets_use_github_delivery_and_others_keep_jira() {
        let github = json!({"taskSnapshot":{"source":"github"}});
        let jira = json!({"taskSnapshot":{"source":"jira"}});
        let local = json!({"taskSnapshot":{}});
        assert_eq!(delivery_flags(&github, true, true), (false, true));
        assert_eq!(delivery_flags(&github, true, false), (false, false));
        assert_eq!(delivery_flags(&jira, true, true), (true, false));
        assert_eq!(delivery_flags(&local, true, true), (true, false));
        assert_eq!(delivery_flags(&local, false, true), (false, false));
    }
}
