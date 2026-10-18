//! Local dependency decisions and provider-observed delivery are separate facts.
use crate::{
    Error, Result,
    store::{Store, conflict, integer, invalid, request_id, strict},
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(crate) fn coding(run: &Value) -> bool {
    run.get("purpose").is_none_or(|p| p == "coding")
}

pub(crate) fn delivery(task: &Value, runs: &[Value], publications: &[Value]) -> Value {
    let runs: Vec<_> = runs
        .iter()
        .filter(|r| r["taskId"] == task["id"] && coding(r))
        .collect();
    let key = task["key"].as_str().unwrap_or("");
    let state = |state: &str, reason: String| json!({"state":state,"reason":reason});
    if runs.is_empty() {
        return state("none", format!("{key}: no coding run or PR yet"));
    }
    let records: Vec<_> = runs
        .iter()
        .filter_map(|r| publications.iter().find(|p| p["runId"] == r["id"]))
        .collect();
    if let Some(merged) = records
        .iter()
        .find(|p| p.get("merged").is_some_and(|v| !v.is_null()))
    {
        return json!({"state":"merged","merge":merged["merged"]["commit"]});
    }
    let Some(latest) = records.last() else {
        return state(
            "none",
            format!(
                "{key}: coding run has no PR yet; a local branch or worktree cannot release it"
            ),
        );
    };
    match latest["pr"]["providerState"].as_str() {
        Some("DECLINED" | "SUPERSEDED") => {
            state("closed", format!("{key}: PR closed without merge"))
        }
        Some("OPEN") => state(
            "open",
            format!(
                "{key}: PR open{}, but not merged",
                if latest["ready"] == true {
                    " and ready for review"
                } else {
                    " or CI pending"
                }
            ),
        ),
        _ if latest["pr"]["state"] == "pending"
            && ["pending", "confirmed"]
                .contains(&latest["push"]["state"].as_str().unwrap_or("")) =>
        {
            state(
                "none",
                format!(
                    "{key}: no delivered PR yet{}",
                    if latest["push"]["state"] == "pending" {
                        "; only a local branch or worktree"
                    } else {
                        ""
                    }
                ),
            )
        }
        _ => state(
            "unknown",
            format!("{key}: provider PR or merge state unknown or unavailable"),
        ),
    }
}

impl Store {
    pub fn set_dependencies(&self, task_id: &str, body: &Value) -> Result<Value> {
        strict(body, &["requestId", "dependencyRevision", "dependsOn"])?;
        let key = request_id(body)?;
        let revision = integer(body, "dependencyRevision", 0, u64::MAX)?;
        let dependencies = body["dependsOn"]
            .as_array()
            .filter(|a| a.len() <= 12)
            .ok_or_else(|| invalid("dependsOn must contain at most 12 ticket IDs."))?;
        let ids: Vec<_> = dependencies
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| crate::store::valid_uuid(s))
                    .ok_or_else(|| invalid("Dependency IDs must be UUIDs."))
            })
            .collect::<Result<_>>()?;
        let payload =
            json!({"taskId":task_id,"dependencyRevision":revision,"dependsOn":dependencies});
        self.transaction(|s| {
            if let Some(prior) = s.replay(key, "dependencies", &payload)? {
                return Ok(prior);
            }
            let task = s.get_task(task_id)?;
            crate::store::operator_task(&task)?;
            if task["dependencyRevision"].as_u64().unwrap_or(0) != revision {
                return Err(conflict(
                    "Ticket dependencies changed. Reload before editing dependencies.",
                ));
            }
            if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
                return Err(conflict("Dependencies must be distinct."));
            }
            if ids.contains(&task_id) {
                return Err(conflict("A ticket cannot depend on itself."));
            }
            let tasks = s.tasks()?;
            let tasks: HashMap<_, _> = tasks
                .iter()
                .map(|t| (t["id"].as_str().unwrap(), t))
                .collect();
            for id in &ids {
                if !tasks.contains_key(id) {
                    return Err(Error::NotFound("Dependency ticket not found.".into()));
                }
                // Iterative traversal also bounds stack use for long imported graphs.
                let mut pending = vec![*id];
                let mut seen = HashSet::new();
                while let Some(next) = pending.pop() {
                    if next == task_id {
                        return Err(conflict("Ticket dependencies would form a cycle."));
                    }
                    if seen.insert(next)
                        && let Some(edges) = tasks.get(next).and_then(|t| t["dependsOn"].as_array())
                    {
                        pending.extend(edges.iter().filter_map(Value::as_str));
                    }
                }
            }
            let mut local = s.refinement(task_id)?;
            local["dependsOn"] = json!(dependencies);
            local["dependencyRevision"] = json!(revision + 1);
            s.save_refinement(task_id, &local)?;
            let saved = s.get_task(task_id)?;
            s.receipt(key, "dependencies", &payload, &saved)?;
            Ok(saved)
        })
    }

    pub fn dependency_states(&self) -> Result<Value> {
        let runs = self.all("runs")?;
        let publications = self.all("publications")?;
        let mut result = json!({});
        for task in self.tasks()? {
            let mut state = delivery(&task, &runs, &publications);
            crate::store::remove(&mut state, "merge");
            result[task["id"].as_str().unwrap()] = state;
        }
        Ok(result)
    }

    pub(crate) fn dependency_hold(&self, task: &Value) -> Result<Option<String>> {
        let Some(ids) = task["dependsOn"].as_array() else {
            return Ok(None);
        };
        let tasks = self.tasks()?;
        let runs = self.all("runs")?;
        let publications = self.all("publications")?;
        for id in ids {
            let Some(dependency) = tasks.iter().find(|t| &t["id"] == id) else {
                return Ok(Some(format!(
                    "Dependency {} is unavailable",
                    id.as_str().unwrap_or("")
                )));
            };
            if let Some(reason) = delivery(dependency, &runs, &publications)["reason"].as_str() {
                return Ok(Some(reason.into()));
            }
        }
        Ok(None)
    }
}
