//! Refinement decisions and research outcome records. Artifact verification is a
//! separate boundary: trusted evidence callers supply verified artifact records.
use crate::{
    Error, Result,
    lifecycle::Transition,
    store::{Store, conflict, id, integer, invalid, request_id, strict, trim_text},
};
use rusqlite::params;
use serde_json::{Value, json};

fn string(value: &Value, min: usize, max: usize, trim: bool) -> Result<String> {
    let value = value.as_str().ok_or_else(|| invalid("Expected text."))?;
    let value = if trim { trim_text(value) } else { value };
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid("Text is outside its permitted size."));
    }
    Ok(value.to_owned())
}
fn strings(value: &Value, min: usize, max: usize) -> Result<Value> {
    let values = value
        .as_array()
        .filter(|a| (min..=100).contains(&a.len()))
        .ok_or_else(|| invalid("Invalid text list size."))?;
    Ok(json!(
        values
            .iter()
            .map(|v| string(v, 1, max, true))
            .collect::<Result<Vec<_>>>()?
    ))
}
fn accepted_input(body: &Value) -> Result<Value> {
    let mut value = json!({"revision":integer(body,"revision",1,u64::MAX)?,"sourceRevision":integer(body,"sourceRevision",1,u64::MAX)?,"brief":string(&body["brief"],0,40000,false)?,"criteria":strings(&body["criteria"],0,2000)?});
    if let Some(note) = body.get("decisionNote") {
        value["decisionNote"] = json!(string(note, 1, 4000, true)?);
    }
    Ok(value)
}
fn proposal_input(body: &Value) -> Result<Value> {
    strict(
        body,
        &["brief", "criteria", "decisions", "unresolvedQuestions"],
    )?;
    Ok(
        json!({"brief":string(&body["brief"],1,40000,true)?,"criteria":strings(&body["criteria"],1,2000)?,"decisions":strings(&body["decisions"],0,4000)?,"unresolvedQuestions":strings(&body["unresolvedQuestions"],0,4000)?}),
    )
}
impl Store {
    pub fn accept_requirements(
        &self,
        task_id: &str,
        body: &Value,
        provenance: &Value,
    ) -> Result<Value> {
        let value = accepted_input(body)?;
        if provenance["origin"] == "refinement"
            && provenance["proposalId"].as_str().is_none_or(str::is_empty)
        {
            return Err(conflict(
                "An approved refinement needs its proposal identity.",
            ));
        }
        self.transaction(|s| s.save_accepted_requirements(task_id, &value, provenance))
    }
    fn save_accepted_requirements(
        &self,
        task_id: &str,
        value: &Value,
        provenance: &Value,
    ) -> Result<Value> {
        let previous = self.get_task(task_id)?;
        crate::store::operator_task(&previous)?;
        if previous["revision"] != value["revision"] {
            return Err(conflict("The brief changed. Reload before saving."));
        }
        if previous["sourceRevision"] != value["sourceRevision"] {
            return Err(conflict(
                "Source requirements changed. Reload before saving.",
            ));
        }
        let revisions = self.requirement_revisions(task_id)?;
        let source_changed = previous["needsReacceptance"] == true
            || revisions
                .last()
                .is_some_and(|r| r["sourceRevision"] != value["sourceRevision"]);
        let mut accepted = json!({"taskId":task_id,"revision":previous["revision"].as_u64().unwrap()+1,"brief":value["brief"],"criteria":value["criteria"],"sourceRevision":value["sourceRevision"],"origin":if source_changed {json!("jira-reacceptance")} else {provenance["origin"].clone()},"createdAt":self.time()});
        if let Some(proposal) = provenance.get("proposalId") {
            accepted["proposalId"] = proposal.clone();
        }
        if let Some(note) = value.get("decisionNote") {
            accepted["decisionNote"] = note.clone();
        }
        let mut refinement = self.refinement(task_id)?;
        for key in ["revision", "brief", "criteria"] {
            refinement[key] = accepted[key].clone();
        }
        refinement["acceptedSourceRevision"] = value["sourceRevision"].clone();
        refinement["needsReacceptance"] = json!(false);
        if let Some(r) = refinement.as_object_mut() {
            r.remove("acceptedSource");
        }
        self.save_refinement(task_id, &refinement)?;
        self.db.execute(
            "INSERT INTO requirement_revisions VALUES(?,?,?)",
            params![
                task_id,
                accepted["revision"].as_u64().unwrap() as i64,
                accepted.to_string()
            ],
        )?;
        self.get_task(task_id)
    }
    fn persist_outcome_note(&self, task_id: &str, value: Value) -> Result<()> {
        let mut note = value;
        note["id"] = json!(id());
        note["taskId"] = json!(task_id);
        note["createdAt"] = json!(self.time());
        self.db.execute(
            "INSERT INTO notes VALUES(?,?,?)",
            params![note["id"].as_str(), task_id, note.to_string()],
        )?;
        Ok(())
    }
    pub fn refinement_proposals(&self, run_id: Option<&str>) -> Result<Vec<Value>> {
        if let Some(run_id) = run_id.filter(|s| !s.is_empty()) {
            self.run_records("refinement_proposals", run_id)
        } else {
            self.all("refinement_proposals")
        }
    }
    pub fn propose_refinement(
        &self,
        attempt_id: &str,
        tool_call_id: &str,
        body: &Value,
    ) -> Result<Value> {
        let mut value = proposal_input(body)?;
        string(&json!(tool_call_id), 1, 200, false)?;
        self.transaction(|s| {
            let attempt=s.get_attempt(attempt_id)?;let run=s.get_run(attempt["runId"].as_str().unwrap())?;
            if let Some(saved)=s.refinement_proposals(None)?.into_iter().find(|p|p["attemptId"]==attempt_id && p["toolCallId"]==tool_call_id) {
                if ["brief","criteria","decisions","unresolvedQuestions"].iter().any(|k|saved[*k]!=value[*k]) {return Err(conflict("Proposal identity has different content."));}
                return Ok(saved);
            }
            let run_id=run["id"].as_str().unwrap();let task_id=run["taskId"].as_str().unwrap();
            if !["explore","grill"].contains(&run["purpose"].as_str().unwrap_or("")) || run["status"]!="running" || run["desiredState"]!="running" || attempt["ownership"]!="held" || !s.run_records("assignments",run_id)?.iter().any(|a|a["coordinator"]==true && a["currentAttemptId"]==attempt_id) {
                return Err(conflict("Only the active refinement coordinator may propose requirements."));
            }
            if run["roleDraft"].is_object() {
                if run["roleDraft"]["proposalId"].is_string() {return Err(conflict("A role draft proposal is already recorded."));}
                value["roleDraft"]=crate::role_drafts::parse_proposal(&run,&value)?;
            }
            value["id"]=json!(id());value["runId"]=run["id"].clone();value["taskId"]=run["taskId"].clone();value["attemptId"]=json!(attempt_id);value["toolCallId"]=json!(tool_call_id);value["baseRevision"]=run["taskRevision"].clone();value["sourceRevision"]=json!(run["taskSnapshot"]["sourceRevision"].as_u64().unwrap_or(1));value["status"]=json!("pending");value["createdAt"]=json!(s.time());
            s.db.execute("INSERT INTO refinement_proposals VALUES(?,?,?,?,?)",params![value["id"].as_str(),run_id,attempt_id,tool_call_id,value.to_string()])?;
            if value["roleDraft"].is_object() {let mut run=run.clone();run["roleDraft"]["proposalId"]=value["id"].clone();s.save_run(&run)?;}
            for body in value["decisions"].as_array().unwrap() {s.persist_outcome_note(task_id,json!({"mode":"decision","body":body,"kind":"decision","proposalId":value["id"],"revision":value["baseRevision"]}))?;}
            for body in value["unresolvedQuestions"].as_array().unwrap() {s.persist_outcome_note(task_id,json!({"mode":run["purpose"],"body":body,"kind":"unresolved_question","proposalId":value["id"],"revision":value["baseRevision"]}))?;}
            Ok(value)
        })
    }
    pub fn decide_refinement(&self, proposal_id: &str, body: &Value) -> Result<Value> {
        strict(
            body,
            &["decision", "requestId", "reason", "brief", "criteria"],
        )?;
        let request = request_id(body)?;
        let accept = match body["decision"].as_str() {
            Some("accept") => true,
            Some("discard") => false,
            _ => return Err(invalid("Decision must accept or discard.")),
        };
        let mut value = json!({"requestId":request});
        for (key, max) in [("reason", 4000), ("brief", 40000)] {
            if let Some(v) = body.get(key) {
                value[key] = json!(string(v, 1, max, true)?);
            }
        }
        if let Some(criteria) = body.get("criteria") {
            value["criteria"] = strings(criteria, 1, 2000)?;
        }
        if !accept
            && (value.get("reason").is_none()
                || value.get("brief").is_some()
                || value.get("criteria").is_some())
        {
            return Err(conflict(
                "Discard requires a reason and cannot edit the proposal.",
            ));
        }
        self.transaction(|s| {
            let mut proposal=s.record("refinement_proposals","id",proposal_id)?.ok_or_else(||Error::NotFound("Refinement proposal not found.".into()))?;
            if proposal["roleDraft"].is_object() {return Err(conflict("Review this specialist draft in Agent roles; it is not a ticket refinement."));}
            let status=if accept {"accepted"} else {"discarded"};
            if proposal["status"]!="pending" {
                if proposal["status"]==status && proposal["decision"]["requestId"]==request {
                    if ["reason","brief","criteria"].iter().any(|k|proposal["decision"][*k]!=value[*k]) {return Err(conflict("Decision identity has different content."));}
                    return Ok(proposal);
                }
                return Err(conflict("This proposal has already been decided."));
            }
            let task_id=proposal["taskId"].as_str().unwrap();let task=s.get_task(task_id)?;
            let revision=if accept {
                if task["revision"]!=proposal["baseRevision"] || task["sourceRevision"]!=proposal["sourceRevision"] || task["needsReacceptance"]==true {return Err(conflict("The proposal is based on an earlier revision. Reload before accepting."));}
                let accepted=s.save_accepted_requirements(task_id,&json!({"revision":task["revision"],"sourceRevision":task["sourceRevision"].as_u64().unwrap_or(1),"brief":value.get("brief").unwrap_or(&proposal["brief"]),"criteria":value.get("criteria").unwrap_or(&proposal["criteria"])}),&json!({"origin":"refinement","proposalId":proposal["id"]}))?;
                for mut note in s.all("notes")?.into_iter().filter(|n|n["proposalId"]==proposal_id) {note["revision"]=accepted["revision"].clone();s.db.execute("UPDATE notes SET data=? WHERE id=?",params![note.to_string(),note["id"].as_str()])?;}
                accepted["revision"].clone()
            } else {
                s.persist_outcome_note(task_id,json!({"mode":"decision","body":value["reason"],"proposalId":proposal_id,"revision":proposal["baseRevision"]}))?;
                proposal["baseRevision"].clone()
            };
            proposal["status"]=json!(status);value["at"]=json!(s.time());value["revision"]=revision;proposal["decision"]=value;
            s.db.execute("UPDATE refinement_proposals SET data=? WHERE id=?",params![proposal.to_string(),proposal_id])?;Ok(proposal)
        })
    }
    pub fn research_artifacts(&self, run_id: &str) -> Result<Vec<Value>> {
        self.get_run(run_id)?;
        self.run_records("research_artifacts", run_id)
    }
    /// The source/evidence service validates the artifact body, citations and identity.
    pub fn save_research_artifact(&self, artifact: &Value) -> Result<Value> {
        self.transaction(|s| {
            let attempt=s.get_attempt(artifact["attemptId"].as_str().unwrap())?;
            let run_id=artifact["runId"].as_str().unwrap();let run=s.get_run(run_id)?;
            if let Some(prior)=s.all("research_artifacts")?.into_iter().find(|a|a["attemptId"]==artifact["attemptId"] && a["toolCallId"]==artifact["toolCallId"]) {return Ok(prior);}
            let current=s.run_records("assignments",run_id)?.into_iter().any(|a|a["id"]==attempt["assignmentId"] && a["currentAttemptId"]==attempt["id"] && a.get("retiredAt").is_none());
            if run["purpose"]!="research" || run["status"]!="running" || run["desiredState"]!="running" || run.get("sourceConflict").is_some() || run.get("researchCompletion").is_some() || run["researchOutcome"]["revision"]==run["taskRevision"] || !current || attempt["ownership"]!="held" || !["idle","executing"].contains(&attempt["state"].as_str().unwrap_or("")) {return Err(conflict("Only a current research agent may submit an artifact before completion is requested."));}
            if artifact["requirementRevision"]!=run["taskRevision"] || artifact["pinnedCommit"]!=run["researchSource"]["commit"] {return Err(conflict("Research requirements or source changed during submission."));}
            if let Some(mut previous)=s.research_artifacts(run_id)?.into_iter().rev().find(|a|a["assignmentId"]==artifact["assignmentId"] && a["title"]==artifact["title"] && a["kind"]==artifact["kind"] && a.get("supersededBy").is_none()) {
                previous["supersededBy"]=artifact["id"].clone();s.db.execute("UPDATE research_artifacts SET data=? WHERE id=?",params![previous.to_string(),previous["id"].as_str()])?;
            }
            s.db.execute("INSERT INTO research_artifacts VALUES(?,?,?,?,?)",params![artifact["id"].as_str(),run_id,attempt["id"].as_str(),artifact["toolCallId"].as_str(),artifact.to_string()])?;Ok(artifact.clone())
        })
    }
    pub fn request_research_completion(
        &self,
        run_id: &str,
        attempt_id: &str,
        tool_call_id: &str,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            let request = format!("{attempt_id}:{tool_call_id}");
            if let Some(prior) = run.get("researchCompletion") {
                if prior["requestId"] != request {
                    return Err(conflict("Research completion has already been requested."));
                }
                return Ok(prior.clone());
            }
            if run["purpose"] != "research"
                || run["status"] != "running"
                || run["desiredState"] != "running"
                || !s
                    .run_records("assignments", run_id)?
                    .iter()
                    .any(|a| a["coordinator"] == true && a["currentAttemptId"] == attempt_id)
            {
                return Err(conflict(
                    "Only the active research coordinator may request completion.",
                ));
            }
            let request = json!({"requestId":request,"revision":run["taskRevision"]});
            run["researchCompletion"] = request.clone();
            s.save_run(&run)?;
            Ok(request)
        })
    }
    pub fn complete_research(
        &self,
        run_id: &str,
        request_id: &str,
        revision: u64,
    ) -> Result<Value> {
        self.transaction(|s| {
            let run = s.get_run(run_id)?;
            if run["status"] == "completed" && run["researchOutcome"]["requestId"] == request_id {
                return Ok(run);
            }
            if run["purpose"] != "research"
                || run["taskRevision"] != revision
                || run["researchCompletion"]["requestId"] != request_id
            {
                return Err(conflict("Current accepted research evidence is required."));
            }
            if !s.insertion_blockers(run_id)?.is_empty() {
                return Err(conflict(
                    "Reconcile inserted specialist responsibilities before completing research.",
                ));
            }
            let current = s
                .research_artifacts(run_id)?
                .into_iter()
                .filter(|a| {
                    a["state"] == "verified"
                        && a.get("supersededBy").is_none()
                        && a["requirementRevision"] == revision
                        && a["pinnedCommit"] == run["researchSource"]["commit"]
                        && a["citations"]
                            .as_array()
                            .is_some_and(|c| c.iter().all(|v| v["verified"] == true))
                })
                .collect::<Vec<_>>();
            let criteria = run["taskSnapshot"]["criteria"].as_array().unwrap();
            if trim_text(run["taskSnapshot"]["brief"].as_str().unwrap_or("")).is_empty()
                || criteria.is_empty()
                || current.is_empty()
                || (0..criteria.len()).any(|index| {
                    !current.iter().any(|a| {
                        a["coverage"]
                            .as_array()
                            .is_some_and(|v| v.iter().any(|e| e["criterionId"] == index))
                    })
                })
            {
                return Err(conflict(
                    "Verified current research artifacts must cover every accepted criterion.",
                ));
            }
            if s.runtime(run_id)?["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            {
                return Err(conflict("Research process ownership must be stopped."));
            }
            let mut updated = s.change_run(run_id, Transition::CompleteResearch)?;
            updated["researchOutcome"] =
                json!({"revision":revision,"completedAt":s.time(),"requestId":request_id});
            s.save_run(&updated)?;
            s.get_run(run_id)
        })
    }
}
