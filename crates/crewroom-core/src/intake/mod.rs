//! Plan history, operator decisions and durable intake intent. No method starts a process.
mod dialogue;
mod requests;
mod source;
pub use source::{PinnedSource, SourcePreflight};
pub mod plan;
use crate::{
    Error, Result,
    store::{Store, conflict, id, integer, request_id, strict, text},
};
use rusqlite::params;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(crate::serialized::stringify(value)))
}
impl Store {
    fn task_intake_records(&self, table: &str, task_id: &str) -> Result<Vec<Value>> {
        let mut query = self.db.prepare(&format!(
            "SELECT data FROM {table} WHERE task_id=? ORDER BY rowid"
        ))?;
        query
            .query_map([task_id], |r| r.get::<_, String>(0))?
            .map(|r| crate::store::parse(r?))
            .collect()
    }
    pub fn implementation_plan_task(&self, plan_id: &str) -> Result<Value> {
        Ok(self
            .record("implementation_plans", "id", plan_id)?
            .ok_or_else(|| Error::NotFound("Plan not found.".into()))?["taskId"]
            .clone())
    }
    pub fn plan_decision_replay(&self, plan_id: &str, input: &Value) -> Result<Option<Value>> {
        self.replay(
            request_id(input)?,
            "plan-decision",
            &json!({"planId":plan_id,"input":input}),
        )
    }

