//! Run-scoped package policy. Plans describe intent; only admission and writer
//! ownership grant execution. Persisted decisions never infer process activity.
mod policy;
pub use policy::{PackageState, Responsibility, Schedule, ScheduledAssignment};
mod obligations;
mod records;
use super::repository_work::array;
use crate::{
    Result,
    store::{Store, conflict, text},
};
use serde_json::{Value, json};

fn binding(run: &Value) -> Value {
    json!({"planId":run["implementationPlan"]["id"],"planHash":run["implementationPlan"]["sha256"],"requirementRevision":run["taskRevision"]})
}
fn package_mut<'a>(state: &'a mut Value, id: &str) -> Result<&'a mut Value> {
    state["packages"]
        .as_array_mut()
        .and_then(|p| p.iter_mut().find(|p| p["id"] == id))
        .ok_or_else(|| conflict("Package is not in the captured implementation plan."))
}
fn member<'a>(state: &'a Value, name: &Value) -> Result<&'a Value> {
    array(&state["schedule"]["assignments"])
        .iter()
        .find(|m| m["name"] == *name)
        .ok_or_else(|| conflict("Assignment has no choreography responsibility."))
}
impl Store {
    pub fn choreography_current(&self, run_id: &str) -> Result<bool> {
        let run = self.get_run(run_id)?;
        Ok(self
            .choreography(run_id)?
            .is_none_or(|state| state["binding"] == binding(&run)))
    }
    fn choreography_caller(&self, attempt_id: &str) -> Result<(Value, Value, Value)> {
        let attempt = self.get_attempt(attempt_id)?;
        let run = self.get_run(text(&attempt, "runId", 1, 100)?)?;
        let assignment = self.get_assignment(text(&attempt, "assignmentId", 1, 100)?)?;
        if run["purpose"].as_str().unwrap_or("coding") != "coding"
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || !run["sourceConflict"].is_null()
            || attempt["ownership"] != "held"
            || assignment["currentAttemptId"] != attempt_id
            || !assignment["retiredAt"].is_null()
        {
            return Err(conflict(
                "Only the current active assignment can update choreography.",
            ));
        }
        Ok((run, assignment, attempt))
    }
    fn current_choreography(&self, run: &Value, revision: &Value) -> Result<Value> {
        let state = self
            .choreography(text(run, "id", 1, 100)?)?
            .ok_or_else(|| conflict("The coordinator must propose package choreography first."))?;
        if state["revision"] != *revision || state["binding"] != binding(run) {
            return Err(conflict(
                "Choreography or accepted requirements changed; reload before deciding.",
            ));
        }
        Ok(state)
    }
}
mod admission;
mod exchanges;
