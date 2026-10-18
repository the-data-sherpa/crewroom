use super::*;

impl<S: StoreAccess> Workflow<S> {
    pub fn pause(&self, run: &str, preserve_intent: bool, reason: Option<&str>) -> Result<Task> {
        self.check_open()?;
        if !preserve_intent {
            self.store(|s| {
                if present(&s.get_run(run)?, "resumeRequest") {
                    s.clear_resume_request(run, "held")?;
                }
                if s.get_run(run)?["desiredState"] != "cancelled" {
                    s.request_pause(run)?;
                }
                Ok(())
            })?;
        }
        let this = self.clone();
        let id = run.to_owned();
        let reason = reason.map(String::from);
        self.0.tasks.enqueue(run, None, move || async move {
            this.stop_run(&id, reason.as_deref())
                .await
                .map_err(Arc::new)
        })
    }
    pub fn cancel(&self, run: &str) -> Result<Task> {
        self.check_open()?;
        self.store(|s| {
            if present(&s.get_run(run)?, "resumeRequest") {
                s.clear_resume_request(run, "cancelled")?;
            }
            s.set_desired_state(run, "cancelled")?;
            Ok(())
        })?;
        self.pause(run, true, None)
    }
    /// Called inside the workflow lane. Cleanup uses independent service lanes;
    /// every physical owner is attempted even when a journal or provider fails.
    pub(super) async fn stop_run(&self, run: &str, reason: Option<&str>) -> Result<Value> {
        let original = self.store(|s| s.get_run(run));
        let terminal = original.as_ref().is_ok_and(|r| r["status"] == "completed");
        let mut errors = Vec::new();
        if !terminal && let Err(e) = self.store(|s| s.begin_pause(run, reason)) {
            errors.push(e.to_string());
        }
        // Publish the durable intermediate state before awaiting owned children.
        // Otherwise clients can show "running" throughout a slow confirmed stop.
        (self.0.changed)(run);
        let (physical, cleanup) = self.cleanup_owners(run).await?;
        errors.extend(cleanup);
        if physical && !terminal {
            self.store(|s| s.finish_pause(run))?;
        }
        if !errors.is_empty() {
            errors.sort();
            errors.dedup();
            let error = errors.join(" ");
            self.failure(run, &error);
            return self.store(|s| s.require_attention(run, &error));
        }
        (self.0.changed)(run);
        self.store(|s| s.get_run(run))
    }
    pub(super) async fn cleanup_owners(&self, run: &str) -> Result<(bool, Vec<String>)> {
        let mut errors = Vec::new();
        let host = self.0.hosts.stop_run(run);
        let repository = self.0.services.repository.stop_run(run);
        let publication = async {
            match &self.0.services.publication {
                Some(p) => p.stop_run(run).await,
                None => Ok(()),
            }
        };
        let (hosts, work, published) = tokio::join!(wait(host), wait(repository), publication);
        let mut physical = true;
        for (name, report) in [("Agent", hosts), ("Repository", work)] {
            match report {
                Ok(report) => {
                    if report["stopped"] != true {
                        physical = false;
                        errors.push(format!("{name} ownership remains unresolved."));
                    }
                    for item in array(&report["attempts"]) {
                        if let Some(e) = item["journalError"].as_str() {
                            errors.push(e.into());
                        }
                    }
                }
                Err(e) => {
                    physical = false;
                    errors.push(e.to_string());
                }
            }
        }
        if let Err(e) = published {
            physical = false;
            errors.push(e.to_string());
        }
        if let Err(error) = self.stop_evaluation_maintenance(run).await {
            physical = false;
            errors.push(error.to_string());
        }
        if physical {
            let has_work = self.store(|s| Ok(s.repository_work(run)?.is_some()));
            match has_work {
                Ok(true) => {
                    if let Err(e) = self
                        .0
                        .services
                        .repository
                        .reconcile_stopped_writer(run)
                        .await
                    {
                        errors.push(e.to_string());
                    }
                    if let Err(e) = self
                        .0
                        .services
                        .repository
                        .reconcile_stopped_edits(run)
                        .await
                    {
                        errors.push(e.to_string());
                    }
                }
                Ok(false) => {}
                Err(e) => errors.push(e.to_string()),
            }
            if let Some(checks) = &self.0.services.checks
                && self.store(|s| Ok(s.coding_record(run)?.is_some()))?
                && let Err(e) = checks.reconcile_stopped(run).await
            {
                errors.push(e.to_string());
            }
            // Ports are reusable only after exact host and command ownership stop.
            let ids = self.store(|s| {
                Ok(array(&s.runtime(run)?["attempts"])
                    .iter()
                    .filter_map(|a| a["id"].as_str().map(String::from))
                    .collect::<Vec<_>>())
            })?;
            self.0
                .state
                .lock()
                .unwrap()
                .ports
                .retain(|id, _| !ids.contains(id));
        }
        Ok((physical, errors))
    }
    /// Fencing is synchronous. The registered worker owns all dependencies even
    /// if the caller drops its waiter or the HTTP connection closes.
    pub fn close(&self) -> Result<Task> {
        self.0.state.lock().unwrap().closing = true;
        let initialization = self.0.cleanup.keyed("__initialize__", "initialize");
        self.0.tasks.fence();
        self.0.timer.cancel();
        let mut runs = self.0.tasks.runs();
        runs.extend(self.0.state.lock().unwrap().admitting.keys().cloned());
        let cached: Vec<_> = self.0.messages.lock().unwrap().keys().cloned().collect();
        for run in cached {
            if self.store(|s| s.get_run(&run)).is_ok_and(|r| {
                matches!(
                    r["status"].as_str(),
                    Some("running" | "awaiting_runner" | "pausing")
                )
            }) {
                runs.push(run);
            }
        }
        let mut initial_error = None;
        match self.store(|s| {
            let mut result = Vec::new();
            for r in array(&s.snapshot()?["runs"]) {
                if array(&s.runtime(text(r, "id")?)?["attempts"])
                    .iter()
                    .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
                {
                    result.push(text(r, "id")?.to_owned());
                }
            }
            Ok(result)
        }) {
            Ok(saved) => runs.extend(saved),
            Err(e) => initial_error = Some(e.to_string()),
        }
        runs.sort();
        runs.dedup();
        for id in &runs {
            if let Err(e) = self.store(|s| {
                let run = s.get_run(id)?;
                if run["status"] != "completed" && run["desiredState"] != "cancelled" {
                    s.set_desired_state(id, "paused")?;
                }
                Ok(())
            }) {
                initial_error = Some(e.to_string());
            }
        }
        let this = self.clone();
        self.0
            .cleanup
            .enqueue("__close__", Some("close"), move || async move {
                let mut errors: Vec<String> = initial_error.into_iter().collect();
                let timer = this.0.timer_task.lock().unwrap().take();
                if let Some(timer) = timer
                    && let Err(e) = timer.await
                {
                    errors.push(e.to_string());
                }
                // Cancel external execution first; queued workflow jobs observe the
                // closing fence rather than launching while their caller is drained.
                let hosts = this.0.hosts.close();
                let repository = this.0.services.repository.close();
                let discovery = async {
                    match &this.0.config.discovery {
                        Some(catalog) => catalog.close().await,
                        None => Ok(()),
                    }
                };
                let providers = async {
                    let mut failures = Vec::new();
                    if let Some(p) = &this.0.services.publication
                        && let Err(e) = p.close().await
                    {
                        failures.push(e.to_string());
                    }
                    if let Some(j) = &this.0.services.jira
                        && let Err(e) = j.close().await
                    {
                        failures.push(e.to_string());
                    }
                    failures
                };
                let (hosts, repository, provider_errors, discovery) =
                    tokio::join!(wait(hosts), wait(repository), providers, discovery);
                if let Err(error) = discovery {
                    errors.push(error.to_string());
                }
                errors.extend(provider_errors);
                for report in [hosts, repository] {
                    match report {
                        Ok(v) => {
                            if v["stopped"] != true {
                                errors.push("Shutdown ownership remains unresolved.".into());
                            }
                            for run in array(&v["runs"]) {
                                for attempt in array(&run["attempts"]) {
                                    if let Some(error) = attempt["journalError"].as_str() {
                                        errors.push(error.into());
                                    }
                                }
                            }
                        }
                        Err(e) => errors.push(e.to_string()),
                    }
                }
                this.0.tasks.drain().await;
                // Initialization owns the controller too; do not acknowledge
                // close while its worker can still inspect or mutate the store.
                if let Some(initialization) = initialization {
                    let _ = initialization.wait().await;
                }
                for run in runs {
                    match this.stop_run(&run, None).await {
                        Ok(v)
                            if matches!(
                                v["status"].as_str(),
                                Some("paused" | "cancelled" | "completed")
                            ) => {}
                        Ok(v) => errors.push(
                            v["attentionReason"]
                                .as_str()
                                .unwrap_or("Workflow shutdown requires attention.")
                                .into(),
                        ),
                        Err(e) => errors.push(e.to_string()),
                    }
                }
                errors.sort();
                errors.dedup();
                Ok(json!({"stopped":errors.is_empty(),"errors":errors}))
            })
    }
}