    pub fn implementation_plans(&self, task_id: &str) -> Result<Vec<Value>> {
        let task = self.get_task(task_id)?;
        let mut plans = self.task_intake_records("implementation_plans", task_id)?;
        for plan in &mut plans {
            plan["decision"] = self
                .record("plan_decisions", "plan_id", plan["id"].as_str().unwrap())?
                .unwrap_or(Value::Null);
            let basis = if plan["decision"]["decision"] == "accept" {
                &plan["decision"]["basis"]
            } else {
                &plan["basis"]
            };
            plan["holdReason"] = json!(self.source_hold(&task, basis)?);
        }
        Ok(plans)
    }
    pub fn current_implementation_plan(&self, task_id: &str) -> Result<Option<Value>> {
        Ok(self
            .implementation_plans(task_id)?
            .into_iter()
            .rev()
            .find(|p| p["decision"]["decision"] == "accept")
            .filter(|p| p["holdReason"].is_null()))
    }
    pub fn save_implementation_plan(&self, task_id: &str, input: &Value) -> Result<Value> {
        self.write_implementation_plan(task_id, input, &json!({"kind":"operator"}))
    }
    fn write_implementation_plan(
        &self,
        task_id: &str,
        input: &Value,
        author: &Value,
    ) -> Result<Value> {
        strict(
            input,
            &[
                "requestId",
                "revision",
                "sourceId",
                "predecessorId",
                "content",
                "requirements",
            ],
        )?;
        let key = request_id(input)?;
        self.transaction(|s| {
            let payload = json!({"taskId":task_id,"input":input});
            if let Some(prior) = s.replay(key,"implementation-plan",&payload)? { return Ok(prior); }
            let task = s.get_task(task_id)?;
            if task["revision"] != integer(input,"revision",1,u64::MAX)? { return Err(conflict("The task changed. Keep your draft and reload.")); }
            let basis = s.plan_source(task_id)?.ok_or_else(|| conflict("Inspect the repository source before proposing a plan."))?;
            if basis["id"] != input["sourceId"] { return Err(conflict("Plan source changed. Revalidate your draft.")); }
            if let Some(reason) = s.source_hold(&task, &basis)? { return Err(conflict(&reason)); }
            let mut accepted_task = task.clone();
            if let Some(requirements) = input.get("requirements") {
                strict(requirements, &["brief","criteria"])?;
                crate::store::trimmed_text(requirements,"brief",1,40000)?;
                let criteria = requirements["criteria"].as_array().filter(|c| !c.is_empty() && c.len()<=100).ok_or_else(|| conflict("Proposed requirements need criteria."))?;
                if criteria.iter().any(|c| c.as_str().is_none_or(|v| v.trim().is_empty() || v.chars().count()>2000)) { return Err(conflict("Invalid proposed criteria.")); }
                accepted_task["brief"] = requirements["brief"].clone(); accepted_task["criteria"] = requirements["criteria"].clone();
            }
            plan::validate(&input["content"],accepted_task["criteria"].as_array().map(Vec::as_slice).unwrap_or(&[]))?;
            let history = s.implementation_plans(task_id)?;
            if history.last().map(|p| &p["id"]) != input.get("predecessorId").filter(|p| !p.is_null()) { return Err(conflict("Plan history changed. Reload before saving another version.")); }
            let mut value = json!({"id":id(),"taskId":task_id,"version":history.len()+1,"basis":basis,"content":input["content"],"sha256":hash(&input["content"]),"predecessorId":input["predecessorId"],"author":author,"createdAt":s.time()});
            if let Some(r) = input.get("requirements") { value["requirements"] = r.clone(); }
            s.db.execute("INSERT INTO implementation_plans VALUES(?,?,?)",params![value["id"].as_str(),task_id,value.to_string()])?;
            s.receipt(key,"implementation-plan",&payload,&value)?;
            Ok(value)
        })
    }
    pub fn decide_implementation_plan(&self, plan_id: &str, input: &Value) -> Result<Value> {
        strict(
            input,
            &["requestId", "revision", "sourceId", "decision", "reason"],
        )?;
        let key = request_id(input)?;
        let decision = text(input, "decision", 1, 20)?;
        if !["accept", "reject"].contains(&decision) {
            return Err(conflict("Choose accept or reject."));
        }
        if decision == "reject" {
            crate::store::trimmed_text(input, "reason", 1, 4000)?;
        }
        self.transaction(|s| {
            let payload = json!({"planId":plan_id,"input":input});
            if let Some(prior) = s.replay(key,"plan-decision",&payload)? { return Ok(prior); }
            let plan = s.record("implementation_plans","id",plan_id)?.ok_or_else(|| Error::NotFound("Plan not found.".into()))?;
            let task_id = plan["taskId"].as_str().unwrap();
            let task = s.get_task(task_id)?;
            if s.record("plan_decisions","plan_id",plan_id)?.is_some() { return Err(conflict("This plan already has an operator decision.")); }
            if task["revision"] != input["revision"] || plan["basis"]["taskRevision"] != task["revision"] || plan["basis"]["id"] != input["sourceId"] {
                return Err(conflict("The displayed task or source changed. Reload before deciding."));
            }
            if s.implementation_plans(task_id)?.last().is_none_or(|p| p["id"] != plan_id) { return Err(conflict("A newer plan draft exists.")); }
            if let Some(reason) = s.source_hold(&task, &plan["basis"])? { return Err(conflict(&reason)); }
            let mut value = json!({"planId":plan_id,"decision":decision,"operator":"local-operator","at":s.time(),"requestId":key,"basis":plan["basis"]});
            if let Some(reason)=input.get("reason") { value["reason"]=reason.clone(); }
            let intake = s.active_intake(task_id)?;
            if let Some(intake) = &intake {
                s.assert_intake_live(intake)?;
                if decision == "accept" && intake["questions"].as_array().into_iter().flatten().any(|q| q["sourceId"] == plan["basis"]["id"] && q["answer"].is_null()) {
                    return Err(conflict("Answer the unresolved questions before accepting this plan."));
                }
            } else if let Some(intake_id) = plan["author"]["intakeId"].as_str() {
                s.assert_intake_live(&s.get_intake(intake_id)?)?;
            }
            if decision == "accept" && let Some(requirements) = plan.get("requirements") {
                    let accepted = s.accept_requirements(task_id,&json!({"revision":task["revision"],"sourceRevision":task["sourceRevision"],"brief":requirements["brief"],"criteria":requirements["criteria"]}),&json!({"origin":"implementation-plan","proposalId":plan_id}))?;
                    value["basis"]["taskRevision"] = accepted["revision"].clone();
                    value["basis"]["requirementsHash"] = json!(crate::requirements_hash(&s.get_task(task_id)?,None));
            }
            s.db.execute("INSERT INTO plan_decisions VALUES(?,?)",params![plan_id,value.to_string()])?;
            if decision == "accept" && let Some(mut queued) = s.open_run(task_id)?
                && crate::dependencies::coding(&queued) && queued["status"]=="awaiting_runner"
                    && s.run_records("assignments",queued["id"].as_str().unwrap())?.is_empty()
                    && queued["implementationPlan"].is_null() {
                    queued["implementationPlan"]=s.current_implementation_plan(task_id)?.unwrap();
                    queued["taskSnapshot"]=s.get_task(task_id)?;
                    queued["taskRevision"]=queued["taskSnapshot"]["revision"].clone();
                    s.save_run(&queued)?;
            }
            if let Some(mut intake) = intake {
                if decision == "accept" {
                    intake["state"] = json!("handoff"); intake["acceptedPlanId"] = json!(plan_id);
                } else {
                    intake["state"] = json!("answer_pending");
                    let run_id = intake["scopingRunId"].as_str().unwrap();
                    let message=s.steer(run_id,&json!({"requestId":key,"body":format!("Plan rejected: {}. Revise it using the retained decisions.", input["reason"].as_str().unwrap())}))?;
                    intake["pendingMessageId"] = message["id"].clone();
                }
                s.save_intake(&mut intake)?;
            }
            s.receipt(key,"plan-decision",&payload,&value)?;
            Ok(value)
        })
    }
}
