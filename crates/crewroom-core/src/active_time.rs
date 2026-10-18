//! Provider-confirmed CI waiting stops the budget only after all local work settles.
use crate::{Result, lifecycle::Transition, store::Store};
use serde_json::Value;
use std::collections::HashMap;

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn nonnull(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|v| !v.is_null() && v != false)
}

pub(crate) fn hosted_ci_pending(record: &Value) -> bool {
    if !nonnull(record, "observation")
        || record["ready"] == true
        || nonnull(record, "error")
        || nonnull(record, "merged")
        || record["push"]["state"] != "confirmed"
        || record["pr"]["state"] != "confirmed"
        || record["remoteSourceHead"] != record["revision"]["head"]
        || record["remoteTargetHead"] != record["revision"]["base"]
    {
        return false;
    }
    let observed = &record["observation"];
    let pr = &observed["pr"];
    if pr["state"] != "OPEN"
        || record["pr"]["providerState"] != "OPEN"
        || pr["id"] != record["pr"]["id"]
        || pr["source"]["commit"]["hash"] != record["revision"]["head"]
        || pr["destination"]["commit"]["hash"] != record["revision"]["base"]
        || pr["source"]["branch"]["name"] != record["branch"]
        || pr["destination"]["branch"]["name"] != record["target"]
    {
        return false;
    }
    let pipeline = array(&observed["pipelines"]).first();
    if let Some(p) = pipeline {
        if p["target"]["commit"]["hash"] != record["revision"]["head"]
            || p["target"]["destination_commit"]["hash"] != record["revision"]["base"]
            || p["target"]["pullrequest"]["id"] != record["pr"]["id"]
        {
            return false;
        }
        if !["PENDING", "IN_PROGRESS"].contains(&p["state"]["name"].as_str().unwrap_or(""))
            && !(p["state"]["name"] == "COMPLETED" && p["state"]["result"]["name"] == "SUCCESSFUL")
        {
            return false;
        }
        if array(&p["steps"]).iter().any(|step| {
            let required = array(&record["policy"]["pipelineSteps"]).contains(&step["name"]);
            (!required && !array(&record["policy"]["optionalSteps"]).contains(&step["name"]))
                || (required
                    && step["state"]["name"] == "COMPLETED"
                    && step["state"]["result"]["name"] != "SUCCESSFUL")
        }) {
            return false;
        }
    }
    let statuses: Vec<_> = array(&record["policy"]["statusKeys"])
        .iter()
        .map(|key| {
            array(&observed["statuses"])
                .iter()
                .find(|s| &s["key"] == key)
        })
        .collect();
    if statuses.iter().flatten().any(|s| {
        !["SUCCESSFUL", "INPROGRESS"].contains(&s["state"].as_str().unwrap_or(""))
            || (s["refname"].as_str().is_some_and(|v| !v.is_empty())
                && s["refname"] != record["branch"])
    }) {
        return false;
    }
    pipeline.is_some_and(|p| {
        ["PENDING", "IN_PROGRESS"].contains(&p["state"]["name"].as_str().unwrap_or(""))
    }) || statuses
        .iter()
        .flatten()
        .any(|s| s["state"] == "INPROGRESS")
}
impl Store {
    pub fn account_time(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| s.account_time_in_transaction(run_id))
    }
    pub(crate) fn account_time_in_transaction(&self, run_id: &str) -> Result<Value> {
        let run = self.get_run(run_id)?;
        let mut waiting = false;
        if run["status"] == "running"
            && !nonnull(&run, "sourceConflict")
            && !nonnull(&run, "codingTransition")
            && !nonnull(&run, "teamTransition")
            && self
                .record("publications", "run_id", run_id)?
                .is_some_and(|p| hosted_ci_pending(&p))
        {
            let runtime = self.runtime(run_id)?;
            let attempts = array(&runtime["attempts"]);
            let current: Vec<_> = array(&runtime["assignments"])
                .iter()
                .filter(|a| a.get("retiredAt").is_none())
                .filter_map(|a| attempts.iter().find(|p| p["id"] == a["currentAttemptId"]))
                .collect();
            let mut commands = HashMap::new();
            for e in self.command_events(run_id)? {
                if attempts
                    .iter()
                    .any(|a| a["id"] == e["attemptId"] && a["ownership"] == "stopped")
                {
                    continue;
                }
                let event = &e["event"];
                let key = (e["attemptId"].to_string(), event["toolCallId"].to_string());
                if ["tool_started", "tool_receipt_reused"]
                    .contains(&event["type"].as_str().unwrap_or(""))
                {
                    commands.insert(key.clone(), true);
                }
                if event["type"] == "tool_exited" && event["ownership"]["state"] == "stopped" {
                    commands.insert(key, false);
                }
            }
            let mut settled = !current.is_empty();
            for attempt in &current {
                settled &= attempt["ownership"] == "stopped"
                    || (attempt["ownership"] == "held"
                        && attempt["state"] == "idle"
                        && self
                            .has_agent_event(attempt["id"].as_str().unwrap(), "session_settled")?);
            }
            waiting = settled
                && attempts.iter().all(|a| {
                    a["ownership"] == "stopped"
                        || a["state"] == "admitted"
                        || current.iter().any(|c| {
                            c["id"] == a["id"] && c["ownership"] == "held" && c["state"] == "idle"
                        })
                })
                && !commands.values().any(|v| *v)
                && array(&runtime["messages"])
                    .iter()
                    .all(|m| m["disposition"] == "received")
                && !self.run_records("steering", run_id)?.iter().any(|m| {
                    ["pending", "dispatched"].contains(&m["delivery"].as_str().unwrap_or(""))
                })
                && !self
                    .run_records("delivery_notices", run_id)?
                    .iter()
                    .any(|n| n["disposition"] != "received")
                && !self
                    .run_records("team_proposals", run_id)?
                    .iter()
                    .any(|p| p["status"] == "pending");
        }
        self.change_run(run_id, Transition::Account { waiting })
    }
}
