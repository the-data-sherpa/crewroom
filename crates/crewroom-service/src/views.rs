//! Run-scoped operator projections. Provider state, workflow state and observation are separate.
mod history;
mod needs;
use crate::{array, execution::Execution, present, text};
use crewroom_core::{Error, Result};
use crewroom_runtime::messaging::StoreAccess;
use serde_json::{Value, json};
use std::collections::HashMap;

pub(super) fn commands(events: &[Value]) -> Value {
    let mut rows: Vec<Value> = Vec::new();
    for item in events {
        let event = &item["event"];
        let Some(key) = event["toolCallId"].as_str() else {
            continue;
        };
        let index = rows.iter().position(|r| r["toolCallId"] == key);
        match event["type"].as_str() {
            Some("tool_started" | "tool_receipt_reused") => {
                let row = json!({"attemptId":item["attemptId"],"toolCallId":key,"commandId":event.get("commandId").cloned().unwrap_or(json!("command")),"state":"running"});
                if let Some(index) = index {
                    rows[index] = row;
                } else {
                    rows.push(row);
                }
            }
            Some("tool_exited") => {
                if let Some(index) = index {
                    rows[index]["state"] = json!("exited");
                    if let Some(code) = event["result"].get("exitCode") {
                        rows[index]["exitCode"] = code.clone();
                    }
                }
            }
            _ => (),
        }
    }
    json!(rows)
}
pub(super) fn workspace(e: &Execution, v: &mut Value) -> Result<()> {
    let mut activity = Vec::new();
    let mut runs = HashMap::new();
    for run in array(&v["runs"]) {
        let id = text(run, "id")?;
        let state = if let Some(w) = &e.workflow {
            w.snapshot(id)?
        } else {
            e.store.with_store(|s| s.runtime(id))?
        };
        let publication = e
            .store
            .with_store(|s| s.publication(id))?
            .unwrap_or(Value::Null);
        runs.insert(
            id.to_owned(),
            needs::RunState {
                runtime: state.clone(),
                publication,
                publication_available: e.publication.available(id)?,
            },
        );
        for assignment in array(&state["assignments"]) {
            if let Some(attempt) = array(&state["attempts"])
                .iter()
                .find(|a| a["id"] == assignment["currentAttemptId"])
            {
                let status = if array(&state["degradedAttemptIds"]).contains(&attempt["id"]) {
                    "observation_degraded"
                } else if array(&state["observedAttemptIds"]).contains(&attempt["id"])
                    || ["exited", "admitted"].contains(&attempt["state"].as_str().unwrap_or(""))
                {
                    attempt["state"].as_str().unwrap_or("ownership_unconfirmed")
                } else {
                    "ownership_unconfirmed"
                };
                let mut row =
                    json!({"runId":id,"assignmentName":assignment["name"],"state":status});
                for key in ["ompPid", "runnerKind", "runnerPid", "ownershipReason"] {
                    if let Some(value) = attempt.get(key) {
                        row[key] = value.clone();
                    }
                }
                activity.push(row);
            }
        }
    }
    if let Some(repos) = v["repositories"].as_array_mut() {
        for r in repos {
            let path = std::path::PathBuf::from(r["path"].as_str().unwrap_or(""));
            let canonical = std::fs::canonicalize(&path).unwrap_or(path);
            let available = e.publication_paths.contains(&canonical);
            r["publicationAvailable"] = json!(available);
            if let Some(unavailable) = e
                .publication_unavailable
                .iter()
                .find(|u| !available && u.repository_path == canonical)
            {
                r["publicationProvider"] = json!({"state":"unavailable","host":unavailable.host,"reason":unavailable.reason});
            }
        }
    }
    v["activity"] = json!(activity);
    v["capabilities"]["execution"] =
        json!(e.workflow.is_some() && e.ready.load(std::sync::atomic::Ordering::SeqCst));
    v["capabilities"]["publication"] = json!(!e.publication_paths.is_empty());
    v["capabilities"]["liveJira"] = json!(e.sync.is_some());
    v["connections"]["runner"] =
        json!(e.workflow.is_some() && e.ready.load(std::sync::atomic::Ordering::SeqCst));
    let jira = e.sync.as_ref().map(|j| j.snapshot()).transpose()?;
    v["connections"]["jira"] = json!(jira.as_ref().is_some_and(|j| j["state"] == "connected"));
    if let Some(jira) = jira {
        v["jira"] = jira;
    }
    v["jiraUpdates"] = json!(e.store.with_store(|s| s.jira_updates(None))?);
    v["githubUpdates"] = json!(e.store.with_store(|s| s.github_updates(None))?);
    v["needsYou"] = json!(needs::needs_you(v, &runs));
    Ok(())
}
pub(super) fn after(q: &HashMap<String, String>) -> Result<u64> {
    q.get("after")
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| Error::Invalid("Invalid event page.".into()))
        })
        .unwrap_or(Ok(0))
}
pub(super) fn limit(q: &HashMap<String, String>) -> Result<usize> {
    let value = q
        .get("limit")
        .map(|v| {
            v.parse::<usize>()
                .map_err(|_| Error::Invalid("Invalid event page.".into()))
        })
        .unwrap_or(Ok(100))?;
    if !(1..=200).contains(&value) {
        return Err(Error::Invalid("Invalid event page.".into()));
    }
    Ok(value)
}
pub(super) async fn read(
    e: &Execution,
    path: &str,
    query: &HashMap<String, String>,
) -> Result<Option<Value>> {
    let parts: Vec<_> = path.split('/').collect();
    Ok(Some(match parts.as_slice() {
        ["runs", id, "attempts", attempt, "terminal"] => {
            e.workflow
                .as_ref()
                .ok_or_else(|| {
                    Error::Conflict(
                        "Native terminal observation is unavailable without a configured runner."
                            .into(),
                    )
                })?
                .terminal(id, attempt)
                .await?
        }
        ["runs", id, "runtime"] => {
            let mut v = if let Some(w) = &e.workflow {
                w.snapshot(id)?
            } else {
                e.store.with_store(|s| s.runtime(id))?
            };
            v["busy"] = json!(e.workflow.as_ref().is_some_and(|w| w.busy(id)));
            e.store.with_store(|s| {
                v["run"] = s.get_run(id)?;
                v["teamProposals"] = json!(s.team_proposals(id)?);
                v["commands"] = commands(&s.command_events(id)?);
                v["history"] = history::read(s, id)?;
                Ok(())
            })?;
            v
        }
        ["runs", id, "events"] => e
            .store
            .with_store(|s| s.agent_event_page(id, after(query)?, limit(query)?))?,
        ["runs", id, "repository"] => e.work.status(id).await?,
        ["runs", id, "research"] => e.research.read(id)?,
        ["runs", id, "coding"] => {
            let record = e.store.with_store(|s| s.coding_record(id))?;
            let assessment = if record.is_some() {
                e.evidence.assess(id).await?
            } else {
                json!({"ready":false,"blockers":["Repository work has not been prepared."],"confirmedInvalidation":false})
            };
            json!({"record":record,"assessment":assessment,"checksAvailable":e.checks})
        }
        ["runs", id, "feedback"] => {
            e.store.with_store(|s| s.get_run(id))?;
            let v = e
                .publication
                .observe_feedback(id)
                .await?
                .unwrap_or(Value::Null);
            e.changed();
            v
        }
        ["runs", id, "result"] => result(e, id).await?,
        ["runs", id, "result", "artifacts", artifact] => artifact_value(e, id, artifact)?,
        ["runs", id, "result", "candidates", candidate, "diff"] => {
            let record = e
                .store
                .with_store(|s| s.coding_record(id))?
                .unwrap_or(Value::Null);
            let candidate = array(&record["candidates"])
                .iter()
                .find(|c| c["id"] == *candidate)
                .ok_or_else(|| Error::NotFound("Candidate is not admitted to this run.".into()))?;
            e.work.diff_revision(id, &candidate["revision"]).await?
        }
        _ => return Ok(None),
    }))
}
fn artifact_value(e: &Execution, id: &str, artifact: &str) -> Result<Value> {
    let (record, research, choreography) = e.store.with_store(|s| {
        Ok((
            s.coding_record(id)?.unwrap_or(Value::Null),
            s.research_artifacts(id)?,
            s.choreography(id)?.unwrap_or(Value::Null),
        ))
    })?;
    let found = research
        .iter()
        .map(|r| &r["body"])
        .chain(
            array(&record["checks"])
                .iter()
                .flat_map(|c| [&c["output"], &c["source"]]),
        )
        .chain(array(&record["reviews"]).iter().map(|r| &r["artifact"]))
        .chain(
            array(&choreography["reports"])
                .iter()
                .flat_map(|r| array(&r["snapshots"])),
        )
        .find(|a| a["id"] == artifact);
    let mut value = found
        .ok_or_else(|| Error::NotFound("Evidence artifact is not admitted to this run.".into()))?
        .clone();
    value["content"] = json!(e.evidence.read_artifact(&value)?);
    Ok(value)
}
async fn result(e: &Execution, id: &str) -> Result<Value> {
    let (run, runtime, record, repository, publication, usage) = e.store.with_store(|s| {
        Ok((
            s.get_run(id)?,
            s.runtime(id)?,
            s.coding_record(id)?.unwrap_or(Value::Null),
            s.repository_work(id)?.unwrap_or(Value::Null),
            s.publication(id)?.unwrap_or(Value::Null),
            history::usage(s, id)?,
        ))
    })?;
    let research = run["purpose"] == "research";
    let mut value = json!({"configured":!record.is_null(),"criteria":run["taskSnapshot"]["criteria"],"refreshable":false,"usage":usage,
        "assignments":array(&runtime["assignments"]).iter().map(|a|{let mut v=json!({"id":a["id"],"name":a["name"],"model":a["model"]["resolved"]["selector"]});if let Some(at)=a.get("retiredAt"){v["retiredAt"]=at.clone();}v}).collect::<Vec<_>>(),
        "attempts":array(&runtime["attempts"]).iter().map(|a|json!({"id":a["id"],"assignmentId":a["assignmentId"],"sessionId":a["sessionId"],"model":history::model(&runtime,a)})).collect::<Vec<_>>()});
    for key in [
        "candidates",
        "checks",
        "reviews",
        "requiredChecks",
        "checkOperations",
        "checkPolicyChanges",
        "completions",
    ] {
        value[key] = json!(array(&record[key]));
    }
    value["refreshes"] = json!(array(&repository["refreshes"]));
    if research {
        let outcome = e.research.read(id)?;
        value["criteria"] = outcome["criteria"].clone();
        value["research"] = outcome;
        value["configured"] = json!(false);
        value["assessment"] = json!({"ready":false,"blockers":[],"confirmedInvalidation":false});
        value["publication"] =
            json!({"available":false,"reason":"Research has no PR publication."});
        return Ok(value);
    }
    // Queued and unprepared runs still expose usage and retained evidence.
    // Only prepared work can carry a managed Git conflict scope.
    let unresolved = if repository.is_null() {
        None
    } else {
        e.work.unresolved_conflict(id)?
    };
    let retry = unresolved.as_ref().is_some_and(|u| {
        u["state"] == "conflicted"
            && !present(&u["supersededBy"])
            && (!present(&u["resolutionWorkspace"])
                || u["resolutionWorkspace"]["state"] == "operator_required")
    });
    let stopped = !present(&repository["writer"])
        && array(&runtime["attempts"])
            .iter()
            .all(|a| a["state"] == "admitted" || a["ownership"] == "stopped");
    value["checkEnvironmentEditable"] = json!(
        !record.is_null()
            && run["status"] == "paused"
            && run["desiredState"] == "paused"
            && stopped
            && publication.is_null()
            && array(&record["checkOperations"])
                .iter()
                .all(|o| ["recorded", "rejected"].contains(&o["state"].as_str().unwrap_or("")))
    );
    let busy = ["push", "pr"].iter().any(|k| {
        ["sending", "uncertain", "blocked"]
            .contains(&publication[k]["state"].as_str().unwrap_or(""))
    });
    value["refreshable"] = json!(
        !repository.is_null()
            && !array(&record["candidates"]).is_empty()
            && ["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
            && stopped
            && !array(&record["completions"])
                .iter()
                .any(|c| c["state"] == "requested")
            && !busy
            && !array(&repository["refreshes"])
                .iter()
                .any(|r| r["state"] == "rebasing"
                    || ["preparing", "continuing"]
                        .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or("")))
            && (unresolved.is_none() || retry)
    );
    if let Some(r) = repository.get("repository") {
        value["repository"] = r.clone();
    }
    value["feedbackContinuations"] = json!(array(&run["feedbackContinuations"]));
    value["assessment"] = if record.is_null() {
        json!({"ready":false,"blockers":["Repository work has not been prepared."],"confirmedInvalidation":false})
    } else {
        e.evidence.assess(id).await?
    };
    value["publication"] = e.publication.view(id).await?;
    Ok(value)
}
