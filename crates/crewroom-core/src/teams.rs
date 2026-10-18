//! Proposed teams reserve work; only a stopped, reconciled run may admit the change.
use crate::{
    Error, Result,
    briefs::snapshot_briefs,
    roles::validate_role,
    store::{Store, conflict, id, integer, invalid, remove, request_id, strict, trim_text},
};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::HashSet;

fn string(value: &Value, min: usize, max: usize, trim: bool) -> Result<&str> {
    let value = value.as_str().ok_or_else(|| invalid("Expected text."))?;
    let value = if trim { trim_text(value) } else { value };
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid("Text is outside its permitted size."));
    }
    Ok(value)
}
fn list(value: Option<&Value>, max: usize) -> Result<Vec<Value>> {
    match value {
        None => Ok(vec![]),
        Some(value) => value
            .as_array()
            .filter(|a| a.len() <= max)
            .cloned()
            .ok_or_else(|| invalid("List is outside its permitted size.")),
    }
}
// Preserve JSON field order where the original implementation uses serialized identity.
fn same_serialized(left: &Value, right: &Value) -> bool {
    crate::serialized::same(left, right)
}
pub fn insertion_request(input: &Value) -> Result<Value> {
    strict(
        input,
        &[
            "requestId",
            "roleId",
            "input",
            "dependsOn",
            "criteria",
            "modelPreview",
        ],
    )?;
    let request = request_id(input)?;
    let role = string(&input["roleId"], 1, 100, false)?;
    let instruction = string(&input["input"], 1, 40000, true)?;
    let dependencies = list(input.get("dependsOn"), 10)?;
    for dep in &dependencies {
        string(dep, 1, 100, false)?;
    }
    let criteria = list(input.get("criteria"), 100)?
        .iter()
        .map(|criterion| {
            integer(&json!({"index":criterion}), "index", 0, u64::MAX).map(|n| json!(n))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut result = json!({"requestId":request,"roleId":role,"input":instruction,"dependsOn":dependencies,"criteria":criteria});
    crate::launch::model_preview(input, &mut result)?;
    Ok(result)
}
fn specialists(input: &[Value]) -> Result<Vec<Value>> {
    if input.len() > 3 {
        return Err(invalid("At most three specialists may be proposed."));
    }
    input
        .iter()
        .map(|item| {
            strict(item, &["name", "role", "model", "input", "brief"])?;
            let name = string(&item["name"], 1, 61, false)?;
            if !name.as_bytes()[0].is_ascii_alphabetic()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                return Err(invalid("Invalid assignment name."));
            }
            let role = validate_role(&item["role"])?;
            let instruction = string(&item["input"], 1, 40000, true)?;
            let brief = item
                .get("brief")
                .cloned()
                .unwrap_or_else(|| json!({"criteria":[],"dependsOn":[]}));
            if !brief.is_object() && !brief.is_array() {
                return Err(invalid("A brief must be an object."));
            }
            let mut specialist = json!({"name":name,"role":role});
            if let Some(model) = item.get("model") {
                specialist["model"] = model.clone();
            }
            specialist["input"] = json!(instruction);
            specialist["brief"] = brief;
            Ok(specialist)
        })
        .collect()
}
impl Store {
    pub fn team_proposals(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run_records("team_proposals", run_id)
    }
    pub fn get_team_proposal(&self, proposal_id: &str) -> Result<Value> {
        self.record("team_proposals", "id", proposal_id)?
            .ok_or_else(|| Error::NotFound("Team proposal not found.".into()))
    }
    fn save_team_proposal(&self, proposal: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE team_proposals SET data=? WHERE id=?",
            params![proposal.to_string(), proposal["id"].as_str()],
        )?;
        Ok(())
    }
    pub fn insertion_replay(&self, run_id: &str, value: &Value) -> Result<Option<Value>> {
        let prior = self
            .team_proposals(run_id)?
            .into_iter()
            .find(|p| p["origin"] == "operator" && p["toolCallId"] == value["requestId"]);
        if let Some(prior) = &prior
            && !same_serialized(&prior["request"], value)
        {
            return Err(conflict(
                "Insertion request identity has different content.",
            ));
        }
        Ok(prior)
    }
    pub fn propose_operator_specialist(
        &self,
        run_id: &str,
        input: &Value,
        role: &Value,
        model: &Value,
    ) -> Result<Value> {
        let value = insertion_request(input)?;
        self.transaction(|s|{
            if let Some(replay)=s.insertion_replay(run_id,&value)? {return Ok(replay);}
            let run=s.get_run(run_id)?;
            if run["desiredState"]=="cancelled" || ["cancelled","closed","completed"].contains(&run["status"].as_str().unwrap_or("")){return Err(conflict("This run has ended. Specialist insertion is unavailable."));}
            if !["coding","research"].contains(&run["purpose"].as_str().unwrap_or("coding")){return Err(conflict("Refinement runs cannot insert specialists."));}
            if role["id"]!=value["roleId"] || validate_role(role)?["enabled"]!=true {return Err(conflict("Selected specialist role is disabled or unavailable."));}
            if run["purpose"]=="research" && ["implementer","reviewer"].contains(&role["id"].as_str().unwrap_or("")){return Err(conflict("Research cannot admit coding specialists."));}
            if run["purpose"]=="research" && role["id"]!="delivery-reporter" && value["criteria"].as_array().unwrap().is_empty(){return Err(conflict("Research specialists need an accepted criterion obligation in their brief."));}
            let proposals=s.team_proposals(run_id)?;
            if proposals.iter().any(|p|p["status"]=="pending"){return Err(conflict("Apply the existing team proposal before inserting another specialist."));}
            let assignments=s.run_records("assignments",run_id)?;
            let existing=if assignments.is_empty(){run["launch"]["team"].as_array().cloned().unwrap_or_else(||vec![json!({"name":"Coordinator","coordinator":true})])}else{assignments};
            if existing.iter().filter(|a|a["coordinator"]!=true && a.get("retiredAt").is_none()).count()>=3{return Err(conflict("The specialist concurrency limit is three plus the coordinator."));}
            let mut used:HashSet<String>=existing.iter().filter_map(|a|a["name"].as_str().map(str::to_owned)).collect();
            for proposal in &proposals {for member in proposal["specialists"].as_array().unwrap(){if let Some(name)=member["name"].as_str(){used.insert(name.into());}}}
            let mut number=1;while used.contains(&format!("Specialist{number}")){number+=1;}
            let name=format!("Specialist{number}");
            // The original limit is 2,000 UTF-16 units. Preserve complete scalars at its boundary.
            let mut units=0;let obligation:String=value["input"].as_str().unwrap().chars().take_while(|c|{units+=c.len_utf16();units<=2000}).collect();
            let criteria:Vec<_>=value["criteria"].as_array().unwrap().iter().map(|index|json!({"index":index,"obligation":obligation,"checkIds":[]})).collect();
            let briefs=snapshot_briefs(&[json!({"name":name,"dependsOn":value["dependsOn"],"criteria":criteria})],&run["taskSnapshot"]["criteria"],&s.configured_check_ids(run_id)?,&existing)?;
            let proposal=json!({"id":id(),"runId":run_id,"toolCallId":value["requestId"],"origin":"operator","request":value,"requirementRevision":run["taskRevision"],"createdAt":s.time(),"specialists":[{"name":name,"role":role,"model":model,"input":value["input"],"brief":briefs[&name]}],"status":"pending"});
            s.db.execute("INSERT INTO team_proposals VALUES(?,?,?,?,?)",params![proposal["id"].as_str(),run_id,Option::<String>::None,value["requestId"].as_str(),proposal.to_string()])?;
            let notice=json!({"id":id(),"runId":run_id,"createdAt":proposal["createdAt"],"disposition":"pending","payload":{"kind":"operator_specialist_proposed","subjectId":proposal["id"],"specialist":proposal["specialists"][0],"instruction":"The operator requested this specialist under the accepted requirements. Crewroom applies it at a safe coordination point; do not duplicate it. Reconcile its results, then retire the assignment through propose_team with a reason. Readiness waits until that responsibility is closed. Changed code still requires checks and independent review."}});
            s.db.execute("INSERT INTO delivery_notices VALUES(?,?,?)",params![notice["id"].as_str(),run_id,notice.to_string()])?;Ok(proposal)
        })
    }
    pub fn insertion_blockers(&self, run_id: &str) -> Result<Vec<String>> {
        let assignments = self.run_records("assignments", run_id)?;
        let mut blockers = vec![];
        for proposal in self
            .team_proposals(run_id)?
            .into_iter()
            .filter(|p| p["origin"] == "operator")
        {
            if proposal["status"] == "pending" {
                blockers
                    .push("Operator specialist insertion awaits a safe coordination point.".into());
            } else if proposal["status"] == "applied" {
                let names: Vec<_> = assignments
                    .iter()
                    .filter(|a| {
                        a.get("retiredAt").is_none()
                            && proposal["assignmentIds"]
                                .as_array()
                                .is_some_and(|ids| ids.contains(&a["id"]))
                    })
                    .map(|a| a["role"]["name"].as_str().unwrap_or(""))
                    .collect();
                if !names.is_empty() {
                    blockers.push(format!("Readiness waits for the coordinator to reconcile and close the inserted assignment: {}.",names.join(", ")));
                }
            }
        }
        Ok(blockers)
    }
    pub fn propose_team(
        &self,
        attempt_id: &str,
        tool_call_id: &str,
        input: &[Value],
        retire: &[Value],
    ) -> Result<Value> {
        string(&json!(tool_call_id), 1, 200, false)?;
        let specialists = specialists(input)?;
        if specialists.iter().any(|s| s["role"]["enabled"] != true) {
            return Err(conflict("Disabled roles cannot receive new assignments."));
        }
        if retire.len() > 3 {
            return Err(invalid("At most three retirements may be proposed."));
        }
        let retirement_requests:Vec<_>=retire.iter().map(|r|{strict(r,&["name","reason"])?;Ok(json!({"name":string(&r["name"],0,usize::MAX,false)?,"reason":string(&r["reason"],1,2000,true)?}))}).collect::<Result<_>>()?;
        if specialists.is_empty() && retirement_requests.is_empty() {
            return Err(conflict("Propose at least one team change."));
        }
        self.transaction(|s|{
            let attempt=s.get_attempt(attempt_id)?;let run_id=attempt["runId"].as_str().unwrap();let run=s.get_run(run_id)?;
            if !["coding","research"].contains(&run["purpose"].as_str().unwrap_or("coding")){return Err(conflict("Refinement cannot propose specialists."));}
            let assignments=s.run_records("assignments",run_id)?;
            if attempt["ownership"]!="held" || !assignments.iter().any(|a|a["coordinator"]==true && a["currentAttemptId"]==attempt_id){return Err(conflict("Only the active coordinator can propose specialists."));}
            let mut retirements=vec![];
            for request in &retirement_requests {
                let target=assignments.iter().find(|a|a["name"]==request["name"]).filter(|a|a["coordinator"]!=true).ok_or_else(||conflict("Only an admitted specialist can retire."))?;
                retirements.push(json!({"assignmentId":target["id"],"reason":request["reason"]}));
            }
            if retirements.iter().map(|r|r["assignmentId"].as_str()).collect::<HashSet<_>>().len()!=retirements.len(){return Err(conflict("Retirement targets must be distinct."));}
            let proposals=s.team_proposals(run_id)?;
            if let Some(previous)=proposals.iter().find(|p|p["attemptId"]==attempt_id && p["toolCallId"]==tool_call_id){
                if !same_serialized(&previous["specialists"],&json!(specialists)) || !same_serialized(previous.get("retirements").unwrap_or(&json!([])),&json!(retirements)){return Err(conflict("Proposal identity has different content."));}return Ok(previous.clone());
            }
            if run["status"]!="running"{return Err(conflict("The run is not accepting proposals."));}
            if proposals.iter().any(|p|p["status"]=="pending"){return Err(conflict("The concurrency limit includes pending proposals; apply the existing change first."));}
            let work=s.record("repository_work","run_id",run_id)?.unwrap_or(Value::Null);
            if retirements.iter().any(|target|assignments.iter().find(|a|a["id"]==target["assignmentId"]).is_some_and(|a|a.get("retiredAt").is_some() || work["writer"]["attemptId"]==a["currentAttemptId"])){return Err(conflict("A retired specialist or current writer cannot retire."));}
            if assignments.iter().filter(|a|a.get("retiredAt").is_none()).count() as i64-retirements.len() as i64+specialists.len() as i64>4{return Err(conflict("The specialist concurrency limit is three plus the coordinator."));}
            if assignments.iter().chain(&specialists).map(|a|a["name"].as_str()).collect::<HashSet<_>>().len()!=assignments.len()+specialists.len(){return Err(conflict("Assignment names must be distinct."));}
            s.validate_choreography_team(run_id,&specialists,&retirements)?;
            let proposal=json!({"id":id(),"runId":run_id,"attemptId":attempt_id,"toolCallId":tool_call_id,"specialists":specialists,"retirements":retirements,"status":"pending"});
            s.db.execute("INSERT INTO team_proposals VALUES(?,?,?,?,?)",params![proposal["id"].as_str(),run_id,attempt_id,tool_call_id,proposal.to_string()])?;Ok(proposal)
        })
    }
    pub fn reject_unavailable_team_proposal(
        &self,
        proposal_id: &str,
        roles: &[Value],
    ) -> Result<Option<Value>> {
        let proposal = self.get_team_proposal(proposal_id)?;
        if proposal["status"] == "rejected" {
            return Ok(Some(proposal));
        }
        if proposal["status"] != "pending"
            || self.team_rejection_reason(&proposal, roles)?.is_none()
        {
            return Ok(None);
        }
        self.transaction(|s| {
            let mut proposal = s.get_team_proposal(proposal_id)?;
            if proposal["status"] == "pending" {
                s.reject_team_in_transaction(&mut proposal, roles)?;
            }
            Ok((proposal["status"] == "rejected").then_some(proposal))
        })
    }
    fn team_rejection_reason(&self, proposal: &Value, roles: &[Value]) -> Result<Option<String>> {
        if proposal["origin"] == "operator"
            && proposal["requirementRevision"]
                != self.get_run(proposal["runId"].as_str().unwrap())?["taskRevision"]
        {
            return Ok(Some("Accepted requirements changed before insertion. Submit a new specialist proposal for the current requirements.".into()));
        }
        if !roles.is_empty()
            && let Some(unavailable) =
                proposal["specialists"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| {
                        !roles
                            .iter()
                            .any(|r| r["id"] == s["role"]["id"] && r["enabled"] == true)
                    })
        {
            return Ok(Some(format!(
                "Selected role {} is disabled or unavailable. No specialists from this proposal were admitted; choose available roles and propose a new team.",
                unavailable["role"]["name"].as_str().unwrap_or("")
            )));
        }
        Ok(None)
    }
    fn reject_team_in_transaction(&self, proposal: &mut Value, roles: &[Value]) -> Result<bool> {
        let Some(reason) = self.team_rejection_reason(proposal, roles)? else {
            return Ok(false);
        };
        self.record_team_rejection(proposal, &reason)?;
        Ok(true)
    }
    /// Retain an invalidated admission as rejected so a fresh proposal can be
    /// made. Already-applied proposals and their frozen assignments are intact.
    pub fn reject_team_proposal(&self, proposal_id: &str, reason: &str) -> Result<Value> {
        string(&json!(reason), 1, 2000, false)?;
        self.transaction(|s| {
            let mut proposal = s.get_team_proposal(proposal_id)?;
            if proposal["status"] == "pending" {
                s.record_team_rejection(&mut proposal, reason)?;
            }
            Ok(proposal)
        })
    }
    fn record_team_rejection(&self, proposal: &mut Value, reason: &str) -> Result<()> {
        proposal["status"] = json!("rejected");
        proposal["reason"] = json!(reason);
        self.save_team_proposal(proposal)?;
        let run = proposal["runId"].as_str().unwrap();
        let mut current = self.get_run(run)?;
        if current["teamTransition"]["proposalId"] == proposal["id"] {
            remove(&mut current, "teamTransition");
            self.save_run(&current)?;
        }
        for mut notice in self.delivery_notices(run)?.into_iter().filter(|n| {
            n["payload"]["subjectId"] == proposal["id"]
                && n["payload"]["kind"] == "operator_specialist_proposed"
                && n["disposition"] == "pending"
        }) {
            notice["payload"] = json!({"kind":"operator_specialist_proposed","subjectId":proposal["id"],"status":"rejected","reason":reason,"instruction":"This operator proposal was rejected before admission. Retain its history; wait for a new operator request rather than recreating it."});
            self.db.execute(
                "UPDATE delivery_notices SET data=? WHERE id=?",
                params![notice.to_string(), notice["id"].as_str()],
            )?;
        }
        let notice = json!({"id":id(),"runId":run,"payload":{"kind":"team_proposal_rejected","subjectId":proposal["id"],"reason":reason,"origin":proposal.get("origin").cloned().unwrap_or(json!("coordinator"))},"createdAt":self.time(),"disposition":"pending"});
        self.db.execute(
            "INSERT INTO delivery_notices VALUES(?,?,?)",
            params![notice["id"].as_str(), run, notice.to_string()],
        )?;
        Ok(())
    }
    pub fn apply_team_proposal(&self, proposal_id: &str, roles: &[Value]) -> Result<Value> {
        self.apply_team_proposal_with_principles(proposal_id, roles, &[])
    }
    pub fn apply_team_proposal_with_principles(
        &self,
        proposal_id: &str,
        roles: &[Value],
        principles: &[Value],
    ) -> Result<Value> {
        self.transaction(|s|{
            let mut proposal=s.get_team_proposal(proposal_id)?;if proposal["status"]!="pending"{return Ok(proposal);}
            let run_id=proposal["runId"].as_str().unwrap().to_owned();let run=s.get_run(&run_id)?;
            let assignments=s.run_records("assignments",&run_id)?;let runtime=s.runtime(&run_id)?;
            if run["status"]!="paused" || run["desiredState"]!="running" || runtime["attempts"].as_array().unwrap().iter().any(|a|a["state"]!="admitted" && a["ownership"]!="stopped"){return Err(conflict("Pause and confirm old ownership before inserting specialists."));}
            if runtime["messages"].as_array().unwrap().iter().any(|m|m["disposition"]!="received") || s.steering(&run_id)?.iter().any(|m|m["delivery"]=="dispatched"){return Err(conflict("Reconcile pending delivery before inserting specialists."));}
            let retirements=proposal["retirements"].as_array().cloned().unwrap_or_default();let specialists=proposal["specialists"].as_array().unwrap().clone();
            if assignments.iter().filter(|a|a.get("retiredAt").is_none()).count() as i64-retirements.len() as i64+specialists.len() as i64>4{return Err(conflict("The specialist concurrency limit is three plus the coordinator."));}
            if s.reject_team_in_transaction(&mut proposal,roles)?{return Ok(proposal);}
            let work=s.record("repository_work","run_id",&run_id)?.unwrap_or(Value::Null);
            for target in &retirements {
                let mut assignment=assignments.iter().find(|a|a["id"]==target["assignmentId"]).filter(|a|a["coordinator"]!=true && a.get("retiredAt").is_none() && work["writer"]["attemptId"]!=a["currentAttemptId"]).cloned().ok_or_else(||conflict("Specialist retirement is no longer valid."))?;
                assignment["retiredAt"]=json!(s.time());assignment["retirementReason"]=target["reason"].clone();s.save_assignment(&assignment)?;
            }
            s.validate_choreography_team(&run_id,&specialists,&retirements)?;
            let mut assignment_ids=vec![];
            for specialist in specialists {
                let attempt_id=id();let assignment_id=id();
                let current=roles.iter().find(|r|r["id"]==specialist["role"]["id"] && r["enabled"]==true && r["model"]==specialist["role"]["model"] && r["effort"]==specialist["role"]["effort"] && r["job"]==specialist["role"]["job"]);
                let role=if let Some(current)=current {validate_role(current)?}else{specialist["role"].clone()};
                let mut assignment=json!({"id":assignment_id,"runId":run_id,"name":specialist["name"],"role":role,"coordinator":false,"currentAttemptId":attempt_id});
                if run["implementationPlan"].is_object() {assignment["implementationPlan"]=run["implementationPlan"].clone();}
                if let Some(binding)=s.choreography_assignment(&run_id,&specialist["name"],&role)?{assignment["choreography"]=binding;}
                if let Some(model)=specialist.get("model"){assignment["model"]=model.clone();}
                assignment["brief"]=crate::principles::snapshot(specialist.get("brief").unwrap_or(&json!({"criteria":[],"dependsOn":[]})), &role, principles)?;
                if let Some(guidance)=s.current_guidance(&run_id)?{assignment["projectGuidance"]=guidance;}
                let mut attempt=json!({"id":attempt_id,"assignmentId":assignment_id,"runId":run_id,"generation":1,"input":specialist["input"],"state":"admitted","createdAt":s.time()});
                if let Some(model)=specialist.get("model"){attempt["model"]=model.clone();}
                s.db.execute("INSERT INTO assignments VALUES(?,?,?,?)",params![assignment_id,run_id,specialist["name"].as_str(),assignment.to_string()])?;
                s.db.execute("INSERT INTO attempts VALUES(?,?,?)",params![attempt_id,assignment_id,attempt.to_string()])?;assignment_ids.push(assignment_id);
            }
            proposal["assignmentIds"]=json!(assignment_ids);proposal["status"]=json!("applied");s.save_team_proposal(&proposal)?;Ok(proposal)
        })
    }
    pub fn begin_team_transition(&self, proposal_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let proposal = s.get_team_proposal(proposal_id)?;
            let run_id = proposal["runId"].as_str().unwrap();
            let mut run = s.get_run(run_id)?;
            if run["status"] != "running" || run["desiredState"] != "running" {
                return Err(conflict("The run is not executing."));
            }
            let mut transition = json!({"proposalId":proposal_id});
            if let Some(revision) = run.get("controlRevision") {
                transition["controlRevision"] = revision.clone();
            }
            run["teamTransition"] = transition;
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn finish_team_transition(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            remove(&mut run, "teamTransition");
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn begin_coding_transition(&self, run_id: &str, completion_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            let coding = s.record("coding", "run_id", run_id)?.unwrap_or(Value::Null);
            if run["status"] != "running"
                || run["desiredState"] != "running"
                || !coding["completions"].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|c| c["id"] == completion_id && c["state"] == "requested")
                })
            {
                return Err(conflict("Coding completion is not ready for transition."));
            }
            let mut transition = json!({"completionId":completion_id});
            if let Some(revision) = run.get("controlRevision") {
                transition["controlRevision"] = revision.clone();
            }
            run["codingTransition"] = transition;
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn finish_coding_transition(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            remove(&mut run, "codingTransition");
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
}
