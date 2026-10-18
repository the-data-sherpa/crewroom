use super::*;

impl<S: StoreAccess> Workflow<S> {
    pub(super) async fn transition_failure(
        &self,
        run: &str,
        error: crewroom_core::Error,
    ) -> Result<()> {
        self.stop_run(run, None).await?;
        if self.store(|s| s.get_run(run))?["desiredState"] == "running" {
            self.store(|s| s.require_attention(run, &error.to_string()))?;
        }
        Ok(())
    }
    pub(super) async fn apply_pending_coding(&self, run: &str) -> Result<()> {
        let current = self.store(|s| s.get_run(run))?;
        if current["status"] != "running"
            || current["desiredState"] != "running"
            || self.check_open().is_err()
        {
            return Ok(());
        }
        let coding = self.store(|s| s.coding_record(run))?.unwrap_or(Value::Null);
        let Some(completion) = array(&coding["completions"])
            .iter()
            .find(|c| c["state"] == "requested")
        else {
            return Ok(());
        };
        let result = async {
            if !self.agents_idle(run).await? {
                return Ok(());
            }
            self.current(run, revision(&current)?)?;
            self.store(|s| s.begin_coding_transition(run, text(completion, "id")?))?;
            self.perform_coding_transition(run, text(completion, "id")?, revision(&current)?)
                .await
        }
        .await;
        if let Err(e) = result {
            self.transition_failure(run, e).await?;
        }
        Ok(())
    }
    pub(super) async fn perform_coding_transition(
        &self,
        run: &str,
        completion: &str,
        control: u64,
    ) -> Result<()> {
        let _reservation = self.reserve(run)?;
        let stopped = self.stop_run(run, None).await?;
        if stopped["status"] != "paused" {
            return Err(conflict(
                "Coding completion is blocked by unresolved ownership.",
            ));
        }
        self.current(run, control)?;
        self.reconcile_deliveries(run, &self.messages(run)?).await?;
        let state = |s: &Store| -> Result<Value> {
            Ok(
                array(&s.coding_record(run)?.unwrap_or(Value::Null)["completions"])
                    .iter()
                    .find(|c| c["id"] == completion)
                    .map(|c| c["state"].clone())
                    .unwrap_or(Value::Null),
            )
        };
        if self.store(state)? != "requested" {
            return Err(conflict(
                "Coding completion is no longer requested; agents cannot resume from it.",
            ));
        }
        self.0.services.evidence.complete(run, completion).await?;
        if self.store(state)? != "completed" {
            return Err(conflict(
                "Coding completion did not produce a candidate; agents remain stopped.",
            ));
        }
        if self.store(|s| s.get_run(run))?["status"] == "attention_required" {
            return Err(conflict(
                "Promoted revision needs operator attention before agents can continue.",
            ));
        }
        self.continue_run(run, control, &HashSet::new()).await?;
        self.store(|s| s.finish_coding_transition(run))?;
        Ok(())
    }
    pub(super) async fn apply_pending_team(&self, run: &str) -> Result<()> {
        let current = self.store(|s| s.get_run(run))?;
        if current["status"] != "running"
            || current["desiredState"] != "running"
            || self.check_open().is_err()
        {
            return Ok(());
        }
        let Some(proposal) = self
            .store(|s| s.team_proposals(run))?
            .into_iter()
            .find(|p| p["status"] == "pending")
        else {
            return Ok(());
        };
        let result = async {
            let roles = self.0.roles.read()?;
            if self
                .store(|s| s.reject_unavailable_team_proposal(text(&proposal, "id")?, &roles))?
                .is_some()
            {
                self.notify(run);
                return Ok(());
            }
            if !self.agents_idle(run).await? {
                return Ok(());
            }
            self.current(run, revision(&current)?)?;
            self.store(|s| s.begin_team_transition(text(&proposal, "id")?))?;
            self.perform_team_transition(run, text(&proposal, "id")?, revision(&current)?)
                .await
        }
        .await;
        if let Err(e) = result {
            self.transition_failure(run, e).await?;
        }
        Ok(())
    }
    pub(super) async fn perform_team_transition(
        &self,
        run: &str,
        proposal: &str,
        control: u64,
    ) -> Result<()> {
        let _reservation = self.reserve(run)?;
        let stopped = self.stop_run(run, None).await?;
        if stopped["status"] != "paused" {
            return Err(conflict("Team change is blocked by unresolved ownership."));
        }
        self.current(run, control)?;
        self.reconcile_deliveries(run, &self.messages(run)?).await?;
        let roles = self.0.roles.read()?;
        let principles = self.0.config.principles()?;
        self.store(|s| {
            let proposed = s.get_team_proposal(proposal)?;
            if self.0.config.verify_team_admission(&proposed).is_err() {
                let reason = "Model selection changed or became unavailable before insertion. Refresh the preview and submit a new specialist proposal.";
                s.reject_team_proposal(proposal, reason)?;
                return Err(conflict(reason));
            }
            s.apply_team_proposal_with_principles(proposal, &roles, &principles)
        })?;
        self.continue_run(run, control, &HashSet::new()).await?;
        self.store(|s| s.finish_team_transition(run))?;
        Ok(())
    }
    pub(super) async fn apply_pending_research(&self, run: &str) -> Result<()> {
        let current = self.store(|s| s.get_run(run))?;
        if current["roleDraft"]["proposalId"].is_string()
            && current["status"] == "running"
            && current["desiredState"] == "running"
        {
            if self.agents_idle(run).await? {
                let result = async {
                    self.current(run, revision(&current)?)?;
                    let stopped = self.stop_run(run, None).await?;
                    if stopped["status"] != "paused" {
                        return Err(conflict(
                            "Draft completion is blocked by unresolved ownership.",
                        ));
                    }
                    self.current(run, revision(&current)?)?;
                    self.store(|s| s.close_refinement(run))?;
                    self.notify(run);
                    Ok(())
                }
                .await;
                if let Err(e) = result {
                    self.transition_failure(run, e).await?;
                }
            }
            return Ok(());
        }
        if current["purpose"] != "research"
            || !present(&current, "researchCompletion")
            || current["status"] != "running"
            || current["desiredState"] != "running"
        {
            return Ok(());
        }
        if let Some(jira) = &self.0.services.jira
            && jira.needs_research_summary(run, current["taskRevision"].as_u64().unwrap_or(0))?
        {
            return Ok(());
        }
        if let Some(github) = &self.0.services.github
            && github.needs_research_summary(run, current["taskRevision"].as_u64().unwrap_or(0))?
        {
            return Ok(());
        }
        let result = async {
            if !self.agents_idle(run).await? {
                return Ok(());
            }
            self.current(run, revision(&current)?)?;
            let stopped = self.stop_run(run, None).await?;
            if stopped["status"] != "paused" {
                return Err(conflict(
                    "Research completion is blocked by unresolved ownership.",
                ));
            }
            self.current(run, revision(&current)?)?;
            let result = self.0.services.research.read(run)?;
            if result["assessment"]["ready"] != true
                || result["assessment"]["revision"] != current["researchCompletion"]["revision"]
            {
                return Err(conflict("Research evidence changed before completion."));
            }
            self.0.services.research.complete(run)?;
            self.notify(run);
            Ok(())
        }
        .await;
        if let Err(e) = result {
            self.transition_failure(run, e).await?;
        }
        Ok(())
    }
}
