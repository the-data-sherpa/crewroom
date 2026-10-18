//! Durable target-recovery state. Recorded proofs are checked here; OS/Git proof comes from the controller.
use crate::{
    Error, Result,
    repository_work::{array, same, truthy},
    store::{Store, conflict, id, remove, trim_text},
};
use rusqlite::params;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn index(work: &Value, request: &str) -> Option<usize> {
    array(&work["refreshes"])
        .iter()
        .position(|r| r["requestId"] == request)
}
fn unresolved(work: &Value) -> Option<usize> {
    array(&work["refreshes"])
        .iter()
        .rposition(|r| r["state"] == "conflicted" && !truthy(&r["supersededBy"]))
}
fn includes(value: &Value, states: &[&str]) -> bool {
    states.contains(&value.as_str().unwrap_or(""))
}
impl Store {
    fn unstopped_repository_attempts(&self, run_id: &str) -> Result<bool> {
        Ok(array(&self.runtime(run_id)?["attempts"])
            .iter()
            .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped"))
    }
    pub fn preflight_target_refresh(
        &self,
        run_id: &str,
        supersedes: Option<&Value>,
    ) -> Result<Value> {
        let run = self.get_run(run_id)?;
        let work = self
            .repository_work(run_id)?
            .filter(|w| w["state"] == "ready")
            .ok_or_else(|| Error::NotFound("Repository work has not been prepared.".into()))?;
        if !includes(&run["status"], &["paused", "attention_required"])
            || truthy(&work["writer"])
            || self.unstopped_repository_attempts(run_id)?
        {
            return Err(conflict(
                "Pause the run and confirm every agent stopped before refreshing from the target.",
            ));
        }
        if self.coding_record(run_id)?.is_some_and(|c| {
            array(&c["completions"])
                .iter()
                .any(|c| c["state"] == "requested")
        }) {
            return Err(conflict(
                "Reconcile pending implementation completion before refreshing from the target.",
            ));
        }
        if array(&work["refreshes"]).iter().any(|r| {
            includes(
                &r["resolutionWorkspace"]["state"],
                &["preparing", "continuing"],
            )
        }) {
            return Err(conflict(
                "Unverified Git mutation ownership blocks target refresh.",
            ));
        }
        if let Some(index) = unresolved(&work) {
            let held = &work["refreshes"][index];
            if supersedes.is_none_or(|v| {
                v["requestId"] != held["requestId"]
                    || trim_text(v["reason"].as_str().unwrap_or("")).is_empty()
            }) || (truthy(&held["resolutionWorkspace"])
                && held["resolutionWorkspace"]["state"] != "operator_required")
            {
                return Err(conflict(
                    "An operator reason is required to supersede this held target conflict.",
                ));
            }
        } else if supersedes.is_some() {
            return Err(conflict(
                "The operator retry no longer matches an active target hold.",
            ));
        }
        if array(&work["refreshes"])
            .iter()
            .any(|r| r["state"] == "rebasing")
        {
            return Err(conflict(
                "Reconcile the current target refresh before starting another.",
            ));
        }
        if self.publication(run_id)?.is_some_and(|p| {
            includes(&p["push"]["state"], &["sending", "uncertain", "blocked"])
                || includes(&p["pr"]["state"], &["sending", "uncertain", "blocked"])
        }) {
            return Err(conflict(
                "Reconcile the publication receipts before refreshing from the target.",
            ));
        }
        Ok(work)
    }
    pub fn begin_target_refresh(&self, run_id: &str, refresh: &Value) -> Result<Value> {
        self.transaction(|s| {
            let mut work=s.preflight_target_refresh(run_id,refresh.get("supersedes"))?;
            let coding=s.coding_record(run_id)?.unwrap_or(Value::Null);let source=array(&coding["candidates"]).last();
            if !truthy(&refresh["sourceCandidateId"]) || source.is_none_or(|c|c["id"]!=refresh["sourceCandidateId"] || c["revision"]["head"]!=refresh["fromHead"] || c["revision"]["base"]!=refresh["fromBase"]) || work["base"]!=refresh["fromBase"] {return Err(conflict("The target refresh source candidate changed before admission."));}
            if refresh["state"]=="completed" && (refresh["toBase"]!=refresh["fromBase"] || refresh["toHead"]!=refresh["fromHead"] || refresh["candidateId"]!=source.unwrap()["id"]) {return Err(conflict("An unchanged target receipt must already identify its source candidate."));}
            if array(&work["refreshes"]).iter().any(|r|r["requestId"]==refresh["requestId"]) {return Err(conflict("This target refresh request is already recorded."));}
            if let Some(index)=unresolved(&work) {work["refreshes"][index]["supersededBy"]=json!({"requestId":refresh["requestId"],"reason":refresh["supersedes"]["reason"],"at":refresh["at"]});}
            if refresh["state"]=="completed" && truthy(&work["continuationTargetRef"]) {work["targetRef"]=refresh["targetRef"].clone();remove(&mut work,"continuationTargetRef");}
            let mut refreshes=array(&work["refreshes"]).to_vec();refreshes.push(refresh.clone());work["refreshes"]=json!(refreshes);s.save_repository_work(&work)
        })
    }
    pub fn record_target_refresh_workspace(
        &self,
        run_id: &str,
        request_id: &str,
        workspace: &Value,
        conflicts: Option<&Value>,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            let at = index(&work, request_id);
            let at = at
                .filter(|i| includes(&work["refreshes"][*i]["state"], &["rebasing", "conflicted"]))
                .ok_or_else(|| {
                    conflict("Target refresh is not preparing or resolving a conflict workspace.")
                })?;
            let refresh = &work["refreshes"][at];
            if let Some(previous) = refresh.get("resolutionWorkspace") {
                if previous["path"] != workspace["path"]
                    || previous["branch"] != workspace["branch"]
                {
                    return Err(conflict("Target refresh workspace identity changed."));
                }
                if previous.get("requirementsHash") != workspace.get("requirementsHash")
                    || previous.get("instructionsHash") != workspace.get("instructionsHash")
                {
                    return Err(conflict(
                        "Conflict workspace accepted-scope snapshot changed.",
                    ));
                }
                if includes(&previous["state"], &["ready", "promoting", "promoted"])
                    && (previous.get("toHead") != workspace.get("toHead")
                        || previous.get("operationId") != workspace.get("operationId")
                        || !same(
                            previous.get("promotionProcess"),
                            workspace.get("promotionProcess"),
                        ))
                {
                    return Err(conflict("Conflict promotion identity changed."));
                }
                let next: &[&str] = match previous["state"].as_str().unwrap_or("") {
                    "preparing" => &["preparing", "conflicted", "operator_required"],
                    "conflicted" => &["conflicted", "ready", "operator_required"],
                    "continuing" => &["continuing"],
                    "operator_required" => &["operator_required"],
                    "ready" => &["ready", "operator_required", "promoting"],
                    "promoting" => &["promoting"],
                    "promoted" => &["promoted"],
                    _ => &[],
                };
                if !includes(&workspace["state"], next) {
                    return Err(conflict(
                        "Target refresh workspace state cannot move backward.",
                    ));
                }
                if !same(previous.get("continuation"), workspace.get("continuation"))
                    || !same(
                        previous.get("continuationReceipts"),
                        workspace.get("continuationReceipts"),
                    )
                {
                    return Err(conflict(
                        "Conflict continuation identity cannot change through workspace updates.",
                    ));
                }
            }
            if let Some(conflicts) = conflicts {
                work["refreshes"][at]["conflicts"] = conflicts.clone();
            }
            work["refreshes"][at]["resolutionWorkspace"] = workspace.clone();
            s.save_repository_work(&work)
        })
    }
    pub fn begin_target_conflict_continuation(
        &self,
        run_id: &str,
        request_id: &str,
        identity: &Value,
    ) -> Result<Value> {
        self.transaction(|s| {
            let run = s.get_run(run_id)?;
            let mut work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            let at = unresolved(&work);
            let refresh = at.map(|i| &work["refreshes"][i]).unwrap_or(&Value::Null);
            let workspace = &refresh["resolutionWorkspace"];
            let attempt = s.get_attempt(identity["attemptId"].as_str().unwrap())?;
            if at.is_none()
                || refresh["requestId"] != request_id
                || workspace["state"] != "conflicted"
                || truthy(&workspace["continuation"])
                || identity["requestId"] != request_id
                || identity["path"] != workspace["path"]
                || identity["branch"] != workspace["branch"]
                || identity["sourceHead"] != refresh["fromHead"]
                || array(&identity["paths"]).is_empty()
                || array(&identity["paths"])
                    .iter()
                    .any(|p| !array(&refresh["conflicts"]).contains(p))
                || array(&workspace["continuationReceipts"]).iter().any(|r| {
                    r["attemptId"] == identity["attemptId"]
                        && r["toolCallId"] == identity["toolCallId"]
                })
                || work["writer"]["attemptId"] != identity["attemptId"]
                || attempt["runId"] != run_id
                || attempt["ownership"] != "held"
                || !array(&s.runtime(run_id)?["assignments"])
                    .iter()
                    .any(|a| a["currentAttemptId"] == identity["attemptId"])
                || run["status"] != "running"
                || run["desiredState"] != "running"
            {
                return Err(conflict(
                    "The scoped conflict writer or Git continuation changed before admission.",
                ));
            }
            let workspace = &mut work["refreshes"][at.unwrap()]["resolutionWorkspace"];
            workspace["state"] = json!("continuing");
            workspace["continuation"] = identity.clone();
            s.save_repository_work(&work)
        })
    }
    pub fn finish_target_conflict_continuation(
        &self,
        run_id: &str,
        request_id: &str,
        identity: &Value,
        result: &Value,
    ) -> Result<Value> {
        self.transaction(|s| {
            let run=s.get_run(run_id)?;let mut work=s.repository_work(run_id)?.unwrap_or(Value::Null);let at=unresolved(&work);
            let refresh=at.map(|i|&work["refreshes"][i]).unwrap_or(&Value::Null);let workspace=&refresh["resolutionWorkspace"];
            if at.is_none() || refresh["requestId"]!=request_id || workspace["state"]!="continuing" || !same(workspace.get("continuation"),Some(identity)) || work["writer"]["attemptId"]!=identity["attemptId"] || s.get_attempt(identity["attemptId"].as_str().unwrap())?["ownership"]!="held" || run["status"]!="running" || run["desiredState"]!="running" || result["state"]=="ready" && !truthy(&result["head"]) {return Err(conflict("Git continuation cannot settle without its current writer and validated identity."));}
            let workspace=&mut work["refreshes"][at.unwrap()]["resolutionWorkspace"];workspace["state"]=result["state"].clone();if let Some(head)=result.get("head").filter(|h|!h.is_null()) {workspace["toHead"]=head.clone();}
            remove(workspace,"continuation");let mut receipts=array(&workspace["continuationReceipts"]).to_vec();let mut receipt=identity.clone();receipt["result"]=result.clone();receipts.push(receipt);workspace["continuationReceipts"]=json!(receipts);work["refreshes"][at.unwrap()]["conflicts"]=result["conflicts"].clone();s.save_repository_work(&work)
        })
    }
    pub fn begin_target_promotion(
        &self,
        run_id: &str,
        request_id: &str,
        operation_id: &str,
        to_head: &str,
        process: &Value,
    ) -> Result<Value> {
        self.transaction(|s| {
            let run = s.get_run(run_id)?;
            let mut work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            let at = index(&work, request_id);
            let at = at
                .filter(|i| {
                    work["refreshes"][*i]["state"] == "conflicted"
                        && includes(
                            &work["refreshes"][*i]["resolutionWorkspace"]["state"],
                            &["ready", "promoting"],
                        )
                })
                .ok_or_else(|| {
                    conflict("A ready isolated target conflict is required for promotion.")
                })?;
            let workspace = &work["refreshes"][at]["resolutionWorkspace"];
            if workspace["state"] == "promoting"
                && (workspace["operationId"] != operation_id
                    || workspace["toHead"] != to_head
                    || !same(workspace.get("promotionProcess"), Some(process)))
            {
                return Err(conflict("Target conflict promotion identity changed."));
            }
            if process["version"] != 1
                || !process["root"].as_str().unwrap_or("").ends_with(&format!(
                    "{operation_id}-{}",
                    process["token"].as_str().unwrap_or("")
                ))
                || process["cwd"] != work["path"]
                || array(&process["args"]).last().is_none_or(|v| v != to_head)
            {
                return Err(conflict(
                    "Promotion process does not match the recorded source.",
                ));
            }
            if run["status"] != "paused"
                || run["desiredState"] != "running"
                || truthy(&work["writer"])
                || s.unstopped_repository_attempts(run_id)?
            {
                return Err(conflict(
                    "Confirm all agent ownership stopped before promoting a conflict.",
                ));
            }
            if !s.coding_record(run_id)?.is_some_and(|c| {
                array(&c["completions"]).iter().any(|c| {
                    c["id"] == operation_id
                        && c["state"] == "requested"
                        && c["conflictRefreshId"] == request_id
                })
            }) {
                return Err(conflict("A recorded conflict completion is required."));
            }
            let workspace = &mut work["refreshes"][at]["resolutionWorkspace"];
            if workspace["state"] == "ready" {
                workspace["state"] = json!("promoting");
                workspace["operationId"] = json!(operation_id);
                workspace["toHead"] = json!(to_head);
                workspace["promotionProcess"] = process.clone();
            }
            s.save_repository_work(&work)
        })
    }
    pub fn reject_unpromoted_conflict(
        &self,
        run_id: &str,
        request_id: &str,
        operation_id: &str,
        proof: &Value,
    ) -> Result<Value> {
        self.terminal_unpromoted_conflict(run_id, request_id, operation_id, proof, true)
    }
    pub fn interrupt_unpromoted_conflict(
        &self,
        run_id: &str,
        request_id: &str,
        operation_id: &str,
        proof: &Value,
    ) -> Result<Value> {
        self.terminal_unpromoted_conflict(run_id, request_id, operation_id, proof, false)
    }
    fn terminal_unpromoted_conflict(
        &self,
        run_id: &str,
        request_id: &str,
        operation_id: &str,
        proof: &Value,
        target_changed: bool,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut run=s.get_run(run_id)?;let mut work=s.repository_work(run_id)?.unwrap_or(Value::Null);let at=index(&work,request_id);
            let refresh=at.map(|i|&work["refreshes"][i]).unwrap_or(&Value::Null);let workspace=&refresh["resolutionWorkspace"];
            let mut coding=s.coding_record(run_id)?.unwrap_or(Value::Null);let completion_index=array(&coding["completions"]).iter().position(|c|c["id"]==operation_id);let completion=completion_index.map(|i|&coding["completions"][i]).unwrap_or(&Value::Null);
            let source=array(&coding["candidates"]).last().unwrap_or(&Value::Null);let owner=completion_index.map(|_|s.get_attempt(completion["attemptId"].as_str().unwrap())).transpose()?.unwrap_or(Value::Null);
            let instructions_hash=workspace.get("instructionsHash").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!(format!("{:x}",Sha256::digest(crate::serialized::stringify(&work["instructions"]).as_bytes()))));
            if at.is_none() || refresh["state"]!="conflicted" || truthy(&refresh["supersededBy"]) || unresolved(&work)!=at || !includes(&workspace["state"],&["ready","promoting"]) || workspace["state"]=="promoting" && workspace["operationId"]!=operation_id || !truthy(&workspace["gitFile"]) || !truthy(&workspace["toHead"]) || workspace["toHead"]!=proof["toHead"] || workspace["gitFile"]!=proof["workspaceGitFile"] || work["gitFile"]!=proof["originalGitFile"] || workspace["path"]!=proof["workspacePath"] || workspace["branch"]!=proof["workspaceBranch"] || workspace.get("requirementsHash").filter(|v|!v.is_null()).unwrap_or(&work["requirementsHash"])!=&proof["requirementsHash"] || instructions_hash!=proof["instructionsHash"] || completion_index.is_none() || completion["state"]!="requested" || completion["conflictRefreshId"]!=request_id || owner["runId"]!=run_id || owner["ownership"]!="stopped" || !array(&coding["claims"]).iter().any(|c|c["writerAttemptId"]==completion["attemptId"] && c["targetRefreshId"]==request_id) || truthy(&completion["candidateId"]) || completion["head"]!=proof["originalHead"] || completion["worktreeHash"]!=proof["worktreeHash"] || work["path"]!=proof["originalPath"] || work["branch"]!=proof["originalBranch"] || work["base"]!=proof["originalBase"] || work["targetRef"]!=proof["originalTargetRef"] || !truthy(&refresh["sourceCandidateId"]) || source["id"]!=refresh["sourceCandidateId"] || source["revision"]["head"]!=refresh["fromHead"] || source["revision"]["base"]!=refresh["fromBase"] || refresh["fromHead"]!=proof["originalHead"] || refresh["fromBase"]!=proof["originalBase"] || refresh["targetRef"]!=proof["targetRef"] || refresh["toBase"]!=proof["expectedTarget"] || target_changed && (!truthy(&proof["observedTarget"]) || proof["observedTarget"]==proof["expectedTarget"]) || truthy(&work["writer"]) || !includes(&run["status"],&["paused","attention_required"]) || run["desiredState"]=="cancelled" || s.unstopped_repository_attempts(run_id)? {return Err(conflict("Unpromoted conflict termination lacks stopped, unmoved proof."));}
            let at_time=s.time();let code=if target_changed {"target_changed"} else {"promotion_interrupted"};
            let mut rejection=json!({"code":code,"at":at_time,"expectedTarget":proof["expectedTarget"]});
            if target_changed {rejection["observedTarget"]=proof["observedTarget"].clone();}
            rejection["reason"]=json!(if target_changed {"The live target moved before the resolved conflict could be promoted."} else {"The checked Git reset stopped without applying the resolved conflict; an explicit operator retry is required."});
            coding["completions"][completion_index.unwrap()]["state"]=json!("rejected");coding["completions"][completion_index.unwrap()]["rejection"]=rejection.clone();work["refreshes"][at.unwrap()]["resolutionWorkspace"]["state"]=json!("operator_required");
            if run["codingTransition"]["completionId"]==operation_id {remove(&mut run,"codingTransition");s.save_run(&run)?;}
            s.db.execute("UPDATE coding SET data=? WHERE run_id=?",params![coding.to_string(),run_id])?;s.save_repository_work(&work)?;
            let kind=if target_changed {"target_conflict_rejected"} else {"target_conflict_interrupted"};let mut payload=json!({"kind":kind,"subjectId":operation_id});payload.as_object_mut().unwrap().extend(rejection.as_object().unwrap().clone());payload["refreshId"]=json!(request_id);
            payload["instruction"]=json!(if target_changed {"The live target moved before conflict promotion. The isolated workspace is retained; choose a reasoned retry from the current target. The original currently matches its recorded clean source; no candidate or Crewroom PR write was recorded."} else {"The checked Git reset did not apply. The original currently matches its recorded clean source, and the resolved isolated workspace remains. Choose an explicit reasoned retry even if the target has not changed; do not replay the rejected completion."});
            if !s.delivery_notices(run_id)?.iter().any(|n|n["payload"]["kind"]==kind && n["payload"]["subjectId"]==operation_id) {let notice=json!({"id":id(),"runId":run_id,"payload":payload,"createdAt":at_time,"disposition":"pending"});s.db.execute("INSERT INTO delivery_notices VALUES(?,?,?)",params![notice["id"].as_str(),run_id,notice.to_string()])?;}
            Ok(coding["completions"][completion_index.unwrap()].clone())
        })
    }
    pub fn record_target_rebase_recovery(
        &self,
        run_id: &str,
        request_id: &str,
        stage: &str,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            let at = index(&work, request_id);
            let refresh = at.map(|i| &work["refreshes"][i]).unwrap_or(&Value::Null);
            if at.is_none()
                || refresh["state"] != "rebasing"
                || truthy(&refresh["resolutionWorkspace"])
                || (if stage == "rebase_stopped" {
                    truthy(&refresh["rebaseRecovery"])
                } else if stage == "undo_started" {
                    refresh["rebaseRecovery"] != "rebase_stopped"
                } else {
                    refresh["rebaseRecovery"] != "undo_started"
                })
            {
                return Err(conflict(
                    "Target rebase recovery stage changed or lacks an observed stopped child.",
                ));
            }
            work["refreshes"][at.unwrap()]["rebaseRecovery"] = json!(stage);
            s.save_repository_work(&work)
        })
    }
    pub fn finish_target_refresh(
        &self,
        run_id: &str,
        request_id: &str,
        outcome: &Value,
        checkpoint: Option<&Value>,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            let at = index(&work, request_id)
                .ok_or_else(|| Error::NotFound("Target refresh is not recorded.".into()))?;
            let refresh = &work["refreshes"][at];
            let promoting = refresh["state"] == "conflicted"
                && refresh["resolutionWorkspace"]["state"] == "promoting"
                && outcome["state"] == "completed";
            if refresh["state"] == "rebasing" || promoting {
                if outcome["state"] == "completed" {
                    let coding = s.coding_record(run_id)?.unwrap_or(Value::Null);
                    let source = array(&coding["candidates"]).last().unwrap_or(&Value::Null);
                    let run = s.get_run(run_id)?;
                    if checkpoint.is_none()
                        || !promoting && refresh["rebaseRecovery"] != "rebase_stopped"
                        || promoting
                            && (refresh["resolutionWorkspace"]["toHead"] != outcome["toHead"]
                                || !array(&coding["completions"]).iter().any(|c| {
                                    c["id"] == refresh["resolutionWorkspace"]["operationId"]
                                        && c["state"] == "requested"
                                        && c["conflictRefreshId"] == request_id
                                }))
                        || checkpoint.is_some_and(|c| {
                            c["revision"]["base"] != refresh["toBase"]
                                || c["revision"]["head"] != outcome["toHead"]
                                || truthy(&c["revision"]["dirty"])
                        })
                    {
                        return Err(conflict(
                            "A completed refresh needs its matching clean checkpoint.",
                        ));
                    }
                    if work["base"] != refresh["fromBase"]
                        || truthy(&work["writer"])
                        || (if promoting {
                            !includes(&run["status"], &["paused", "attention_required"])
                        } else {
                            run["status"] != "paused"
                        })
                        || promoting && run["desiredState"] == "cancelled"
                        || s.unstopped_repository_attempts(run_id)?
                        || truthy(&refresh["sourceCandidateId"])
                            && source["id"] != refresh["sourceCandidateId"]
                        || source["revision"]["head"] != refresh["fromHead"]
                        || source["revision"]["base"] != refresh["fromBase"]
                        || !promoting
                            && array(&coding["completions"])
                                .iter()
                                .any(|c| c["state"] == "requested")
                    {
                        return Err(conflict(
                            "Refresh source or stopped ownership changed before completion.",
                        ));
                    }
                    let base = refresh["toBase"].clone();
                    let target = refresh["targetRef"].clone();
                    work["base"] = base;
                    work["targetRef"] = target;
                    remove(&mut work, "continuationTargetRef");
                    work["checkpoints"]
                        .as_array_mut()
                        .unwrap()
                        .push(checkpoint.unwrap().clone());
                }
                let refresh = &work["refreshes"][at];
                if outcome["state"] == "interrupted" && truthy(&refresh["supersedes"]) {
                    let coding = s.coding_record(run_id)?.unwrap_or(Value::Null);
                    let source = array(&coding["candidates"]).last().unwrap_or(&Value::Null);
                    let run = s.get_run(run_id)?;
                    if checkpoint.is_some()
                        || truthy(&outcome["toHead"])
                        || truthy(&outcome["candidateId"])
                        || truthy(&outcome["resolutionWorkspace"])
                        || truthy(&refresh["resolutionWorkspace"])
                        || refresh["rebaseRecovery"] != "restored"
                        || truthy(&refresh["toHead"])
                        || truthy(&refresh["candidateId"])
                        || work["base"] != refresh["fromBase"]
                        || truthy(&work["writer"])
                        || run["status"] != "paused"
                        || s.unstopped_repository_attempts(run_id)?
                        || array(&coding["completions"])
                            .iter()
                            .any(|c| c["state"] == "requested")
                        || source.is_null()
                        || source["id"] != refresh["sourceCandidateId"]
                        || source["revision"]["head"] != refresh["fromHead"]
                        || source["revision"]["base"] != refresh["fromBase"]
                    {
                        return Err(conflict(
                            "Interrupted retry lacks stopped, unchanged source proof.",
                        ));
                    }
                    work["refreshes"][at]["state"] = json!("conflicted");
                    work["refreshes"][at]["holdReason"] = json!("interrupted_retry");
                    work["refreshes"][at]["conflicts"] = json!([]);
                    return s.save_repository_work(&work);
                }
                if outcome["state"] == "interrupted"
                    && !truthy(&refresh["supersedes"])
                    && refresh["rebaseRecovery"] != "restored"
                {
                    return Err(conflict(
                        "An interrupted refresh requires a completed, clean rollback receipt.",
                    ));
                }
                work["refreshes"][at]
                    .as_object_mut()
                    .unwrap()
                    .extend(outcome.as_object().unwrap().clone());
            } else if truthy(&outcome["candidateId"])
                && refresh["state"] == "completed"
                && refresh["candidateId"].is_null()
            {
                work["refreshes"][at]["candidateId"] = outcome["candidateId"].clone();
            }
            s.save_repository_work(&work)
        })
    }
}
