//! Advisory run checklists. They never grant evidence, ownership, or readiness.
use crate::{
    Result,
    authority::write_private,
    roles::Roles,
    store::{Store, conflict, id, integer, invalid, request_id, strict, trimmed_text},
};
use serde_json::{Value, json};
use std::{collections::HashSet, path::PathBuf};
pub struct Playbooks {
    path: PathBuf,
}
impl Playbooks {
    pub fn beside_roles(roles: &Roles) -> Self {
        Self {
            path: roles.path().with_file_name("playbooks.json"),
        }
    }
    pub fn read(&self) -> Result<Vec<Value>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&self.path)?)?;
        if rows.len() > 100 {
            return Err(invalid("At most 100 playbooks are supported."));
        }
        let mut seen = HashSet::new();
        rows.iter()
            .map(|r| {
                let r = validate(r)?;
                if !seen.insert(r["id"].clone()) {
                    return Err(invalid("Playbook IDs must be unique."));
                }
                Ok(r)
            })
            .collect()
    }
    pub fn save(&self, input: &Value) -> Result<Value> {
        let mut next = validate(input)?;
        let mut rows = self.read()?;
        if let Some(current) = rows.iter_mut().find(|r| r["id"] == next["id"]) {
            if current["revision"] != next["revision"] {
                return Err(conflict(
                    "Playbook changed. Reload before saving; your draft is retained.",
                ));
            }
            next["revision"] = json!(
                next["revision"]
                    .as_u64()
                    .unwrap()
                    .checked_add(1)
                    .ok_or_else(|| invalid("Playbook revision exhausted."))?
            );
            *current = next.clone();
        } else {
            if next["revision"] != 0 {
                return Err(conflict(
                    "Playbook no longer exists. Create a new playbook.",
                ));
            }
            if rows.len() >= 100 {
                return Err(invalid("At most 100 playbooks are supported."));
            }
            next["revision"] = json!(1);
            rows.push(next.clone());
        }
        write_private(&self.path, &serde_json::to_vec_pretty(&rows)?)?;
        Ok(next)
    }
}
fn validate(v: &Value) -> Result<Value> {
    let id = trimmed_text(v, "id", 1, 64)?;
    if !id
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || !id.as_bytes()[0].is_ascii_lowercase()
    {
        return Err(invalid("Invalid playbook ID."));
    }
    let name = trimmed_text(v, "name", 1, 100)?;
    let description = trimmed_text(v, "description", 1, 2000)?;
    let revision = integer(v, "revision", 0, u64::MAX)?;
    let steps = v["steps"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| invalid("A playbook needs 1–50 steps."))?;
    let mut seen = HashSet::new();
    let steps = steps
        .iter()
        .map(|s| {
            let id = trimmed_text(s, "id", 1, 64)?;
            let title = trimmed_text(s, "title", 1, 1000)?;
            if !seen.insert(id.to_owned()) {
                return Err(invalid("Step IDs must be unique."));
            }
            Ok(json!({"id":id,"title":title}))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"id":id,"name":name,"description":description,"revision":revision,"steps":steps}))
}
fn copy(value: &Value, origin: &str, version: u64) -> Value {
    let mut v = value.clone();
    v["origin"] = json!(origin);
    v["selectionVersion"] = json!(version);
    for step in v["steps"].as_array_mut().unwrap() {
        step["state"] = json!("pending");
    }
    v
}
pub(crate) fn selected(catalog: &[Value], id: &str) -> Result<Value> {
    catalog
        .iter()
        .find(|p| p["id"] == id)
        .cloned()
        .ok_or_else(|| conflict("Playbook is unavailable. Reload the catalog."))
}
pub(crate) fn initial(run: &mut Value, playbook: Value) {
    run["playbook"] = copy(&playbook, "operator", 1);
    run["playbookVersion"] = json!(1);
}
fn events(run: &Value) -> Vec<Value> {
    run["playbookEvents"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
fn replay(run: &Value, key: &Value, input: &Value) -> Result<Option<Value>> {
    if let Some(event) = events(run).iter().find(|e| e["key"] == *key) {
        if event["input"] != *input {
            return Err(conflict("Playbook request identity has different content."));
        }
        return Ok(Some(event["result"].clone()));
    }
    Ok(None)
}
impl Store {
    fn save_playbook_event(
        &self,
        run: &mut Value,
        key: Value,
        input: &Value,
        result: &Value,
    ) -> Result<Value> {
        let mut rows = events(run);
        rows.push(json!({"id":id(),"at":self.time(),"key":key,"input":input,"result":result}));
        run["playbookEvents"] = json!(rows);
        self.save_run(run)?;
        Ok(result.clone())
    }
    /// Operator selection is atomic with its steering receipt. Queued runs need no message.
    pub fn select_playbook(&self, run_id: &str, input: &Value, catalog: &[Value]) -> Result<Value> {
        strict(
            input,
            &[
                "playbookId",
                "proposalId",
                "revision",
                "reason",
                "requestId",
            ],
        )?;
        let request = request_id(input)?;
        let revision = integer(input, "revision", 0, u64::MAX)?;
        let reason = trimmed_text(input, "reason", 1, 2000)?;
        if input.get("playbookId").is_some() == input.get("proposalId").is_some() {
            return Err(invalid("Choose a playbook or a recorded proposal."));
        }
        self.transaction(|s|{
            let mut run=s.get_run(run_id)?;let key=json!({"operator":request});
            if run["purpose"]=="evaluation" {return Err(conflict("An evaluation keeps its frozen playbook."));}
            if let Some(result)=replay(&run,&key,input)? {return Ok(result);}
            if let Some(reason)=crate::messages::closed_reason(&run) {return Err(conflict(reason));}
            if run["roleDraft"].is_object() || matches!(run["purpose"].as_str(),Some("explore"|"grill")) {return Err(conflict("Playbooks apply to delivery and research runs."));}
            if run["playbookVersion"].as_u64().unwrap_or(0)!=revision {return Err(conflict("Run playbook changed. Reload before selecting."));}
            let version=revision.checked_add(1).ok_or_else(||invalid("Playbook version exhausted."))?;
            let proposal=input["proposalId"].as_str();
            let value=if let Some(proposal)=proposal {
                if run["playbookProposal"]["id"]!=proposal || run["playbookProposal"]["status"]!="pending" {return Err(conflict("The playbook proposal changed or was already decided."));}
                run["playbookProposal"]["status"]=json!("accepted");
                run["playbookProposal"]["playbook"].clone()
            }else{
                let id=input["playbookId"].as_str().ok_or_else(||invalid("Invalid playbook selection."))?;
                if id.is_empty() {Value::Null}else{selected(catalog,id)?}
            };
            run["playbookVersion"]=json!(version);
            run["playbook"]=if value.is_null(){Value::Null}else{copy(&value,"operator",version)};
            let mut result=json!({"playbook":run["playbook"],"version":version});
            if !s.run_records("assignments",run_id)?.is_empty() {
                let body=format!("Operator changed the advisory playbook to selection version {version}. Reason: {reason}. Selected playbook: {}. Call playbook_status to inspect current state. This does not change requirements or readiness gates.",run["playbook"]["name"].as_str().unwrap_or("none"));
                let message=s.steer(run_id,&json!({"requestId":format!("playbook-{request}"),"body":body}))?;
                result["steeringId"]=message["id"].clone();run["playbookSteeringId"]=message["id"].clone();
            }
            s.save_playbook_event(&mut run,key,input,&result)
        })
    }
    fn playbook_caller(&self, attempt: &str) -> Result<Value> {
        let attempt = self.get_attempt(attempt)?;
        let run = self.get_run(attempt["runId"].as_str().unwrap())?;
        if run["sourceConflict"].is_object()
            || matches!(run["purpose"].as_str(), Some("explore" | "grill"))
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || attempt["ownership"] != "held"
            || !self
                .run_records("assignments", run["id"].as_str().unwrap())?
                .iter()
                .any(|a| {
                    a["coordinator"] == true
                        && a["currentAttemptId"] == attempt["id"]
                        && a["retiredAt"].is_null()
                })
        {
            return Err(conflict(
                "Only the current active coordinator may update a playbook.",
            ));
        }
        Ok(run)
    }
    pub fn propose_playbook(
        &self,
        attempt: &str,
        call: &str,
        input: &Value,
        catalog: &[Value],
    ) -> Result<Value> {
        strict(input, &["playbookId", "rationale"])?;
        let pid = trimmed_text(input, "playbookId", 1, 64)?;
        trimmed_text(input, "rationale", 1, 4000)?;
        self.transaction(|s|{
            let mut run=s.playbook_caller(attempt)?;let key=json!({"attempt":attempt,"call":call});
            if run["purpose"]=="evaluation" {return Err(conflict("An evaluation cannot propose a different playbook."));}
            if let Some(result)=replay(&run,&key,input)? {return Ok(result);}
            let value=selected(catalog,pid)?;let result=json!({"id":id(),"origin":"coordinator","attemptId":attempt,"toolCallId":call,"status":"pending","playbook":value,"rationale":input["rationale"],"at":s.time()});
            run["playbookProposal"]=result.clone();s.save_playbook_event(&mut run,key,input,&result)
        })
    }
    pub fn record_playbook_step(&self, attempt: &str, call: &str, input: &Value) -> Result<Value> {
        strict(input, &["version", "stepId", "state", "reason"])?;
        let version = integer(input, "version", 1, u64::MAX)?;
        let step = trimmed_text(input, "stepId", 1, 64)?;
        let state = trimmed_text(input, "state", 1, 20)?;
        if !["pending", "active", "done", "skipped"].contains(&state) {
            return Err(invalid("Invalid checklist state."));
        }
        let reason = input
            .get("reason")
            .map(|_| trimmed_text(input, "reason", 1, 2000))
            .transpose()?;
        if state == "skipped" && reason.is_none() {
            return Err(invalid("A skipped step needs a reason."));
        }
        self.transaction(|s| {
            let mut run = s.playbook_caller(attempt)?;
            let key = json!({"attempt":attempt,"call":call});
            if let Some(result) = replay(&run, &key, input)? {
                return Ok(result);
            }
            if run["playbookVersion"] != version {
                return Err(conflict(
                    "Playbook selection changed. Read playbook_status before reporting progress.",
                ));
            }
            let steps = run["playbook"]["steps"]
                .as_array_mut()
                .ok_or_else(|| conflict("No playbook is selected."))?;
            let item = steps
                .iter_mut()
                .find(|v| v["id"] == step)
                .ok_or_else(|| conflict("Unknown step in the copied checklist."))?;
            item["state"] = json!(state);
            item["reason"] = json!(reason);
            item["updatedAt"] = json!(s.time());
            let result = item.clone();
            s.save_playbook_event(&mut run, key, input, &result)
        })
    }
}
