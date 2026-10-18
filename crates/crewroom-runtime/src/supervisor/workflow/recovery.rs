use super::*;

impl<S: StoreAccess> Workflow<S> {
    /// Call and await once before attaching the daemon's scheduler. Each run
    /// still uses its normal driven lane, so observation of other runs continues.
    pub fn initialize(&self) -> Result<Task> {
        // Serialize initialization admission with the synchronous close fence.
        let state = self.0.state.lock().unwrap();
        if state.closing {
            return Err(conflict("Workflow supervision is stopping."));
        }
        let this = self.clone();
        self.0
            .cleanup
            .enqueue("__initialize__", Some("initialize"), move || async move {
                if let Some(catalog) = &this.0.config.discovery {
                    catalog.recover_processes().await.map_err(Arc::new)?;
                }
                let runs = this.store(|s| s.snapshot()).map_err(Arc::new)?;
                for run in array(&runs["runs"]) {
                    let id = text(run, "id").map_err(Arc::new)?.to_owned();
                    let worker = this.clone();
                    let queued_id = id.clone();
                    let task = this
                        .0
                        .tasks
                        .enqueue(&id, None, move || async move {
                            match worker.initialize_run(&queued_id).await {
                                Ok(value) => Ok(value),
                                Err(error) => {
                                    // Unexpected recovery errors must not leave independently
                                    // owned agents running behind a workflow attention state.
                                    if worker.check_open().is_ok() {
                                        let cleanup =
                                            worker.stop_run(&queued_id, Some("recovery")).await;
                                        let reason = match cleanup {
                                            Ok(_) => error.to_string(),
                                            Err(cleanup) => format!("{error} Cleanup: {cleanup}"),
                                        };
                                        worker.failure(&queued_id, &reason);
                                        let _ = worker
                                            .store(|s| s.require_attention(&queued_id, &reason));
                                    }
                                    Err(Arc::new(error))
                                }
                            }
                        })
                        .map_err(Arc::new)?;
                    let _ = task.wait().await;
                    if this.check_open().is_err() {
                        break;
                    }
                }
                this.start_timer();
                Ok(Value::Null)
            })
    }
    async fn collect_saved_events(&self, run: &str) -> Result<()> {
        let attempts = self.store(|s| s.runtime(run))?["attempts"].clone();
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            for attempt in array(&attempts) {
                let id = text(attempt, "id")?;
                let root = this.0.config.root.join(id);
                if root.try_exists()? {
                    crate::canonical(&root)?;
                }
                super::super::journal::visit_events(&root, |sequence, event| {
                    this.store(|s| s.record_agent_event(id, sequence, event))
                })?;
            }
            Ok(())
        })
        .await
        .map_err(|_| conflict("Saved workflow event recovery was interrupted."))?
    }
    async fn initialize_run(&self, run: &str) -> Result<Value> {
        self.check_open()?;
        // Durable agent_start outranks a pre-crash markerless launch decision.
        if let Err(e) = self.collect_saved_events(run).await {
            self.stop_run(run, Some("recovery")).await?;
            return self.store(|s| s.require_attention(run, &e.to_string()));
        }
        if self.evaluation_is_finalizing(run)? {
            self.stop_run(run, Some("evaluation")).await?;
            return self.snapshot(run);
        }
        let mut current = self.store(|s| s.normalize_launch_execution(run))?;
        if present(&current, "schedule")
            && !present(&current, "launchCompletedAt")
            && present(&current, "launch")
            && !present(&current, "slotRelease")
            && current["desiredState"] == "running"
        {
            let attempts = self.store(|s| s.runtime(run))?["attempts"].clone();
            if array(&attempts)
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
                && self.stop_run(run, Some("recovery")).await?["status"] != "paused"
            {
                return self.store(|s| {
                    s.require_attention(run, "Interrupted coding launch has unresolved ownership.")
                });
            }
            current = self.store(|s| s.normalize_launch_execution(run))?;
            if present(&current, "launchCompletedAt") {
                let automatic = self.started_assignments(run)?;
                return self
                    .continue_run(run, revision(&current)?, &automatic)
                    .await;
            }
            if current["status"] == "paused" {
                self.store(|s| s.restart_scheduled_launch(run))?;
            }
            return self.snapshot(run);
        }
        if present(&current, "codingTransition") {
            let coding = self.store(|s| s.coding_record(run))?.unwrap_or(Value::Null);
            if array(&coding["completions"]).iter().any(|c| {
                c["id"] == current["codingTransition"]["completionId"] && c["state"] == "rejected"
            }) {
                self.stop_run(run, None).await?;
                return self.store(|s|s.require_attention(run,"The recorded conflict completion was rejected. Choose a reasoned target retry; do not resume agents."));
            }
        }
        if current["desiredState"] == "running" && current["status"] != "attention_required" {
            for field in ["codingTransition", "teamTransition"] {
                if present(&current, field)
                    && current[field]["controlRevision"] == current["controlRevision"]
                {
                    let control = revision(&current)?;
                    let result = if field == "codingTransition" {
                        self.perform_coding_transition(
                            run,
                            text(&current[field], "completionId")?,
                            control,
                        )
                        .await
                    } else {
                        self.perform_team_transition(
                            run,
                            text(&current[field], "proposalId")?,
                            control,
                        )
                        .await
                    };
                    if let Err(e) = result {
                        self.transition_failure(run, e).await?;
                    }
                    return self.snapshot(run);
                }
            }
        }
        let runtime = self.store(|s| s.runtime(run))?;
        let attempts: Vec<_> = array(&runtime["attempts"])
            .iter()
            .filter(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            .collect();
        if attempts.is_empty() {
            // Receipt-backed command and Git cleanup remains required for
            // paused/cancelled/completed runs with no observable agent hosts.
            let (_, errors) = self.cleanup_owners(run).await?;
            if !errors.is_empty() {
                return self.store(|s| s.require_attention(run, &errors.join(" ")));
            }
            if current["status"] == "pausing"
                || current["resumeRequest"]["state"] == "starting"
                    && current["status"] == "awaiting_runner"
            {
                self.stop_run(run, Some("recovery")).await?;
                if let Some(reason) = current["sourceConflict"].as_str() {
                    self.store(|s| s.require_attention(run, reason))?;
                }
            }
            return self.snapshot(run);
        }
        let mut failed = HashSet::new();
        let mut uncertain = false;
        for attempt in attempts {
            if !active(&runtime)
                .iter()
                .any(|a| a["currentAttemptId"] == attempt["id"])
            {
                uncertain = true;
                continue;
            }
            let owned = self.0.hosts.owned_state(attempt).await?;
            if owned["state"] == "uncertain" {
                uncertain = true;
                continue;
            }
            if owned["state"] == "stopped" {
                failed.insert(text(attempt, "assignmentId")?.to_owned());
                continue;
            }
            if current["desiredState"] != "running"
                || !matches!(
                    current["status"].as_str(),
                    Some("running" | "awaiting_runner")
                )
            {
                continue;
            }
            match wait(self.0.hosts.reattach(text(attempt, "id")?)).await {
                Ok(_) => {
                    if let Some(port) = attempt["hostUrl"]
                        .as_str()
                        .and_then(|s| url::Url::parse(s).ok())
                        .and_then(|u| u.port())
                    {
                        self.0
                            .state
                            .lock()
                            .unwrap()
                            .ports
                            .insert(text(attempt, "id")?.into(), port);
                    }
                }
                Err(_) => {
                    if self.0.hosts.owned_state(attempt).await?["state"] == "stopped" {
                        failed.insert(text(attempt, "assignmentId")?.into());
                    } else {
                        uncertain = true;
                    }
                }
            }
        }
        if uncertain {
            self.stop_run(run, None).await?;
            self.store(|s|{s.clear_resume_request(run,"held")?;s.require_attention(run,"Agent ownership or host identity is uncertain after restart. Reconcile before continuing.")})?;
        } else if !failed.is_empty()
            || !matches!(
                current["status"].as_str(),
                Some("running" | "awaiting_runner")
            )
            || current["desiredState"] != "running"
        {
            if !failed.is_empty()
                && current["desiredState"] == "running"
                && current["status"] == "running"
            {
                self.continue_run(run, revision(&current)?, &failed).await?;
            } else {
                self.stop_run(run, None).await?;
                if let Some(reason) = current["sourceConflict"].as_str().or_else(|| {
                    if current["status"] == "attention_required" {
                        current["attentionReason"].as_str()
                    } else {
                        None
                    }
                }) {
                    self.store(|s| s.require_attention(run, reason))?;
                }
            }
            self.store(|s| s.clear_resume_request(run, "held"))?;
        } else {
            self.store(|s| s.clear_resume_request(run, "held"))?;
            self.messages(run)?;
            self.notify(run);
        }
        self.snapshot(run)
    }
    fn started_assignments(&self, run: &str) -> Result<HashSet<String>> {
        self.store(|s| {
            active(&s.runtime(run)?)
                .iter()
                .filter_map(
                    |a| match s.get_attempt(a["currentAttemptId"].as_str().unwrap_or("")) {
                        Ok(attempt) if attempt["state"] != "admitted" => {
                            Some(text(a, "id").map(String::from))
                        }
                        Ok(_) => None,
                        Err(e) => Some(Err(e)),
                    },
                )
                .collect()
        })
    }
}
