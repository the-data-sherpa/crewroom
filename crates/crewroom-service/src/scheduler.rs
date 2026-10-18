//! Signals wake a durable single coding slot. Launch workers outlive HTTP callers.
mod comparisons;
mod intake;
use crate::{Access, array, present, text};
use crewroom_core::{Error, Result, serialized::stringify};
use crewroom_runtime::{
    evidence::Evidence,
    messaging::StoreAccess,
    repository::Repository,
    supervisor::{
        tasks::{RunTasks, Task},
        workflow::Workflow,
    },
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    pending: bool,
    running: bool,
    closed: bool,
    error: Option<String>,
}
struct Inner {
    store: Access,
    work: Repository<Access>,
    evidence: Evidence<Access>,
    workflow: Workflow<Access>,
    state: Mutex<State>,
    workers: RunTasks,
    launches: RunTasks,
    changed: Arc<dyn Fn() + Send + Sync>,
}
#[derive(Clone)]
pub(crate) struct Scheduler(Arc<Inner>);
impl Scheduler {
    pub fn new(
        store: Access,
        work: Repository<Access>,
        evidence: Evidence<Access>,
        workflow: Workflow<Access>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Inner {
            store,
            work,
            evidence,
            workflow,
            state: Mutex::default(),
            workers: RunTasks::default(),
            launches: RunTasks::default(),
            changed: Arc::new(changed),
        }))
    }
    pub fn kick(&self) {
        let mut state = self.0.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.pending = true;
        if state.running {
            return;
        }
        state.running = true;
        let this = self.clone();
        if self
            .0
            .workers
            .enqueue("scheduler", None, move || async move {
                loop {
                    {
                        let mut state = this.0.state.lock().unwrap();
                        if state.closed || !state.pending {
                            state.running = false;
                            break;
                        }
                        state.pending = false;
                    }
                    let result = this.reconcile().await;
                    this.0.state.lock().unwrap().error = result.err().map(|e| e.to_string());
                    (this.0.changed)();
                }
                Ok(Value::Null)
            })
            .is_err()
        {
            state.running = false;
        }
    }
    pub fn error(&self) -> Option<String> {
        self.0.state.lock().unwrap().error.clone()
    }
    pub fn fence(&self) {
        self.0.state.lock().unwrap().closed = true;
        self.0.workers.fence();
        self.0.launches.fence();
    }
    pub async fn drain(&self) {
        self.0.workers.drain().await;
        self.0.launches.drain().await;
    }
    fn store<T>(&self, f: impl FnOnce(&crewroom_core::store::Store) -> Result<T>) -> Result<T> {
        self.0.store.with_store(f)
    }
    fn open(&self) -> Result<()> {
        if self.0.state.lock().unwrap().closed {
            Err(Error::Conflict("Coding scheduler is stopping.".into()))
        } else {
            Ok(())
        }
    }
    async fn wait(task: Task) -> Result<Value> {
        task.wait()
            .await
            .map_err(|e| Error::Conflict(e.to_string()))
    }
    async fn reconcile(&self) -> Result<()> {
        self.open()?;
        self.reconcile_intakes().await?;
        self.reconcile_comparisons().await?;
        if let Some(holder) = self.store(|s| s.coding_slot())? {
            let id = text(&holder, "id")?;
            if holder["purpose"] == "evaluation" {
                return Ok(());
            }
            let publication = self.store(|s| s.publication(id))?.unwrap_or(Value::Null);
            if present(&publication["merged"]) {
                self.store(|s| s.release_coding_slot(id, "merged", None))?;
                self.kick();
                return Ok(());
            }
            if present(&holder["resumeRequest"]) && !self.0.workflow.busy(id) {
                Self::wait(self.0.workflow.resume(id)?).await?;
                return Ok(());
            }
            if publication["ready"] == true && !present(&holder["sourceConflict"]) {
                if holder["slotRelease"]["candidateId"] != publication["candidateId"] {
                    self.store(|s| {
                        s.release_coding_slot(id, "review", publication["candidateId"].as_str())
                    })?;
                    Self::wait(self.0.workflow.pause(id, true, Some("review"))?).await?;
                    self.kick();
                    return Ok(());
                }
                if holder["slotRelease"]["reason"] == "review"
                    && holder["status"] == "paused"
                    && holder["desiredState"] == "paused"
                    && ["review", "validation"]
                        .contains(&holder["pauseReason"].as_str().unwrap_or(""))
                {
                    self.store(|s| {
                        s.release_coding_slot(id, "review", publication["candidateId"].as_str())
                    })?;
                    self.kick();
                    return Ok(());
                }
            }
            if present(&holder["schedule"])
                && !self.0.workflow.busy(id)
                && !self.0.launches.busy(id)
                && self.replay(&holder)?
            {
                Self::wait(self.launch(holder, None)?).await?;
            }
            return Ok(());
        }
        let snapshot = self.store(|s| s.snapshot())?;
        let mut resumes: Vec<_> = array(&snapshot["runs"])
            .iter()
            .filter(|r| {
                present(&r["resumeRequest"])
                    && ["paused", "attention_required"]
                        .contains(&r["status"].as_str().unwrap_or(""))
                    && r["desiredState"] != "cancelled"
            })
            .collect();
        resumes.sort_by_key(|r| r["resumeRequest"]["requestedAt"].as_str().unwrap_or(""));
        if let Some(run) = resumes.first() {
            let id = text(run, "id")?;
            if self.store(|s| s.unstopped_coding_run(id))?.is_none() {
                Self::wait(self.0.workflow.resume(id)?).await?;
            }
            return Ok(());
        }
        if self.store(|s| s.unstopped_coding_run(""))?.is_some() {
            return Ok(());
        }
        let mut reholds = Vec::new();
        for run in array(&snapshot["runs"]) {
            let publication = self
                .store(|s| s.publication(text(run, "id")?))?
                .unwrap_or(Value::Null);
            if run["slotRelease"]["reason"] == "review"
                && ["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
                && run["desiredState"] == "paused"
                && !present(&publication["merged"])
                && run["feedbackResumeHold"]["terminal"] != true
                && array(&run["feedbackContinuations"])
                    .last()
                    .is_none_or(|v| v["state"] != "blocked")
                && (present(&run["sourceConflict"]) || present(&publication["reviewInvalidated"]))
            {
                reholds.push(run);
            }
        }
        reholds.sort_by_key(|r| r["slotRelease"]["at"].as_str().unwrap_or(""));
        if let Some(run) = reholds.first() {
            self.store(|s| s.reclaim_coding_slot(text(run, "id")?, true))?;
            return Ok(());
        }
        for plan in array(&self.store(|s| s.coding_queue())?["plans"]) {
            if present(&plan["implementationPlan"]) {
                let result = self.refresh_source(text(plan, "taskId")?).await;
                if let Err(error) = result {
                    self.store(|s| {
                        s.set_launch_hold(
                            text(plan, "id")?,
                            &plan["launch"],
                            Some(&error.to_string()),
                        )
                    })?;
                }
            }
        }
        for plan in array(&self.store(|s| s.coding_queue())?["plans"]) {
            let mut eligible = plan.clone();
            eligible.as_object_mut().unwrap().remove("launchHold");
            if !present(&plan["launch"])
                || (present(&plan["launchHold"]) && plan["launchHold"]["source"] != "preflight")
                || self.store(|s| s.eligibility_hold(&eligible))?.is_some()
            {
                continue;
            }
            let id = text(plan, "id")?;
            let result = self.0.store.roles().and_then(|roles| {
                self.0
                    .workflow
                    .preflight_start(id, &json!({"team":plan["launch"]["team"]}), &roles)
            });
            let reason = result.err().map(|e| e.to_string());
            self.store(|s| s.set_launch_hold(id, &plan["launch"], reason.as_deref()))?;
        }
        for plan in array(&self.store(|s| s.coding_queue())?["plans"]) {
            self.open()?;
            let merges = stringify(&self.store(|s| s.dependency_merges(plan))?);
            if merges == "[]"
                || (!present(&plan["launch"]) && !present(&plan["dependencyBaseHold"]))
            {
                continue;
            }
            let hold = self.store(|s| s.eligibility_hold(plan))?;
            if hold.is_some() && hold.as_deref() != plan["dependencyBaseHold"]["reason"].as_str() {
                continue;
            }
            let id = text(plan, "id")?;
            let reason = self.0.work.dependency_base_hold(id, None).await?;
            if plan["dependencyBaseHold"]["merges"] != merges
                || plan["dependencyBaseHold"]["reason"].as_str() != reason.as_deref()
            {
                self.store(|s| s.save_dependency_base_hold(id, &merges, reason.as_deref()))?;
            }
        }
        self.open()?;
        if let Some(run) = self.store(|s| s.claim_next_coding_slot())? {
            Self::wait(self.launch(run, None)?).await?;
        }
        Ok(())
    }
    fn replay(&self, run: &Value) -> Result<bool> {
        let Some(decision) = run["schedule"]["id"].as_str() else {
            return Ok(false);
        };
        self.store(|s| {
            s.can_replay_initial_launch(
                text(run, "id")?,
                decision,
                run["controlRevision"].as_u64().unwrap_or(0),
            )
        })
    }
    pub fn launch(&self, run: Value, manual: Option<Value>) -> Result<Task> {
        self.open()?;
        let id = text(&run, "id")?.to_owned();
        let this = self.clone();
        self.0
            .launches
            .enqueue(&id, Some("launch"), move || async move {
                this.perform_launch(run, manual).await.map_err(Arc::new)
            })
    }
    pub async fn prepare(&self, id: &str, input: &Value) -> Result<Value> {
        let prepared = self.0.work.prepare(id, input).await?;
        self.0.evidence.configure(
            id,
            prepared["repository"].get("checks").unwrap_or(&json!([])),
        )?;
        Ok(prepared)
    }
    async fn perform_launch(&self, run: Value, manual: Option<Value>) -> Result<Value> {
        let id = text(&run, "id")?;
        let scheduled = manual.is_none();
        let mut dispatched = false;
        let result = async {
            self.open()?;
            if scheduled && (!self.replay(&run)? || self.0.workflow.busy(id)) {
                return Ok(Value::Null);
            }
            if present(&run["launch"])
                && let Err(error) = self.prepare(id, &run["launch"]["repository"]).await
            {
                if !matches!(error, Error::RepositoryConfigurationChanged(_)) {
                    return Err(error);
                }
                self.prepare(id, &run["launch"]["repository"]).await?;
            }
            self.open()?;
            if scheduled {
                if !self.replay(&run)? || self.0.workflow.busy(id) {
                    return Ok(Value::Null);
                }
                dispatched = true;
                Self::wait(self.0.workflow.start_scheduled(
                    id,
                    text(&run["schedule"], "id")?,
                    run["controlRevision"].as_u64().unwrap_or(0),
                    json!({"team":run["launch"]["team"]}),
                    self.0.store.roles()?,
                )?)
                .await
            } else {
                let current = self.store(|s| s.get_run(id))?;
                if ["cancelled", "paused"].contains(&current["desiredState"].as_str().unwrap_or(""))
                    || current["status"] != "awaiting_runner"
                    || self
                        .store(|s| s.coding_slot())?
                        .is_none_or(|r| r["id"] != id)
                {
                    return Ok(Value::Null);
                }
                dispatched = true;
                Self::wait(self.0.workflow.start(
                    id,
                    manual.clone().unwrap(),
                    self.0.store.roles()?,
                )?)
                .await
            }
        }
        .await;
        let Err(error) = result else {
            return result;
        };
        if scheduled && !dispatched && !self.replay(&run)? {
            return Ok(Value::Null);
        }
        if self.store(|s| s.repository_work(id))?.is_none()
            && !array(&self.store(|s| s.dependency_merges(&s.get_run(id)?))?).is_empty()
            && let Some(reason) = self.0.work.dependency_base_hold(id, None).await?
            && self
                .store(|s| s.coding_slot())?
                .is_some_and(|r| r["id"] == id)
        {
            self.store(|s| s.defer_unadmitted_launch(id, Some(&reason), true))?;
            return Ok(Value::Null);
        }
        let current = self.store(|s| s.get_run(id))?;
        let reason = if matches!(error, Error::RepositoryConfigurationChanged(_)) {
            "Repository configuration changed again during preparation. Inspect the current configuration and Save launch details to retry.".into()
        } else if !dispatched && present(&run["launch"]) {
            let message = error.to_string();
            if [
                "Continuation held:",
                "Selected repository checkout is unavailable",
                "Selected branch is already checked out",
                "Generated branch already exists",
                "Worktree destination already exists",
            ]
            .iter()
            .any(|prefix| message.starts_with(prefix))
            {
                message
            } else {
                "Repository preparation could not complete. Inspect the selected checkout and Save launch details to retry.".into()
            }
        } else {
            error.to_string()
        };
        if !["cancelled", "paused"].contains(&current["desiredState"].as_str().unwrap_or(""))
            && self.store(|s| s.repository_work(id))?.is_none()
            && array(&self.store(|s| s.runtime(id))?["assignments"]).is_empty()
            && self
                .store(|s| s.coding_slot())?
                .is_some_and(|r| r["id"] == id)
        {
            self.store(|s| s.defer_unadmitted_launch(id, Some(&reason), false))?;
            if scheduled {
                return Ok(Value::Null);
            }
        } else if current["desiredState"] != "cancelled"
            && (current["desiredState"] != "paused" || current["pauseReason"] != "operator")
        {
            self.store(|s| s.require_attention(id, &reason))?;
        }
        Err(error)
    }
}
