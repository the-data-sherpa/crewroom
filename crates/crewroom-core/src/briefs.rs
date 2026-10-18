//! Snapshot accepted criteria and validate obligations across the complete team.
use crate::{
    Result,
    store::{Store, conflict, invalid},
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub fn snapshot_briefs(
    requests: &[Value],
    criteria: &Value,
    checks: &[Value],
    existing: &[Value],
) -> Result<Value> {
    let names: HashSet<_> = existing
        .iter()
        .chain(requests)
        .filter_map(|v| v["name"].as_str())
        .collect();
    let mut briefs = json!({});
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let dependencies = |v: &Value| -> Result<Vec<String>> {
        v.as_array()
            .ok_or_else(|| invalid("Brief dependencies must be a list."))?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("Brief dependencies must be names."))
            })
            .collect()
    };
    for item in existing {
        edges.insert(
            item["name"].as_str().unwrap_or("").into(),
            if item["brief"]["dependsOn"].is_array() {
                dependencies(&item["brief"]["dependsOn"])?
            } else {
                vec![]
            },
        );
    }
    for request in requests {
        let name = request["name"]
            .as_str()
            .ok_or_else(|| invalid("A brief needs an assignment name."))?;
        let obligations = request["criteria"]
            .as_array()
            .ok_or_else(|| invalid("Brief criteria must be a list."))?;
        let mut seen = HashSet::new();
        let mut snapshot = Vec::new();
        for item in obligations {
            let index = item["index"]
                .as_u64()
                .ok_or_else(|| invalid("Criterion index must be nonnegative."))?;
            if !seen.insert(index) {
                return Err(conflict(&format!(
                    "{name}: each accepted criterion may appear once in a brief."
                )));
            }
            let criterion = criteria
                .as_array()
                .and_then(|a| a.get(index as usize))
                .ok_or_else(|| {
                    conflict(&format!(
                        "{name}: criterion {index} is not an accepted criterion."
                    ))
                })?;
            let ids = item["checkIds"]
                .as_array()
                .ok_or_else(|| invalid("Check IDs must be a list."))?;
            let mut unique = Vec::new();
            for check in ids {
                if !checks.contains(check) {
                    return Err(conflict(&format!(
                        "{name}: check {} is not configured for this repository.",
                        check.as_str().unwrap_or("")
                    )));
                }
                if !unique.contains(check) {
                    unique.push(check.clone());
                }
            }
            snapshot.push(json!({"index":index,"criterion":criterion,"obligation":item["obligation"],"checkIds":unique}));
        }
        let mut deps = Vec::new();
        for dep in dependencies(&request["dependsOn"])? {
            if dep == name {
                return Err(conflict(&format!(
                    "{name}: an assignment cannot depend on itself."
                )));
            }
            if !names.contains(dep.as_str()) {
                return Err(conflict(&format!(
                    "{name}: dependency {dep} is not an assignment in this team."
                )));
            }
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
        briefs[name] = json!({"criteria":snapshot,"dependsOn":deps});
        edges.insert(name.into(), deps);
    }
    for name in edges.keys() {
        let mut pending = edges[name].clone();
        let mut seen = HashSet::new();
        while let Some(next) = pending.pop() {
            if &next == name {
                return Err(conflict(&format!(
                    "Assignment dependencies form a cycle through {name}."
                )));
            }
            if seen.insert(next.clone())
                && let Some(deps) = edges.get(&next)
            {
                pending.extend(deps.iter().cloned());
            }
        }
    }
    Ok(briefs)
}
impl Store {
    pub fn configured_check_ids(&self, run_id: &str) -> Result<Vec<Value>> {
        let run = self.get_run(run_id)?;
        if !crate::dependencies::coding(&run) {
            return Ok(vec![]);
        }
        let repository = if let Some(work) = self.record("repository_work", "run_id", run_id)? {
            Some(work["repository"].clone())
        } else if let Some(id) = run["taskSnapshot"]["repositoryId"].as_str() {
            self.record("repositories", "id", id)?
        } else {
            None
        };
        Ok(repository
            .and_then(|r| {
                r["checks"]
                    .as_array()
                    .map(|a| a.iter().map(|c| c["id"].clone()).collect())
            })
            .unwrap_or_default())
    }
    pub fn validate_insertion_team(&self, run_id: &str, team: &[Value]) -> Result<()> {
        let pending: Vec<_> = self
            .run_records("team_proposals", run_id)?
            .into_iter()
            .filter(|p| p["origin"] == "operator" && p["status"] == "pending")
            .flat_map(|p| p["specialists"].as_array().cloned().unwrap_or_default())
            .collect();
        if team.iter().filter(|a| a["coordinator"] != true).count() + pending.len() > 3 {
            return Err(conflict(
                "The specialist concurrency limit includes pending insertions.",
            ));
        }
        let names: HashSet<_> = team
            .iter()
            .chain(&pending)
            .map(|a| a["name"].clone().to_string())
            .collect();
        if names.len() != team.len() + pending.len() {
            return Err(conflict("Assignment names must be distinct."));
        }
        let requests: Vec<_> = pending.iter().map(|p| json!({"name":p["name"],"criteria":p["brief"]["criteria"],"dependsOn":p["brief"]["dependsOn"]})).collect();
        snapshot_briefs(
            &requests,
            &self.get_run(run_id)?["taskSnapshot"]["criteria"],
            &self.configured_check_ids(run_id)?,
            team,
        )?;
        Ok(())
    }
}
