//! Operator-decided learning with durable approval before any role-catalog write.
use crate::{
    Error, Result,
    roles::Roles,
    store::{Store, conflict, id, invalid, remove, request_id, strict, trim_text},
};
use rusqlite::params;
use serde_json::{Value, json};

fn string(value: &Value, min: usize, max: usize, trim: bool) -> Result<&str> {
    let value = value.as_str().ok_or_else(|| invalid("Expected text."))?;
    let value = if trim { trim_text(value) } else { value };
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid("Text is outside its permitted size."));
    }
    Ok(value)
}
/// Operators re-propose from Agent roles; only coordinator proposals come from a run.
fn new_proposal_route(proposal: &Value) -> &'static str {
    if proposal["origin"] == "operator" {
        "Submit a new proposal from Agent roles."
    } else {
        "Ask the coordinator for a new proposal."
    }
}
fn same_serialized(left: &Value, right: &Value) -> bool {
    crate::serialized::same(left, right)
}
fn proposal_input(input: &Value) -> Result<Value> {
    strict(input, &["target", "change", "rationale", "evidence"])?;
    strict(&input["target"], &["kind", "roleId"])?;
    let kind = string(&input["target"]["kind"], 1, 20, false)?;
    if !["role", "project"].contains(&kind)
        || (kind == "role") != input["target"].get("roleId").is_some()
    {
        return Err(invalid(
            "A role target names roleId; a project target does not.",
        ));
    }
    let mut target = json!({"kind":kind});
    if kind == "role" {
        target["roleId"] = json!(string(&input["target"]["roleId"], 1, 64, false)?);
    }
    let change = string(&input["change"], 1, 12000, true)?;
    let rationale = string(&input["rationale"], 1, 4000, true)?;
    let evidence = input["evidence"]
        .as_array()
        .filter(|a| (1..=20).contains(&a.len()))
        .ok_or_else(|| invalid("Cite one to twenty recorded evidence items."))?;
    let mut citations = vec![];
    for item in evidence {
        strict(item, &["kind", "id"])?;
        let kind = string(&item["kind"], 1, 20, false)?;
        if !["check", "review", "steering", "check_policy"].contains(&kind) {
            return Err(invalid("Invalid learning evidence kind."));
        }
        citations.push(json!({"kind":kind,"id":string(&item["id"],1,200,false)?}));
    }
    Ok(json!({"target":target,"change":change,"rationale":rationale,"evidence":citations}))
}
impl Store {
    pub fn learning_proposals(&self) -> Result<Vec<Value>> {
        self.all("learning_proposals")
    }
    pub fn get_learning_proposal(&self, proposal_id: &str) -> Result<Value> {
        self.record("learning_proposals", "id", proposal_id)?
            .ok_or_else(|| Error::NotFound("Learning proposal not found.".into()))
    }
    fn save_learning_proposal(&self, proposal: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE learning_proposals SET data=? WHERE id=?",
            params![proposal.to_string(), proposal["id"].as_str()],
        )?;
        Ok(())
    }
    /// Trusted, validated proposal input from Learning; no role file changes here.
    pub fn propose_learning(&self, input: &Value) -> Result<Value> {
        self.transaction(|s| {
            if let Some(previous) = s.learning_proposals()?.into_iter().find(|p| {
                p["attemptId"] == input["attemptId"] && p["toolCallId"] == input["toolCallId"]
            }) {
                let identity = |v: &Value| {
                    json!([
                        v["target"],
                        v["change"],
                        v["baseVersion"],
                        v["rationale"],
                        v["evidence"]
                    ])
                };
                if !same_serialized(&identity(&previous), &identity(input)) {
                    return Err(conflict(
                        "Learning proposal identity has different content.",
                    ));
                }
                return Ok(previous);
            }
            let mut proposal = input.clone();
            proposal["id"] = json!(id());
            proposal["status"] = json!("pending");
            proposal["createdAt"] = json!(s.time());
            s.db.execute(
                "INSERT INTO learning_proposals VALUES(?,?,?,?,?)",
                params![
                    proposal["id"].as_str(),
                    proposal["runId"].as_str(),
                    proposal["attemptId"].as_str(),
                    proposal["toolCallId"].as_str(),
                    proposal.to_string()
                ],
            )?;
            Ok(proposal)
        })
    }
    pub fn decide_learning(&self, proposal_id: &str, decision: &Value) -> Result<Value> {
        self.transaction(|s| {
            let mut proposal = s.get_learning_proposal(proposal_id)?;
            if proposal["status"] != "pending" {
                if proposal["status"] == decision["status"]
                    && proposal["decision"]["requestId"] == decision["requestId"] {
                    return Ok(proposal);
                }
                return Err(conflict("This proposal has already been decided."));
            }
            proposal["status"] = decision["status"].clone();
            proposal["decision"] = json!({
                "requestId":decision["requestId"], "at":s.time(), "reason":decision["reason"]
            });
            if decision["pendingRoleSave"] == true {
                proposal["decision"]["pendingRoleSave"] = json!(true);
            }
            if decision["status"] == "approved" && proposal["target"]["kind"] == "project" {
                let repository_id = proposal["target"]["repositoryId"].as_str().unwrap();
                let mut repository = s.record("repositories", "id", repository_id)?
                    .ok_or_else(|| Error::NotFound("Repository not found.".into()))?;
                let mut guidance = repository["guidance"].as_array().cloned().unwrap_or_default();
                let current = guidance.last().and_then(|g| g["version"].as_u64()).unwrap_or(0);
                if proposal["baseVersion"] != current {
                    return Err(conflict("Project guidance changed since this proposal. Ask the coordinator for a new proposal."));
                }
                let version = current + 1;
                guidance.push(json!({
                    "version":version, "text":proposal["change"],
                    "approvedAt":proposal["decision"]["at"], "proposalId":proposal["id"]
                }));
                repository["guidance"] = json!(guidance);
                s.db.execute("UPDATE repositories SET data=? WHERE id=?",
                    params![repository.to_string(),repository_id])?;
                proposal["decision"]["appliedVersion"] = json!(version);
            }
            s.save_learning_proposal(&proposal)?;
            Ok(proposal)
        })
    }
    pub fn complete_learning_approval(&self, proposal_id: &str, outcome: &Value) -> Result<Value> {
        self.transaction(|s| {
            let mut proposal = s.get_learning_proposal(proposal_id)?;
            if proposal["decision"]["pendingRoleSave"] != true {
                return Ok(proposal);
            }
            remove(&mut proposal["decision"], "pendingRoleSave");
            if let Some(version) = outcome.get("appliedVersion") {
                proposal["decision"]["appliedVersion"] = version.clone();
            } else {
                proposal["status"] = json!("superseded");
                proposal["decision"]["notApplied"] = outcome["notApplied"].clone();
            }
            s.save_learning_proposal(&proposal)?;
            Ok(proposal)
        })
    }
}

