//! Ownership receipts only. Native Git/process proof is a separate admission boundary.
use crate::{
    Error, Result,
    dependencies::coding,
    store::{Store, conflict, remove},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
pub(crate) fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => true,
    }
}
/// Undefined and null differ, and field order remains part of captured proof identity.
pub(crate) fn same(left: Option<&Value>, right: Option<&Value>) -> bool {
    left.map(crate::serialized::stringify) == right.map(crate::serialized::stringify)
}
impl Store {
    pub fn reserve_repository_work(&self, record: &Value) -> Result<Value> {
        self.transaction(|s| {
            let run=s.get_run(record["runId"].as_str().unwrap())?;let run_id=run["id"].as_str().unwrap();
            if !coding(&run) || run["taskSnapshot"]["repositoryId"]!=record["repository"]["id"] || run["status"]!="awaiting_runner" || ["cancelled","paused"].contains(&run["desiredState"].as_str().unwrap_or("")) || array(&s.runtime(run_id)?["attempts"]).iter().any(|a|a["state"]!="admitted") {return Err(conflict("Only a coding run can prepare repository ownership before execution."));}
            if let Some(reason)=s.implementation_plan_hold(&run)? { return Err(conflict(&reason)); }
            if s.run_records("assignments",run_id)?.is_empty() && run["implementationPlan"]["decision"]["basis"]["commit"] != record["base"] {
                return Err(conflict("Prepared repository base differs from the accepted plan source. Revalidate before creating work."));
            }
            let selected=s.repository_by_id(record["repository"]["id"].as_str().unwrap())?;
            if truthy(&selected["retiredAt"]) || selected["revision"].as_u64().unwrap_or(1)!=record["repository"]["revision"].as_u64().unwrap_or(1) {return Err(Error::RepositoryConfigurationChanged("Repository configuration changed during preparation; retry from current configuration.".into()));}
            if s.repository_work(run_id)?.is_some() {return Err(conflict("Repository work is already reserved."));}
            if s.db.query_row("SELECT run_id FROM repository_work WHERE common_dir=? AND branch=? AND json_extract(data,'$.transferredTo') IS NULL",params![record["commonDir"].as_str(),record["branch"].as_str()],|r|r.get::<_,String>(0)).optional()?.is_some() {return Err(conflict("The branch is already owned by another run."));}
            s.db.execute("INSERT INTO repository_work VALUES(?,?,?,?)",params![run_id,record["commonDir"].as_str(),record["branch"].as_str(),record.to_string()])?;Ok(record.clone())
        })
    }
    pub fn confirm_repository_work(
        &self,
        run_id: &str,
        git_file: &str,
        instructions: &Value,
        checkpoint: &Value,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut work = s
                .repository_work(run_id)?
                .ok_or_else(|| Error::NotFound("Repository work is not reserved.".into()))?;
            if work["state"] == "ready" {
                return Ok(work);
            }
            work["state"] = json!("ready");
            work["gitFile"] = json!(git_file);
            work["instructions"] = instructions.clone();
            work["checkpoints"]
                .as_array_mut()
                .unwrap()
                .push(checkpoint.clone());
            s.save_repository_work(&work)
        })
    }
    pub fn claim_repository_writer(
        &self,
        attempt_id: &str,
        checkpoint: &Value,
        claim: Option<&Value>,
    ) -> Result<Value> {
        self.transaction(|s| {
            let attempt=s.get_attempt(attempt_id)?;let run_id=attempt["runId"].as_str().unwrap();let run=s.get_run(run_id)?;
            let mut work=s.repository_work(run_id)?.unwrap_or(Value::Null);let claim_value=claim.unwrap_or(&Value::Null);
            if array(&work["refreshes"]).iter().any(|r|r["state"]=="rebasing" || ["preparing","continuing"].contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))) {return Err(conflict("Unverified Git mutation ownership blocks repository writers."));}
            let unresolved=array(&work["refreshes"]).iter().rev().find(|r|r["state"]=="rebasing" || r["state"]=="conflicted" && !truthy(&r["supersededBy"]));
            let conflict_workspace=unresolved.filter(|r|r["state"]=="conflicted" && ["conflicted","ready"].contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or("")));
            if let Some(unresolved)=unresolved && (conflict_workspace.is_none() || claim_value["targetRefreshId"]!=unresolved["requestId"]) {return Err(conflict("Target conflict requires an explicitly scoped writer and approved workspace."));}
            if truthy(&claim_value["targetRefreshId"]) && conflict_workspace.is_none_or(|r|claim_value["targetRefreshId"]!=r["requestId"]) {return Err(conflict("Implementation claim targets a different conflict workspace."));}
            let publication=s.publication(run_id)?;let feedback=array(&run["feedbackContinuations"]).iter().find(|f|f.get("id")==claim_value.get("feedbackContinuationId"));
            let coding=s.coding_record(run_id)?.unwrap_or(Value::Null);let source=array(&coding["candidates"]).last();
            let grant=if truthy(&claim_value["restoredFrom"]) {array(&coding["claims"]).iter().rev().find(|c|c["writerAttemptId"]==claim_value["restoredFrom"] && c.get("feedbackContinuationId")==feedback.and_then(|f|f.get("id")))} else {None};
            let prior_attempt=if grant.is_some() {Some(s.get_attempt(claim_value["restoredFrom"].as_str().unwrap())?)} else {None};
            let before=array(&work["checkpoints"]).last();
            let restored=match (grant,prior_attempt.as_ref(),source,before) {
                (Some(grant),Some(prior),Some(source),Some(before))=>attempt["previousAttemptId"]==prior["id"] && prior["assignmentId"]==attempt["assignmentId"] && prior["ownership"]=="stopped" && grant["candidateId"]==source["id"] && grant["instruction"]==claim_value["instruction"] && before["reason"]=="interrupted" && same(before.get("revision"),checkpoint.get("revision")) && ["head","tree","base","requirementsHash","instructionsHash"].iter().all(|key|checkpoint["revision"][key]==source["revision"][key]),
                _=>false,
            };
            let root_candidate=array(&coding["candidates"]).iter().find(|c|c.get("id")==feedback.and_then(|f|f.get("sourceCandidateId")));
            let active_feedback=match (source,feedback) {(Some(source),Some(feedback))=>(["starting","handed_off"].contains(&feedback["state"].as_str().unwrap_or("")) && source["id"]==feedback["sourceCandidateId"]) || (feedback["state"]=="candidate" && feedback["candidateId"]==source["id"] && source["feedbackFrom"]["continuationId"]==feedback["id"]),_=>false};
            let feedback_allowed=match (publication.as_ref(),feedback,source,root_candidate) {
                (Some(publication),Some(feedback),Some(source),Some(root))=>active_feedback && publication["id"]==feedback["publicationId"] && publication["pr"]["id"]==feedback["prId"] && publication["repository"]["uuid"]==feedback["repository"]["uuid"] && publication["pr"]["state"]=="confirmed" && publication["pr"]["providerState"]=="OPEN" && publication["push"]["state"]=="confirmed" && publication["candidateId"]==root["id"] && root["revision"]["head"]==feedback["sourceHead"] && (if truthy(&claim_value["restoredFrom"]) {restored} else {same(checkpoint.get("revision"),source.get("revision"))}) && publication["branch"]==work["branch"] && publication["repositoryPath"]==work["repository"]["path"],_=>false,
            };
            if publication.as_ref().is_some_and(|p|(conflict_workspace.is_none() && !feedback_allowed) || truthy(&p["merged"]) || ["sending","uncertain","blocked"].contains(&p["push"]["state"].as_str().unwrap_or("")) || ["sending","uncertain","blocked"].contains(&p["pr"]["state"].as_str().unwrap_or(""))) {return Err(conflict("Publication owns this revision. Explicit review continuation is required before more repository writes."));}
            let runtime=s.runtime(run_id)?;let assignment=array(&runtime["assignments"]).iter().find(|a|a["currentAttemptId"]==attempt_id);
            if work.is_null() || work["state"]!="ready" || assignment.is_none_or(|a|truthy(&a["retiredAt"]) || a["coordinator"]==true) || attempt["ownership"]!="held" || !["idle","executing"].contains(&attempt["state"].as_str().unwrap_or("")) || run["status"]!="running" || run["desiredState"]!="running" {return Err(conflict("Only an active specialist may own repository writes."));}
            s.assert_choreography_writer(attempt_id,claim)?;
            if let Some(handoff)=array(&coding["completions"]).iter().find(|c|c.get("id")==claim_value.get("handoffId"))
                && !array(&coding["claims"]).iter().any(|c|c["handoffId"]==handoff["id"])
                && handoff["checkpointRevision"]!=checkpoint["revision"] {
                return Err(conflict("Repository changed after the handoff checkpoint."));
            }
            let assignment=assignment.unwrap();if truthy(&assignment["rebriefRequired"]) {return Err(conflict("This assignment needs a new brief after requirements adoption."));}
            if s.team_proposals(run_id)?.iter().any(|p|p["status"]=="pending" && array(&p["retirements"]).iter().any(|t|t["assignmentId"]==assignment["id"])) {return Err(conflict("This specialist is pending retirement."));}
            if work["writer"]["attemptId"]==attempt_id {return Ok(work);}
            if truthy(&work["writer"]) {return Err(conflict("Another writer owns repository work. Confirm termination before replacement."));}
            if let Some(claim)=claim {
                let mut coding=s.coding_record(run_id)?.ok_or_else(||conflict("Implementation claim is not configured."))?;
                if claim["writerAttemptId"]!=attempt_id {return Err(conflict("Implementation claim is not configured."));}
                if coding["repairAssignmentId"].is_null() {coding["repairAssignmentId"]=assignment["id"].clone();}
                let mut owner=s.get_assignment(coding["repairAssignmentId"].as_str().unwrap())?;
                if claim["automatic"]==true {
                    let limit=2+array(&owner["repairAllowances"]).len() as u64;let cycles=owner["automaticCycles"].as_u64().unwrap_or(0);
                    if cycles>=limit {return Err(conflict(&format!("{} automatic repair/replacement cycles are exhausted. Allow another cycle explicitly to continue.",if limit==2 {"The two".into()} else {format!("All {limit}")})));}
                    owner["automaticCycles"]=json!(cycles+1);s.save_assignment(&owner)?;
                }
                let mut claims=array(&coding["claims"]).to_vec();claims.push(claim.clone());coding["claims"]=json!(claims);s.db.execute("UPDATE coding SET data=? WHERE run_id=?",params![coding.to_string(),run_id])?;
            }
            work["writer"]=json!({"attemptId":attempt_id,"acquiredAt":s.time()});work["checkpoints"].as_array_mut().unwrap().push(checkpoint.clone());s.save_repository_work(&work)
        })
    }
    pub fn release_repository_writer(&self, attempt_id: &str, checkpoint: &Value) -> Result<Value> {
        self.transaction(|s| {
            let attempt = s.get_attempt(attempt_id)?;
            let mut work = s
                .repository_work(attempt["runId"].as_str().unwrap())?
                .unwrap_or(Value::Null);
            if !truthy(&work["writer"]) || work["writer"]["attemptId"] != attempt_id {
                return Err(conflict("Attempt does not own repository writes."));
            }
            if attempt["ownership"] != "stopped" {
                return Err(conflict(
                    "Confirm stopped ownership before releasing the writer.",
                ));
            }
            remove(&mut work, "writer");
            work["checkpoints"]
                .as_array_mut()
                .unwrap()
                .push(checkpoint.clone());
            s.save_repository_work(&work)
        })
    }
    pub fn checkpoint_repository(&self, run_id: &str, checkpoint: &Value) -> Result<Value> {
        self.transaction(|s| {
            let mut work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            if work.is_null()
                || truthy(&work["writer"])
                || array(&s.runtime(run_id)?["attempts"])
                    .iter()
                    .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            {
                return Err(conflict(
                    "Confirm stopped ownership before checkpointing a completed revision.",
                ));
            }
            work["checkpoints"]
                .as_array_mut()
                .unwrap()
                .push(checkpoint.clone());
            s.save_repository_work(&work)
        })
    }
}
