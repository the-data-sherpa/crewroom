//! Durable run planning. These transactions do not admit or launch a process.
use crate::{
    Result,
    dependencies::coding,
    lifecycle::{Transition, transition},
    store::{Store, conflict, id, integer, invalid, remove, request_id, strict, text},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

impl Store {
    pub(crate) fn save_run(&self, value: &Value) -> Result<()> {
        let mut run = value.clone();
        let run_id = run["id"].as_str().unwrap().to_owned();
        let before = self.get_run(&run_id)?;
        let assigned: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM assignments WHERE run_id=?)",
            [&run_id],
            |r| r.get(0),
        )?;
        let eligible = coding(&run)
            && run.get("schedule").is_none()
            && run["status"] == "awaiting_runner"
            && !assigned;
        if !eligible {
            remove(&mut run, "position");
        } else if run.get("position").is_none() {
            run["position"] = json!(self.queue_view()?["plans"].as_array().unwrap().len());
        }
        self.db.execute(
            "UPDATE runs SET data=? WHERE id=?",
            params![run.to_string(), run_id],
        )?;
        if run["status"] == "cancelled"
            && let Some(parent_id) = run["continuedFrom"]["runId"].as_str()
            && self.record("repository_work", "run_id", &run_id)?.is_none()
        {
            let mut parent = self.get_run(parent_id)?;
            let transferred = self
                .record("repository_work", "run_id", parent_id)?
                .is_some_and(|w| w.get("transferredTo").is_some());
            if parent["continuedBy"] == run_id && !transferred {
                remove(&mut parent, "continuedBy");
                self.save_run(&parent)?;
            }
        }
        let ended = run["desiredState"] == "cancelled"
            || ["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""));
        if ended
            && run["desiredState"] == "cancelled"
            && run["status"] != "closed"
            && let Some(intake_id) = run["intakeId"].as_str()
        {
            let mut intake = self.get_intake(intake_id)?;
            if !matches!(intake["state"].as_str(), Some("handed_off" | "cancelled"))
                && intake["scopingRunId"] == run_id
            {
                intake["state"] = json!("cancelled");
                intake["revision"] = json!(intake["revision"].as_u64().unwrap_or(0) + 1);
                self.db.execute(
                    "UPDATE intake_requests SET data=? WHERE id=?",
                    params![intake.to_string(), intake_id],
                )?;
            }
        }
        if ended {
            let reason = match run["status"].as_str() {
                Some("completed") => {
                    "Research completed before delivery. This message will not be sent if the run reopens."
                }
                Some("closed") => "The run closed before delivery.",
                _ => "Cancellation was requested before delivery.",
            };
            for mut message in self
                .all("steering")?
                .into_iter()
                .filter(|m| m["runId"] == run_id && m["delivery"] == "pending")
            {
                self.transition_steering(&mut message, "not_delivered", Some(reason))?;
            }
            for mut proposal in self.all("team_proposals")?.into_iter().filter(|p| {
                p["runId"] == run_id && p["origin"] == "operator" && p["status"] == "pending"
            }) {
                proposal["status"] = json!("rejected");
                proposal["reason"] = json!("The run ended before this specialist was admitted.");
                self.db.execute(
                    "UPDATE team_proposals SET data=? WHERE id=?",
                    params![proposal.to_string(), proposal["id"].as_str()],
                )?;
            }
        }
        match (before["position"].as_u64(), run["position"].as_u64()) {
            (Some(old), None) => {
                for mut plan in self.queue_view()?["plans"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|p| p["position"].as_u64().is_some_and(|p| p > old))
                    .cloned()
                {
                    plan["position"] = json!(plan["position"].as_u64().unwrap() - 1);
                    self.db.execute(
                        "UPDATE runs SET data=? WHERE id=?",
                        params![plan.to_string(), plan["id"].as_str()],
                    )?;
                }
                self.bump_queue()?;
            }
            (None, Some(_)) => self.bump_queue()?,
            _ => (),
        }
        Ok(())
    }

    pub(crate) fn create_alternate_run(
        &self,
        task: Value,
        purpose: &str,
        limit: u64,
        key: Option<&str>,
    ) -> Result<Value> {
        let mut run = json!({"id":id(),"taskId":task["id"],"purpose":purpose,"status":"awaiting_runner","createdAt":self.time(),
            "taskRevision":task["revision"],"taskSnapshot":task,"roles":[],"stages":[],"activeMs":0,"activeLimitMs":limit});
        if let Some(key) = key {
            run["requestId"] = json!(key);
        }
        self.db.execute(
            "INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",
            params![run["id"].as_str(), run["taskId"].as_str(), run.to_string()],
        )?;
        Ok(run)
    }

    pub(crate) fn alternate_replay(&self, task_id: &str, key: &str) -> Result<Option<Value>> {
        self.db
            .query_row(
                "SELECT data FROM runs WHERE task_id=? AND json_extract(data,'$.requestId')=?",
                params![task_id, key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(crate::store::parse)
            .transpose()
    }

    pub fn queue_refinement(&self, task_id: &str, body: &Value) -> Result<Value> {
        strict(body, &["revision", "purpose", "requestId"])?;
        let revision = integer(body, "revision", 1, u64::MAX)?;
        let purpose = text(body, "purpose", 1, 20)?;
        if !["explore", "grill"].contains(&purpose) {
            return Err(invalid("Choose explore or grill."));
        }
        let key = body
            .get("requestId")
            .map(|_| request_id(body))
            .transpose()?;
        self.transaction(|s| {
            let task = s.get_task(task_id)?;
            crate::store::operator_task(&task)?;
            if let Some(key) = key
                && let Some(prior) = s.alternate_replay(task_id, key)?
            {
                if prior["purpose"] != purpose || prior["taskRevision"] != revision {
                    return Err(conflict(
                        "Refinement request identity has different content.",
                    ));
                }
                return Ok(prior);
            }
            if task["revision"] != revision {
                return Err(conflict("The brief changed. Reload before refining."));
            }
            if s.open_run(task_id)?.is_some() {
                return Err(conflict("Close the open run before starting another."));
            }
            s.create_alternate_run(task, purpose, 3_600_000, key)
        })
    }

    pub fn close_refinement(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let run = s.get_run(run_id)?;
            if run["status"] == "closed" {
                return Ok(run);
            }
            if let Some(id) = run["intakeId"].as_str() {
                let intake = s.get_intake(id)?;
                if !["handoff", "cancelled"].contains(&intake["state"].as_str().unwrap_or("")) {
                    return Err(conflict(
                        "Cancel the queued intake or accept its plan before closing scoping.",
                    ));
                }
            }
            if s.runtime(run_id)?["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            {
                return Err(conflict(
                    "Process ownership must be confirmed stopped before closing.",
                ));
            }
            let run = transition(&run, &Transition::Close, s.clock_ms())?;
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
}
