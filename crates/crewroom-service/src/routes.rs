use crate::{ApiError, array, execution::Execution, text, unavailable, wait};
use crewroom_core::Error;
use crewroom_runtime::messaging::StoreAccess;
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) async fn mutate(
    e: &Arc<Execution>,
    path: &str,
    body: &Value,
) -> Result<Option<Value>, ApiError> {
    let parts: Vec<_> = path.split('/').collect();
    let value = match parts.as_slice() {
        ["comparisons", "preflight"] => {
            let w = e.workflow.as_ref().ok_or_else(unavailable)?;
            match w.comparison_preflight(body) {
                Ok(v) => match e
                    .store
                    .with_store(|s| s.validate_comparison_proposal(&v["manifest"]["target"]))
                {
                    Ok(()) => v,
                    Err(error) => json!({"canStart":false,"reasons":[error.to_string()]}),
                },
                Err(error) => json!({"canStart":false,"reasons":[error.to_string()]}),
            }
        }
        ["comparisons"] => {
            if let Some(receipt) = e.store.with_store(|s| s.comparison_create_replay(body))? {
                receipt
            } else {
                let w = e.workflow.as_ref().ok_or_else(unavailable)?;
                w.verify_comparison_manifest(&body["manifest"])?;
                e.store.with_store(|s| s.create_comparison(body))?
            }
        }
        ["comparisons", id, "control"] => {
            e.workflow.as_ref().ok_or_else(unavailable)?;
            e.store.with_store(|s| s.control_comparison(id, body))?
        }
        ["comparisons", id, "adoption"] => {
            if let Some(receipt) = e
                .store
                .with_store(|s| s.comparison_adoption_replay(id, body))?
            {
                receipt
            } else {
                let w = e.workflow.as_ref().ok_or_else(unavailable)?;
                let comparison = e.store.with_store(|s| s.get_comparison(id))?;
                w.verify_comparison_adoption(&comparison, body)?;
                e.store
                    .with_store(|s| s.record_comparison_adoption(id, body))?
            }
        }
        ["jira", "sync"] => {
            e.sync
                .as_ref()
                .ok_or_else(|| Error::Conflict("Jira connection is not configured.".into()))?
                .refresh()
                .await?
        }
        ["jira", "reconcile"] => {
            e.jira
                .as_ref()
                .ok_or_else(|| Error::Conflict("Jira connection is not configured.".into()))?
                .flush()
                .await?;
            e.notify_all()?;
            json!(e.store.with_store(|s| s.jira_updates(None))?)
        }
        ["github", "delivery", "reconcile"] => {
            e.github.reconcile().await?;
            e.notify_all()?;
            json!(e.store.with_store(|s| s.github_updates(None))?)
        }
        ["github", "delivery", id, "retry"] => {
            e.github.retry(id)?;
            e.github.reconcile().await?;
            e.notify_all()?;
            json!(e.store.with_store(|s| s.github_updates(None))?)
        }
        ["runs", id, "publication", "reconcile"] => {
            e.store.with_store(|s| s.get_run(id))?;
            e.publication.flush().await?;
            e.notify_all()?;
            e.publication.view(id).await?
        }
        ["role-drafts"] => {
            let workflow = e.workflow.as_ref().ok_or_else(unavailable)?;
            let run = e.store.with_store(|s| s.queue_role_draft(body))?;
            let id = text(&run, "id")?;
            if run["status"] == "awaiting_runner"
                && array(&e.store.with_store(|s| s.runtime(id))?["assignments"]).is_empty()
            {
                wait(workflow.start_refinement(id)?).await?;
            }
            e.store.with_store(|s| s.get_run(id))?
        }
        ["tasks", id, "refinement-runs"] => {
            let workflow = e.workflow.as_ref().ok_or_else(unavailable)?;
            let run = e.store.with_store(|s| s.queue_refinement(id, body))?;
            let id = text(&run, "id")?;
            if run["status"] == "awaiting_runner"
                && array(&e.store.with_store(|s| s.runtime(id))?["assignments"]).is_empty()
            {
                wait(workflow.start_refinement(id)?).await?;
            }
            e.store.with_store(|s| s.get_run(id))?
        }
        ["runs", id, "start"] => {
            let w = e.workflow.as_ref().ok_or_else(unavailable)?;
            let run = e.store.with_store(|s| s.get_run(id))?;
            match run["purpose"].as_str().unwrap_or("coding") {
                "explore" | "grill" => {
                    if body != &json!({}) {
                        return Err(Error::Invalid(
                            "Refinement start does not accept a team.".into(),
                        )
                        .into());
                    }
                    wait(w.start_refinement(id)?).await?
                }
                "research" => wait(w.start(id, body.clone(), e.store.roles()?)?).await?,
                "coding" => {
                    e.store.with_store(|s| s.assert_coding_slot_available(id))?;
                    if let Some(reason) = e.work.dependency_base_hold(id, None).await? {
                        return Err(Error::Conflict(reason).into());
                    }
                    w.preflight_start(id, body, &e.store.roles()?)?;
                    let decided = e.store.with_store(|s| s.claim_coding_slot(id, true))?;
                    wait(
                        e.scheduler
                            .as_ref()
                            .ok_or_else(unavailable)?
                            .launch(decided, Some(body.clone()))?,
                    )
                    .await?
                }
                _ => {
                    return Err(Error::Conflict(
                        "Use the comparison controls for evaluation runs.".into(),
                    )
                    .into());
                }
            }
        }
        ["runs", id, "pause"] => {
            wait(
                e.workflow
                    .as_ref()
                    .ok_or_else(unavailable)?
                    .pause(id, false, None)?,
            )
            .await?
        }
        ["runs", id, "resume"] => {
            let expected = body
                .get("controlRevision")
                .map(|value| {
                    value.as_u64().ok_or_else(|| {
                        Error::Invalid("controlRevision must be a nonnegative integer.".into())
                    })
                })
                .transpose()?;
            wait(
                e.workflow
                    .as_ref()
                    .ok_or_else(unavailable)?
                    .resume_at_control(id, expected)?,
            )
            .await?
        }
        ["runs", id, "cancel"] => {
            let run = e.store.with_store(|s| s.get_run(id))?;
            if body.get("requestId").is_some() && run.get("position").is_some() {
                e.store.with_store(|s| s.cancel_queued(id, body))?
            } else if let Some(w) = &e.workflow {
                wait(w.cancel(id)?).await?
            } else {
                e.store.with_store(|s| {
                    s.set_desired_state(id, "cancelled")?;
                    s.begin_pause(id, Some("cancelled"))?;
                    s.finish_pause(id)
                })?
            }
        }
        ["runs", id, "close"] => {
            let run = e.store.with_store(|s| s.get_run(id))?;
            if !["explore", "grill"].contains(&run["purpose"].as_str().unwrap_or("")) {
                return Err(Error::Conflict("Only refinement runs can be closed.".into()).into());
            }
            if run["status"] == "closed" {
                run
            } else {
                if let Some(w) = &e.workflow
                    && !["cancelled", "paused"].contains(&run["status"].as_str().unwrap_or(""))
                {
                    wait(w.pause(id, false, None)?).await?;
                }
                e.store.with_store(|s| s.close_refinement(id))?
            }
        }
        ["runs", id, "adopt-requirements"] | ["runs", id, "requirements", "adopt"] => {
            let run = e.store.with_store(|s| s.adopt_requirements(id, body))?;
            if run["status"] == "pausing"
                && let Some(w) = &e.workflow
            {
                wait(w.pause(id, true, None)?).await?;
            }
            if let Some(w) = &e.workflow {
                w.notify(id);
            }
            e.store.with_store(|s| s.get_run(id))?
        }
        ["runs", id, "specialists"] => e
            .workflow
            .as_ref()
            .ok_or_else(unavailable)?
            .insert_specialist(id, body)?,
        ["runs", id, "assignments", assignment, "repair-allowance"] => {
            let result = e
                .store
                .with_store(|s| s.grant_repair_cycle(id, assignment, body))?;
            if let Some(w) = &e.workflow {
                w.notify(id);
            }
            result
        }
        ["runs", id, "budget"] => {
            let result = e.store.with_store(|s| s.set_active_limit(id, body))?;
            if let Some(w) = &e.workflow {
                w.notify(id);
            }
            result
        }
        ["runs", id, "repository"] => {
            let prepared = e.work.prepare(id, body).await?;
            e.evidence.configure(
                id,
                prepared["repository"].get("checks").unwrap_or(&json!([])),
            )?;
            prepared
        }
        ["runs", id, "check-environment"] => e.evidence.correct_check_environment(id, body)?,
        ["runs", id, "target-refresh"] | ["runs", id, "continue-later"] => {
            let owned = e.clone();
            let id = (*id).to_owned();
            let input = body.clone();
            let continuation = parts[2] == "continue-later";
            let lane = id.clone();
            let perform = move || async move {
                if continuation {
                    owned
                        .work
                        .queue_cancelled_continuation(&id, &input, &owned.store.roles()?)
                        .await
                } else {
                    owned.target.refresh(&id, &input).await
                }
            };
            let value = if let Some(w) = &e.workflow {
                wait(w.operator_transition(&lane, perform)?).await?
            } else {
                perform().await?
            };
            if let Some(w) = &e.workflow {
                w.notify(&lane);
            }
            value
        }
        _ => return Ok(None),
    };
    // Admission controls always return durable state, not a successful receipt for an unstarted plan.
    Ok(Some(value))
}
