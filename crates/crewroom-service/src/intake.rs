//! Intake uses server-produced Git proofs; no Git process runs under the workflow mutex.
use crate::{ApiError, AppState, operation};
use crewroom_core::Error;
use serde_json::Value;

async fn refresh(state: &AppState, task_id: &str, revision: u64) -> Result<Value, ApiError> {
    let id = task_id.to_owned();
    let prepared = operation(state, move |s| s.store.prepare_plan_source(&id, revision)).await?;
    let pinned = tokio::task::spawn_blocking(move || prepared.validate())
        .await
        .map_err(|_| Error::Conflict("Intake source inspection was interrupted.".into()))??;
    operation(state, move |s| s.store.save_plan_source(&pinned)).await
}
pub(crate) async fn mutate(
    state: &AppState,
    path: &str,
    body: &Value,
) -> Result<Option<Value>, ApiError> {
    let parts: Vec<_> = path.split('/').collect();
    let result = match parts.as_slice() {
        ["intakes", id, "answers"] => {
            let intake_id = id.to_string();
            let input = body.clone();
            if let Some(prior) = operation(state, move |s| {
                s.store.intake_answer_replay(&intake_id, &input)
            })
            .await?
            {
                return Ok(Some(prior));
            }
            let intake_id = id.to_string();
            let task = operation(state, move |s| {
                let intake = s.store.get_intake(&intake_id)?;
                s.store.get_task(intake["taskId"].as_str().unwrap())
            })
            .await?;
            refresh(
                state,
                task["id"].as_str().unwrap(),
                task["revision"].as_u64().unwrap(),
            )
            .await?;
            let intake_id = id.to_string();
            let input = body.clone();
            operation(state, move |s| s.store.answer_intake(&intake_id, &input)).await?
        }
        ["tasks", id, "plan-source"] => {
            refresh(
                state,
                id,
                body["revision"]
                    .as_u64()
                    .ok_or_else(|| Error::Invalid("Task revision required.".into()))?,
            )
            .await?
        }
        ["tasks", id, "runs"] => {
            let task_id = id.to_string();
            let input = body.clone();
            if let Some(prior) = operation(state, move |s| {
                s.store.queued_intake_replay(&task_id, &input)
            })
            .await?
            {
                return Ok(Some(prior));
            }
            refresh(
                state,
                id,
                body["revision"]
                    .as_u64()
                    .ok_or_else(|| Error::Invalid("Task revision required.".into()))?,
            )
            .await?;
            let task_id = id.to_string();
            let input = body.clone();
            operation(state, move |s| {
                s.store.queue_with_playbooks(
                    &task_id,
                    &input,
                    &s.roles.read()?,
                    &crewroom_core::playbooks::Playbooks::beside_roles(&s.roles).read()?,
                )
            })
            .await?
        }
        ["implementation-plans", id, "decision"] => {
            let plan_id = id.to_string();
            let input = body.clone();
            // Replay succeeds without another Git observation, including after source removal.
            let prior = operation(state, move |s| {
                s.store.plan_decision_replay(&plan_id, &input)
            })
            .await?;
            if let Some(prior) = prior {
                return Ok(Some(prior));
            }
            let plan_id = id.to_string();
            let task_id =
                operation(state, move |s| s.store.implementation_plan_task(&plan_id)).await?;
            refresh(
                state,
                task_id.as_str().unwrap(),
                body["revision"]
                    .as_u64()
                    .ok_or_else(|| Error::Invalid("Task revision required.".into()))?,
            )
            .await?;
            let plan_id = id.to_string();
            let input = body.clone();
            operation(state, move |s| {
                s.store.decide_implementation_plan(&plan_id, &input)
            })
            .await?
        }
        _ => return Ok(None),
    };
    state.changes.send_modify(|v| *v += 1);
    if let Some(execution) = &state.execution {
        execution.changed();
    }
    Ok(Some(result))
}