/// Coordinator-authentication belongs to the supervisor; only decisions are operator APIs.
pub struct Learning<'a> {
    store: &'a Store,
    roles: &'a Roles,
}
impl<'a> Learning<'a> {
    pub fn new(store: &'a Store, roles: &'a Roles) -> Self {
        Self { store, roles }
    }
    pub fn propose_operator(&self, input: &Value) -> Result<Value> {
        strict(
            input,
            &[
                "roleId",
                "revision",
                "mode",
                "change",
                "rationale",
                "requestId",
            ],
        )?;
        let key = request_id(input)?;
        let role_id = string(&input["roleId"], 1, 64, false)?;
        let mode = string(&input["mode"], 1, 20, false)?;
        if !["ask", "teach"].contains(&mode) {
            return Err(invalid("Choose ask or teach."));
        }
        let change = string(&input["change"], 1, 12000, true)?;
        let rationale = string(&input["rationale"], 1, 4000, true)?;
        let revision = crate::store::integer(input, "revision", 1, u64::MAX)?;
        let value = json!({"origin":"operator","mode":mode,"runId":null,"attemptId":"operator","toolCallId":key,"target":{"kind":"role","roleId":role_id},"change":change,"baseVersion":revision,"rationale":rationale,"evidence":[{"kind":"operator_note","id":key,"text":rationale}]});
        if let Some(prior) = self
            .store
            .learning_proposals()?
            .iter()
            .find(|p| p["attemptId"] == "operator" && p["toolCallId"] == key)
        {
            if prior["mode"] != mode {
                return Err(conflict("Proposal identity has different content."));
            }
            return self.store.propose_learning(&value);
        }
        let role = self
            .roles
            .read()?
            .into_iter()
            .find(|r| r["id"] == role_id)
            .ok_or_else(|| conflict("Role not found."))?;
        if role["revision"].as_u64().unwrap_or(1) != revision {
            return Err(conflict("The role changed. Reload before proposing."));
        }
        if role["instructions"] == change {
            return Err(conflict(
                "The proposal does not change the role instructions.",
            ));
        }
        self.store.propose_learning(&value)
    }
    pub fn propose(&self, attempt_id: &str, tool_call_id: &str, input: &Value) -> Result<Value> {
        let value = proposal_input(input)?;
        string(&json!(tool_call_id), 1, 200, false)?;
        if let Some(saved) = self
            .store
            .learning_proposals()?
            .into_iter()
            .find(|p| p["attemptId"] == attempt_id && p["toolCallId"] == tool_call_id)
        {
            if saved["change"] != value["change"]
                || saved["rationale"] != value["rationale"]
                || !same_serialized(&saved["evidence"], &value["evidence"])
                || saved["target"]["kind"] != value["target"]["kind"]
                || (saved["target"]["kind"] == "role"
                    && saved["target"]["roleId"] != value["target"]["roleId"])
            {
                return Err(conflict(
                    "Learning proposal identity has different content.",
                ));
            }
            return Ok(saved);
        }
        let attempt = self.store.get_attempt(attempt_id)?;
        let run_id = attempt["runId"].as_str().unwrap();
        let run = self.store.get_run(run_id)?;
        let record = self
            .store
            .record("coding", "run_id", run_id)?
            .unwrap_or(Value::Null);
        let steering = self.store.steering(run_id)?;
        for item in value["evidence"].as_array().unwrap() {
            let recorded = match item["kind"].as_str().unwrap() {
                "check" => record["checks"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|r| r["id"] == item["id"])),
                "review" => record["reviews"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|r| r["id"] == item["id"])),
                "steering" => steering.iter().any(|m| m["id"] == item["id"]),
                _ => record["checkPolicyChanges"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|r| r["requestId"] == item["id"])),
            };
            if !recorded {
                return Err(conflict(&format!(
                    "Evidence {} {} is not recorded in this run.",
                    item["kind"].as_str().unwrap(),
                    item["id"].as_str().unwrap()
                )));
            }
        }
        let (target, base_version) = if value["target"]["kind"] == "role" {
            let role_id = value["target"]["roleId"].as_str().unwrap();
            let role = self
                .roles
                .read()?
                .into_iter()
                .find(|r| r["id"] == role_id)
                .ok_or_else(|| conflict("The proposed role does not exist."))?;
            if role["instructions"] == value["change"] {
                return Err(conflict(
                    "The proposal does not change the role instructions.",
                ));
            }
            (
                json!({"kind":"role","roleId":role_id}),
                role["revision"].as_u64().unwrap_or(1),
            )
        } else {
            let repository =
                if let Some(repository_id) = run["taskSnapshot"]["repositoryId"].as_str() {
                    self.store.record("repositories", "id", repository_id)?
                } else {
                    None
                }
                .ok_or_else(|| {
                    conflict("This run has no configured repository for project guidance.")
                })?;
            let latest = repository["guidance"].as_array().and_then(|a| a.last());
            if latest.is_some_and(|g| g["text"] == value["change"]) {
                return Err(conflict(
                    "The proposal does not change the project guidance.",
                ));
            }
            (
                json!({"kind":"project","repositoryId":repository["id"]}),
                latest.and_then(|g| g["version"].as_u64()).unwrap_or(0),
            )
        };
        self.store.propose_learning(&json!({"origin":"coordinator","runId":run_id,"attemptId":attempt_id,"toolCallId":tool_call_id,"target":target,"change":value["change"],"baseVersion":base_version,"rationale":value["rationale"],"evidence":value["evidence"]}))
    }
    pub fn decide(&self, proposal_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["decision", "reason", "requestId"])?;
        let request = request_id(input)?;
        let reason = string(&input["reason"], 1, 2000, true)?;
        let status = match input["decision"].as_str() {
            Some("approve") => "approved",
            Some("reject") => "rejected",
            _ => return Err(invalid("Decision must approve or reject.")),
        };
        let proposal = self.store.get_learning_proposal(proposal_id)?;
        if proposal["status"] == "approved"
            && proposal["decision"]["pendingRoleSave"] == true
            && status == "approved"
        {
            return self.apply_role(&proposal);
        }
        let mut decision = json!({"requestId":request,"reason":reason,"status":status});
        if proposal["target"]["kind"] == "role"
            && proposal["status"] == "pending"
            && status == "approved"
        {
            let role = self
                .roles
                .read()?
                .into_iter()
                .find(|r| r["id"] == proposal["target"]["roleId"])
                .ok_or_else(|| conflict("The proposed role no longer exists."))?;
            if role["revision"].as_u64().unwrap_or(1) != proposal["baseVersion"].as_u64().unwrap() {
                return Err(conflict(&format!(
                    "The role changed since this proposal. {}",
                    new_proposal_route(&proposal)
                )));
            }
            decision["pendingRoleSave"] = json!(true);
            return self.apply_role(&self.store.decide_learning(proposal_id, &decision)?);
        }
        let decided = self.store.decide_learning(proposal_id, &decision)?;
        if decided["decision"]["pendingRoleSave"] == true {
            self.apply_role(&decided)
        } else {
            Ok(decided)
        }
    }
    pub fn reconcile(&self) -> Result<()> {
        for proposal in self
            .store
            .learning_proposals()?
            .into_iter()
            .filter(|p| p["decision"]["pendingRoleSave"] == true)
        {
            self.apply_role(&proposal)?;
        }
        Ok(())
    }
    fn apply_role(&self, proposal: &Value) -> Result<Value> {
        if proposal["target"]["kind"] != "role" {
            return Ok(proposal.clone());
        }
        let proposal_id = proposal["id"].as_str().unwrap();
        let role_id = proposal["target"]["roleId"].as_str().unwrap();
        let Some(mut role) = self.roles.read()?.into_iter().find(|r| r["id"] == role_id) else {
            return self.store.complete_learning_approval(
                proposal_id,
                &json!({"notApplied":"The role no longer exists."}),
            );
        };
        if let Some(entry) = crate::role_history::history(&role)
            .iter()
            .find(|entry| entry["origin"] == "learning" && entry["proposalId"] == proposal_id)
        {
            return self.store.complete_learning_approval(
                proposal_id,
                &json!({"appliedVersion":entry["revision"]}),
            );
        }
        let changed = format!(
            "The role changed before the approved version was saved. {}",
            new_proposal_route(proposal)
        );
        if role["learningProposalId"] != proposal["id"] {
            if role["revision"].as_u64().unwrap_or(1) != proposal["baseVersion"].as_u64().unwrap() {
                return self
                    .store
                    .complete_learning_approval(proposal_id, &json!({"notApplied":changed}));
            }
            role["revision"] = proposal["baseVersion"].clone();
            role["instructions"] = proposal["change"].clone();
            match self.roles.update_from_learning(role_id, &role, proposal_id) {
                Ok(_) => (),
                Err(Error::Conflict(_)) => {
                    return self
                        .store
                        .complete_learning_approval(proposal_id, &json!({"notApplied":changed}));
                }
                Err(error) => return Err(error),
            }
        }
        let role = self
            .roles
            .read()?
            .into_iter()
            .find(|r| r["id"] == role_id)
            .ok_or_else(|| {
                conflict(
                    "The approved role disappeared before its saved revision could be recorded.",
                )
            })?;
        self.store.complete_learning_approval(
            proposal_id,
            &json!({"appliedVersion":role["revision"].as_u64().unwrap_or(1)}),
        )
    }
}
