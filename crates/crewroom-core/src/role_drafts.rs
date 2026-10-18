//! A bounded Explore process drafts a proposal; only an operator can save the role.
use crate::{
    Result,
    roles::validate_role,
    store::{Store, conflict, id, request_id, strict, trimmed_text},
};
use serde_json::{Value, json};
impl Store {
    pub fn queue_role_draft(&self, input: &Value) -> Result<Value> {
        strict(input, &["description", "requestId"])?;
        let key = request_id(input)?;
        let description = trimmed_text(input, "description", 1, 4000)?;
        self.transaction(|s| {
            if let Some(run)=s.all("runs")?.into_iter().find(|r|r["roleDraft"]["requestId"]==key) {
                if run["roleDraft"]["description"]!=description {return Err(conflict("Draft request identity has different content."));}
                return Ok(run);
            }
            let task=s.create_task(&json!({"title":"Draft a new specialist","description":description}))?;
            let task=s.refine(task["id"].as_str().unwrap(),&json!({"revision":1,"sourceRevision":1,"brief":description,"criteria":["Propose a reusable specialist role for operator review."]}))?;
            let mut run=s.create_alternate_run(task,"explore",300_000,Some(key))?;
            run["roleDraft"]=json!({"requestId":key,"description":description,"roleId":format!("specialist-{}",id())});
            s.save_run(&run)?;
            Ok(run)
        })
    }
    pub fn role_draft_proposal(&self, proposal_id: &str) -> Result<Value> {
        let p = self
            .refinement_proposals(None)?
            .into_iter()
            .find(|p| p["id"] == proposal_id)
            .ok_or_else(|| conflict("Draft proposal not found."))?;
        if !p["roleDraft"].is_object() {
            return Err(conflict("This is not a specialist draft."));
        }
        Ok(p)
    }
}
pub fn parse_proposal(run: &Value, body: &Value) -> Result<Value> {
    let mut value: Value = serde_json::from_str(
        body["brief"]
            .as_str()
            .ok_or_else(|| conflict("Draft brief must contain a JSON role."))?,
    )
    .map_err(|_| conflict("Draft brief must contain valid role JSON without Markdown fences."))?;
    if !value.is_object() {
        return Err(conflict("Draft role must be an object."));
    }
    value["id"] = run["roleDraft"]["roleId"].clone();
    value["model"] = json!("");
    value["effort"] = json!("default");
    value["enabled"] = json!(true);
    value["principles"] = json!([]);
    value.as_object_mut().unwrap().remove("revision");
    if !crate::model_jobs::JOBS.contains(&value["job"].as_str().unwrap_or("")) {
        return Err(conflict(
            "Draft role needs a Code, Judgment, Review or Fast job.",
        ));
    }
    validate_role(&value)
}
