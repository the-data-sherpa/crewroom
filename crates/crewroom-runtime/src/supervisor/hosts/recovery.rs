use super::*;
use crate::{
    CheckedProcess, CheckedSpec, container_command::ContainerCommand, inspect_checked,
    inspect_settled, seal_checked, stop_owned,
};
use std::time::Duration;

impl<S: StoreAccess> HostManager<S> {
    fn descriptor(&self, root: &Path) -> Result<Option<CheckedProcess>> {
        let path = root.join("host-intent.json");
        if !path.try_exists()? {
            return Ok(None);
        }
        let value = private_json(&path, 256 * 1024)?;
        let descriptor: CheckedProcess = serde_json::from_value(value["descriptor"].clone())?;
        let spec: CheckedSpec = serde_json::from_value(value["spec"].clone())?;
        if crate::spec::describe_retained_checked(&spec)? != descriptor
            || descriptor.root != root.join("host-process")
            || descriptor.controller_root != self.0.authority.root()
            || spec.command.cwd != root
            || spec.command.args != vec![root.join("host.json").to_string_lossy().into_owned()]
        {
            return Err(conflict(
                "Retained host descriptor does not match its supervised attempt.",
            ));
        }
        Ok(Some(descriptor))
    }
    pub async fn owned_state(&self, attempt: &Value) -> Result<Value> {
        let root = self.root(text(attempt, "id")?)?;
        let process_dir = crate::runner::process_dir(&attempt["model"])?;
        let pid_key = crate::runner::pid_key(&attempt["model"])?;
        crewroom_core::verify_runner_identity(&attempt["model"], attempt)?;
        if existing_directory(&root.join(process_dir)).is_err() {
            return Ok(
                json!({"state":"uncertain","reason":"Runner ownership directory changed identity."}),
            );
        }
        let descriptor = match self.descriptor(&root) {
            Ok(v) => v,
            Err(_) => {
                return Ok(
                    json!({"state":"uncertain","reason":"Saved host descriptor is unreadable or inconsistent."}),
                );
            }
        };
        if descriptor.is_none()
            && (attempt.get("hostPid").is_some()
                || root.join("host-process/launch.json").try_exists()?)
        {
            return Ok(
                json!({"state":"uncertain","reason":"Host has no verified native launch descriptor."}),
            );
        }
        let host = if let Some(descriptor) = descriptor {
            inspect_checked(&descriptor).await
        } else {
            inspect_settled(&root.join("host-process")).await
        };
        let omp = inspect_settled(&root.join(process_dir)).await;
        if host.state == "uncertain"
            || omp.state == "uncertain"
            || (host.state == "live"
                && host.owner.as_ref().map(|p| i64::from(p.pid)) != attempt["hostPid"].as_i64())
            || (omp.state == "live"
                && omp.owner.as_ref().map(|p| i64::from(p.pid)) != attempt[pid_key].as_i64())
            || (attempt.get(pid_key).is_some()
                && !root.join(process_dir).join("launch.json").try_exists()?)
        {
            return Ok(
                json!({"state":"uncertain","reason":"Host or runner OS identity differs from its saved attempt."}),
            );
        }
        Ok(
            json!({"state":if host.state=="live" && omp.state=="live" {"live"} else if host.state=="stopped" && omp.state=="stopped" {"stopped"} else {"incomplete"}}),
        )
    }
    pub fn reattach(&self, attempt_id: &str) -> Result<Task> {
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let run = text(&attempt, "runId")?.to_owned();
        let slot = {
            let mut registry = self.0.registry.lock().unwrap();
            if registry.closing
                || registry.stopping.contains(&run)
                || registry.slots.contains_key(attempt_id)
            {
                return Err(conflict("Attempt is already registered or stopping."));
            }
            let slot = Arc::new(Slot::new(&run, attempt_id, self.root(attempt_id)?, None));
            registry.slots.insert(attempt_id.into(), slot.clone());
            slot
        };
        let this = self.clone();
        self.0
            .tasks
            .enqueue(&run, Some(attempt_id), move || async move {
                this.reattach_owned(&slot, &attempt).await.map_err(Arc::new)
            })
    }
    async fn reattach_owned(&self, slot: &Arc<Slot>, attempt: &Value) -> Result<Value> {
        let saved = private_json(&slot.root.join("host.json"), 16 * 1024 * 1024)?;
        if saved["attemptId"] != slot.attempt
            || saved["root"] != json!(slot.root)
            || saved["backend"] != self.0.options.backend
        {
            return Err(conflict(
                "Retained host configuration or backend origin changed.",
            ));
        }
        let client = HostClient::new(text(attempt, "hostUrl")?, text(&saved, "token")?)?;
        let endpoint = private_json(&slot.root.join("endpoint.json"), 4096)?;
        if super::super::client::origin(client.url())?
            .port()
            .map(u64::from)
            != endpoint["port"].as_u64()
        {
            return Err(conflict(
                "Retained endpoint differs from its saved host URL.",
            ));
        }
        if self.owned_state(attempt).await?["state"] != "live" {
            return Err(conflict(
                "Retained agent process ownership is not fully live.",
            ));
        }
        let identity = client
            .request(&json!({"op":"identity"}), &slot.cancel)
            .await;
        if let Ok(identity) = &identity {
            Self::check_identity(attempt, identity)?;
        }
        self.collect_events(slot).await?;
        self.0.tasks.with_open(|| {
            let registry = self.0.registry.lock().unwrap();
            if registry.closing
                || registry.stopping.contains(&slot.run)
                || slot.cancel.is_cancelled()
            {
                return Err(conflict("Reattachment was stopped."));
            }
            self.store(|s| {
                let latest = s.get_attempt(&slot.attempt)?;
                let run = s.get_run(&slot.run)?;
                if run["desiredState"] != "running"
                    || !matches!(run["status"].as_str(), Some("running" | "awaiting_runner"))
                    || latest["ownership"] != "held"
                {
                    return Err(conflict(
                        "Retained attempt no longer has execution permission.",
                    ));
                }
                Self::check_identity(&latest, attempt)?;
                if !s.runtime(&slot.run)?["assignments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| {
                        a["currentAttemptId"] == slot.attempt
                            && a.get("retiredAt").is_none_or(Value::is_null)
                            && a.get("rebriefRequired").is_none_or(Value::is_null)
                    })
                {
                    return Err(conflict(
                        "Retained attempt is no longer the current assignment.",
                    ));
                }
                Ok(())
            })?;
            *slot.client.lock().unwrap() = Some(client);
            Ok(())
        })?;
        let terminal = self.terminal(&slot.run, &slot.attempt).await?;
        self.tmux_output(
            &[
                "set-option",
                "-w",
                "-t",
                text(&terminal, "window")?,
                "window-size",
                "manual",
            ],
            false,
        )
        .await?;
        self.register_observer(slot, identity.is_ok())?;
        Ok(json!({"reattached":true,"degraded":identity.is_err()}))
    }
    pub(super) fn check_identity(attempt: &Value, actual: &Value) -> Result<()> {
        crewroom_core::verify_runner_identity(&attempt["model"], actual)?;
        if [
            "hostPid",
            crate::runner::pid_key(&attempt["model"])?,
            "sessionId",
        ]
        .iter()
        .any(|k| attempt.get(*k).is_none() || attempt[*k] != actual[*k])
        {
            return Err(conflict(
                "Live host/runner/session identity differs from its saved attempt.",
            ));
        }
        Ok(())
    }
    pub(super) async fn collect_events(&self, slot: &Slot) -> Result<u64> {
        existing_directory(&slot.root)?;
        let root = slot.root.clone();
        let attempt = slot.attempt.clone();
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            super::super::journal::visit_events(&root, |sequence, event| {
                this.store(|s| s.record_agent_event(&attempt, sequence, event))
            })
        })
        .await
        .map_err(|_| conflict("Saved agent journal recovery was interrupted."))?
    }
    pub fn stop_run(&self, run: &str) -> Result<Task> {
        let epoch = {
            let mut registry = self.0.registry.lock().unwrap();
            registry.stopping.insert(run.into());
            *registry.stop_epochs.entry(run.into()).or_default() += 1;
            let epoch = registry.stop_epochs[run];
            for slot in registry.slots.values().filter(|slot| slot.run == run) {
                slot.cancel.cancel();
            }
            epoch
        };
        let this = self.clone();
        let run_id = run.to_owned();
        self.0.cleanup.enqueue(run, None, move || async move {
            this.stop_registered(&run_id, epoch).await.map_err(Arc::new)
        })
    }
    async fn stop_registered(&self, run: &str, epoch: u64) -> Result<Value> {
        self.0.tasks.drain_run(run).await;
        let (attempts, mut failure) =
            match self.store(|s| Ok(s.runtime(run)?["attempts"].as_array().unwrap().clone())) {
                Ok(attempts) => (attempts, None),
                Err(error) => (Vec::new(), Some(error)),
            };
        let mut slots: HashMap<_, _> = self
            .0
            .registry
            .lock()
            .unwrap()
            .slots
            .iter()
            .filter(|(_, slot)| slot.run == run)
            .map(|(id, slot)| (id.clone(), slot.clone()))
            .collect();
        let mut reports = Vec::new();
        for attempt in attempts {
            let mut add = || -> Result<()> {
                let id = text(&attempt, "id")?;
                if attempt["state"] == "admitted" && !self.root(id)?.try_exists()? {
                    return Ok(());
                }
                let mut registry = self.0.registry.lock().unwrap();
                let slot = registry
                    .slots
                    .entry(id.into())
                    .or_insert(Arc::new(Slot::new(run, id, self.root(id)?, None)))
                    .clone();
                slots.insert(id.into(), slot);
                Ok(())
            };
            if let Err(error) = add() {
                failure = Some(error);
            }
        }
        for slot in slots.values() {
            slot.cancel.cancel();
            match self.stop_attempt(slot).await {
                Ok(report) => reports.push(report),
                Err(error) => failure = Some(error),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let stopped = reports.iter().all(|v| v["stopped"] == true);
        if stopped {
            let mut registry = self.0.registry.lock().unwrap();
            if registry.stop_epochs.get(run) == Some(&epoch) {
                registry.stopping.remove(run);
            }
        }
        Ok(json!({"stopped":stopped,"attempts":reports}))
    }
    pub(super) async fn stop_attempt(&self, slot: &Slot) -> Result<Value> {
        let _stop = slot.stop.lock().await;
        slot.cancel.cancel();
        let observer = slot.observer.lock().unwrap().clone();
        if let Some(observer) = observer {
            let _ = observer.wait().await;
        }
        existing_directory(&slot.root)?;
        let client = slot.client.lock().unwrap().clone();
        if let Some(client) = client {
            let _ = client
                .request(&json!({"op":"close"}), &CancellationToken::new())
                .await;
        }
        let (attempt, store_error) = match self.store(|s| s.get_attempt(&slot.attempt)) {
            Ok(attempt) => (attempt, None),
            Err(error) => (json!({"id":slot.attempt}), Some(error)),
        };
        let mut reasons = Vec::new();
        let retained =
            private_json(&slot.root.join("host.json"), 16 * 1024 * 1024).unwrap_or(Value::Null);
        let model = attempt
            .get("model")
            .filter(|v| !v.is_null())
            .unwrap_or(&retained["model"]);
        let process_dir = crate::runner::process_dir(model)?;
        let pid_key = crate::runner::pid_key(model)?;
        // Stop the host before enumerating any tool intent: no new contained
        // command may appear after that enumeration.
        let mut final_host = None;
        match self.descriptor(&slot.root) {
            Ok(Some(descriptor)) => {
                let sealed = seal_checked(&descriptor).await;
                if let Some(saved_pid) = attempt["hostPid"].as_i64()
                    && sealed.owner.as_ref().map(|owner| i64::from(owner.pid)) != Some(saved_pid)
                {
                    reasons.push("Saved host PID does not match its owned launch record.".into());
                }
                if sealed.state == "live" {
                    let _ = stop_owned(&descriptor.root).await;
                }
                final_host = Some(descriptor);
            }
            Ok(None) => {
                if attempt.get("hostPid").is_some()
                    || slot.root.join("host-process").try_exists().unwrap_or(true)
                {
                    reasons.push("Host native ownership descriptor is missing.".into());
                }
            }
            Err(_) => reasons.push("Host ownership descriptor is unreadable.".into()),
        }
        let omp_root = slot.root.join(process_dir);
        let omp = if existing_directory(&omp_root).is_err() {
            crate::Ownership {
                state: "uncertain".into(),
                reason: Some("Runner ownership directory changed identity.".into()),
                owner: None,
                members: None,
            }
        } else {
            stop_owned(&omp_root).await
        };
        if let Some(saved_pid) = attempt[pid_key].as_i64()
            && omp.owner.as_ref().map(|owner| i64::from(owner.pid)) != Some(saved_pid)
        {
            reasons.push("Saved runner PID does not match its owned launch record.".into());
        }
        if omp.state != "stopped"
            || (omp_root.join("owner.json").try_exists().unwrap_or(true)
                && !omp_root.join("launch.json").try_exists().unwrap_or(false))
            || (attempt.get(pid_key).is_some()
                && !slot
                    .root
                    .join(process_dir)
                    .join("launch.json")
                    .try_exists()
                    .unwrap_or(false))
        {
            reasons.push("Runner ownership remains unresolved.".into());
        }
        self.stop_tools(&slot.root, &mut reasons).await;
        // An OMP child can retain its parent's ownership FD after host death.
        // Inspect again after stopping that exact runner and all tool owners.
        if let Some(descriptor) = final_host
            && seal_checked(&descriptor).await.state != "stopped"
        {
            reasons.push("Host ownership remains unresolved.".into());
        }
        let journal = self.collect_events(slot).await.err().map(|_| {
            format!(
                "Saved agent events for {} are unreadable; inspect the retained journal.",
                slot.attempt
            )
        });
        let stopped = reasons.is_empty();
        self.status(
            slot,
            if stopped { "stopped" } else { "blocked" },
            journal
                .as_deref()
                .or_else(|| reasons.first().map(String::as_str)),
        );
        if let Some(error) = store_error {
            return Err(error);
        }
        let unlaunched = attempt["state"] == "admitted"
            && !slot
                .root
                .join("host-intent.json")
                .try_exists()
                .unwrap_or(true)
            && !slot.root.join("host-process").try_exists().unwrap_or(true)
            && !slot.root.join(process_dir).try_exists().unwrap_or(true)
            && !slot.root.join("tools").try_exists().unwrap_or(true);
        self.store(|s| {
            if unlaunched && s.get_attempt(&slot.attempt)?["state"] == "admitted" {
                Ok(())
            } else {
                s.record_ownership(&slot.attempt, stopped, reasons.first().map(String::as_str))
            }
        })?;
        // A queued launch that never claimed its attempt remains reusable. It
        // must not consume a recovery cycle or acquire a false execution history.
        if unlaunched && stopped {
            let mut registry = self.0.registry.lock().unwrap();
            if registry
                .slots
                .get(&slot.attempt)
                .is_some_and(|saved| std::ptr::eq(saved.as_ref(), slot))
            {
                registry.slots.remove(&slot.attempt);
            }
        }
        Ok(
            json!({"attemptId":slot.attempt,"stopped":stopped,"ownershipErrors":reasons,"journalError":journal}),
        )
    }
    async fn stop_tools(&self, root: &Path, reasons: &mut Vec<String>) {
        let tools = root.join("tools");
        if existing_directory(&tools).is_err() {
            reasons.push("Contained-command directory changed identity.".into());
            return;
        }
        let entries = match std::fs::read_dir(&tools) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => {
                reasons.push("Contained-command directory is unreadable.".into());
                return;
            }
        };
        for entry in entries {
            let cleanup = async {
                let path = entry?.path();
                canonical(&path)?;
                if path.join("launch.json").try_exists()? {
                    return Err(conflict(
                        "Legacy tool process needs explicit ownership reconciliation.",
                    ));
                }
                let engine = self.0.options.command_engine.as_ref().ok_or_else(|| {
                    conflict("Restore the contained command engine before cleanup.")
                })?;
                let job = ContainerCommand::new(path, engine)?;
                if job.stop().await?["state"] != "stopped" {
                    return Err(conflict("Contained command ownership remains unresolved."));
                }
                job.remove().await
            }
            .await;
            if let Err(error) = cleanup {
                reasons.push(error.to_string());
            }
        }
    }
    /// Shutdown is itself registered and driven. The caller may be cancelled;
    /// authority remains held by this manager until verified cleanup completes.
    pub fn close(&self) -> Result<Task> {
        self.0.tasks.fence();
        {
            let mut registry = self.0.registry.lock().unwrap();
            registry.closing = true;
            for slot in registry.slots.values() {
                slot.cancel.cancel();
            }
        }
        let this = self.clone();
        self.0
            .cleanup
            .enqueue("__close__", Some("close"), move || async move {
                this.0.tasks.drain().await;
                let known = this.store(|s| {
                    s.snapshot()?["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| text(v, "id").map(String::from))
                        .collect::<Result<Vec<_>>>()
                });
                let (mut runs, mut failure) = match known {
                    Ok(runs) => (runs, None),
                    Err(error) => (Vec::new(), Some(Arc::new(error))),
                };
                runs.extend(
                    this.0
                        .registry
                        .lock()
                        .unwrap()
                        .slots
                        .values()
                        .map(|slot| slot.run.clone()),
                );
                runs.sort();
                runs.dedup();
                let mut results = Vec::new();
                for run in runs {
                    match this.stop_run(&run) {
                        Ok(task) => match task.wait().await {
                            Ok(result) => results.push(result),
                            Err(error) => failure = Some(error),
                        },
                        Err(error) => failure = Some(Arc::new(error)),
                    }
                }
                this.0.observers.fence();
                this.0.observers.drain().await;
                if let Some(error) = failure {
                    return Err(error);
                }
                let stopped = results.iter().all(|v| v["stopped"] == true);
                // Never erase the dedicated terminal server while process ownership
                // is uncertain. It may be the operator's only remaining view.
                if stopped {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(16),
                        this.tmux_output(&["kill-server"], false),
                    )
                    .await;
                }
                Ok(json!({"stopped":stopped,"runs":results}))
            })
    }
}
