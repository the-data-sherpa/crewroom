use super::*;

impl Scheduler {
    pub(super) async fn reconcile_intakes(&self) -> Result<()> {
        let intakes = self.store(|s| s.intakes())?;
        // Finish decision stops before considering another intake coordinator.
        for intake in &intakes {
            if intake["state"] == "handed_off" {
                continue;
            }
            let id = text(intake, "scopingRunId")?;
            let run = self.store(|s| s.get_run(id))?;
            if self.0.workflow.busy(id) {
                continue;
            }
            if ["waiting", "review", "handoff", "cancelled"]
                .contains(&intake["state"].as_str().unwrap_or(""))
            {
                let result = async {
                    if !["paused", "closed", "cancelled"]
                        .contains(&run["status"].as_str().unwrap_or(""))
                    {
                        Self::wait(self.0.workflow.pause(id, false, Some("intake"))?).await?;
                    }
                    if intake["state"] == "handoff" {
                        self.refresh_source(text(intake, "taskId")?).await?;
                        let roles = self.0.store.roles()?;
                        let books = self.0.store.playbooks()?;
                        self.store(|s| s.handoff_intake(text(intake, "id")?, &roles, &books))?;
                    }
                    Ok(())
                }
                .await;
                let reason = result.err().map(|e: Error| e.to_string());
                self.store(|s| s.record_intake_hold(text(intake, "id")?, reason.as_deref()))?;
            }
        }
        let current = self.store(|s| s.intakes())?;
        for intake in &current {
            if intake["state"] == "handed_off" {
                continue;
            }
            let id = text(intake, "scopingRunId")?;
            let runtime = self.store(|s| s.runtime(id))?;
            let run = self.store(|s| s.get_run(id))?;
            if run["status"] == "running" && intake["state"] == "answer_pending" {
                self.store(|s| s.intake_resumed(text(intake, "id")?))?;
            }
            if self.0.workflow.busy(id)
                || array(&runtime["attempts"])
                    .iter()
                    .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            {
                return Ok(());
            }
        }
        for intake in current {
            if !["scoping", "answer_pending"].contains(&intake["state"].as_str().unwrap_or("")) {
                continue;
            }
            let id = text(&intake, "scopingRunId")?;
            let run = self.store(|s| s.get_run(id))?;
            if run["desiredState"] == "cancelled"
                || run["status"] == "attention_required"
                || run["status"] == "closed"
            {
                continue;
            }
            let result = if intake["state"] == "answer_pending"
                && run["status"] == "paused"
                && run["pauseReason"] == "intake"
            {
                match self.0.workflow.resume(id) {
                    Ok(task) => Self::wait(task).await,
                    Err(error) => Err(error),
                }
            } else if run["status"] == "awaiting_runner" && run["desiredState"] != "paused" {
                match self.0.workflow.start_refinement(id) {
                    Ok(task) => Self::wait(task).await,
                    Err(error) => Err(error),
                }
            } else {
                continue;
            };
            let reason = result.err().map(|e| e.to_string());
            self.store(|s| s.record_intake_hold(text(&intake, "id")?, reason.as_deref()))?;
            self.store(|s| s.intake_resumed(text(&intake, "id")?))?;
            break;
        }
        Ok(())
    }
    pub(super) async fn refresh_source(&self, task_id: &str) -> Result<()> {
        let prepared = self.store(|s| {
            let task = s.get_task(task_id)?;
            s.prepare_plan_source(task_id, task["revision"].as_u64().unwrap())
        })?;
        let pinned = tokio::task::spawn_blocking(move || prepared.validate())
            .await
            .map_err(|_| Error::Conflict("Source validation interrupted.".into()))??;
        self.store(|s| s.save_plan_source(&pinned))?;
        Ok(())
    }
}
