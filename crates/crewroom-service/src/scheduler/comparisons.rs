//! Comparisons contend for the same serial repository slot as ordinary coding.
use super::*;
impl Scheduler {
    pub(super) async fn reconcile_comparisons(&self) -> Result<()> {
        for comparison in self.store(|s| s.comparisons())? {
            let id = text(&comparison, "id")?;
            let Some(cell) = array(&comparison["cells"])
                .iter()
                .find(|c| c.get("result").is_none() && c["state"] != "cancelled")
            else {
                continue;
            };
            if let Some(run_id) = cell["runId"].as_str() {
                if self.0.workflow.busy(run_id) {
                    continue;
                }
                let run = self.store(|s| s.get_run(run_id))?;
                if array(&cell["events"])
                    .iter()
                    .any(|e| matches!(e["type"].as_str(), Some("handoff" | "finalization")))
                {
                    self.0.workflow.finalize_evaluation(run_id).await?;
                    if self.store(|s| s.get_run(run_id))?["status"] != "attention_required" {
                        self.kick();
                    }
                    continue;
                }
                let desired = comparison["desiredState"].as_str().unwrap_or("planned");
                if desired == "paused" || desired == "cancelled" {
                    if !matches!(
                        run["status"].as_str(),
                        Some("paused" | "cancelled" | "completed")
                    ) {
                        Self::wait(self.0.workflow.pause(run_id, true, None)?).await?;
                    }
                    if desired == "cancelled" {
                        self.0
                            .workflow
                            .finish_evaluation_result(
                                run_id,
                                "cancelled",
                                Some("Operator cancelled this comparison."),
                            )
                            .await?;
                    }
                    continue;
                }
                if desired != "running" {
                    continue;
                }
                if run["pauseReason"] == "budget" {
                    Self::wait(self.0.workflow.pause(run_id, true, None)?).await?;
                    self.0
                        .workflow
                        .finish_evaluation_result(
                            run_id,
                            "incomplete",
                            Some("Frozen active allowance exhausted."),
                        )
                        .await?;
                    self.kick();
                    continue;
                }
                if run["status"] == "attention_required" {
                    // Unknown ownership stays held. Confirmed provider failure has
                    // a retained unavailable result, never a fallback model turn.
                    let runtime = self.store(|s| s.runtime(run_id))?;
                    let stopped = array(&runtime["attempts"])
                        .iter()
                        .all(|a| a["state"] == "admitted" || a["ownership"] == "stopped");
                    let reason = run["attentionReason"]
                        .as_str()
                        .unwrap_or("Evaluation needs attention.");
                    if stopped && reason.contains("provider unavailable") {
                        self.0
                            .workflow
                            .finish_evaluation_result(run_id, "unavailable", Some(reason))
                            .await?;
                        self.hold_unavailable_comparison(run_id)?;
                    }
                    continue;
                }
                if run["status"] == "paused" {
                    Self::wait(self.0.workflow.resume(run_id)?).await?;
                } else if run["status"] == "awaiting_runner"
                    && array(&self.store(|s| s.runtime(run_id))?["assignments"]).is_empty()
                {
                    self.launch_comparison(run_id).await?;
                }
            } else if comparison["desiredState"] == "running"
                && self.store(|s| s.coding_slot())?.is_none()
                && self.store(|s| s.unstopped_coding_run(""))?.is_none()
            {
                let run = self.store(|s| s.create_comparison_cell_run(id, text(cell, "id")?))?;
                self.launch_comparison(text(&run, "id")?).await?;
            }
        }
        Ok(())
    }
    async fn launch_comparison(&self, run: &str) -> Result<()> {
        let result = async {
            self.0.workflow.prepare_evaluation(run).await?;
            if self.comparison_stop_requested(run)? {
                self.kick();
                return Ok(());
            }
            Self::wait(self.0.workflow.start_evaluation(run)?).await?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(error) = result {
            // A stop can win during preparation or between the check above and
            // atomic team admission. Let normal reconciliation settle its
            // requested state instead of recording a provider failure.
            if self.comparison_stop_requested(run)? {
                self.kick();
                return Ok(());
            }
            Self::wait(self.0.workflow.pause(run, true, None)?).await?;
            if self.comparison_stop_requested(run)? {
                self.kick();
                return Ok(());
            }
            self.0
                .workflow
                .finish_evaluation_result(run, "unavailable", Some(&error.to_string()))
                .await?;
            self.hold_unavailable_comparison(run)?;
        }
        Ok(())
    }
    fn comparison_stop_requested(&self, run: &str) -> Result<bool> {
        self.store(|s| {
            let link = s
                .comparison_cell_for_run(run)?
                .ok_or_else(|| Error::Invalid("Evaluation has no comparison.".into()))?;
            Ok(matches!(
                link["desiredState"].as_str(),
                Some("paused" | "cancelled")
            ))
        })
    }
    fn hold_unavailable_comparison(&self, run: &str) -> Result<()> {
        self.store(|s| {
            let link = s
                .comparison_cell_for_run(run)?
                .ok_or_else(|| Error::Invalid("Evaluation has no comparison.".into()))?;
            let comparison = s.get_comparison(text(&link, "comparisonId")?)?;
            if comparison["desiredState"] == "running"
                && !matches!(
                    comparison["state"].as_str(),
                    Some("completed" | "incomplete" | "cancelled")
                )
            {
                s.control_comparison(
                    text(&link, "comparisonId")?,
                    &json!({"requestId":run,"action":"pause"}),
                )?;
            }
            Ok(())
        })
    }
}
