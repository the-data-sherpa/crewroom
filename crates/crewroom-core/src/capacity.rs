//! FOR-76: per-project run capacity with an optional workspace ceiling, and
//! the atomic ticket claim.
//!
//! A project is a configured repository; a run belongs to the project of its
//! ticket's target repository. Each project admits up to its capacity
//! (default 1) of coding runs. An optional workspace ceiling bounds the total.
//! A run occupies capacity while it holds a slot (until review release,
//! merge, cancellation, or a pause that confirms every agent stopped) or
//! while any of its agents' process ownership is not confirmed stopped, so a
//! paused run whose ownership is unconfirmed keeps its slot. A start over capacity is queued with the reason rather than
//! refused, and the scheduler admits the next queued plan when a slot frees.
//!
//! The ticket claim is atomic: `coding_slots.task_id` is unique, so two runs
//! can never hold a slot for one ticket, and an attempt is never claimed while
//! another run of the same ticket still has a live agent. Both conflicts are
//! final; recovery re-attaches to the existing run instead.
//!
//! Comparison (evaluation) runs keep the whole workspace to themselves so
//! their measurements are not shared with other work.
use crate::{
    Result,
    store::{Store, conflict, integer, invalid, parse, strict},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

/// Largest accepted per-project capacity and workspace ceiling.
pub const MAX_CAPACITY: u64 = 16;

pub(crate) fn run_project(run: &Value) -> String {
    run["taskSnapshot"]["repositoryId"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}
fn key(run: &Value) -> &str {
    run["taskSnapshot"]["key"].as_str().unwrap_or("")
}
fn evaluation(run: &Value) -> bool {
    run["purpose"] == "evaluation"
}

impl Store {
    /// `{"workspaceCeiling": n | null, "projects": {repositoryId: n}}`.
    pub fn execution_settings(&self) -> Result<Value> {
        let data: Option<String> = self
            .db
            .query_row("SELECT data FROM execution_settings WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?;
        let mut value = data.map(parse).transpose()?.unwrap_or_else(|| json!({}));
        if !value["projects"].is_object() {
            value["projects"] = json!({});
        }
        if !value["workspaceCeiling"].is_u64() {
            value["workspaceCeiling"] = Value::Null;
        }
        Ok(value)
    }
    pub fn project_capacity(&self, project: &str) -> Result<u64> {
        Ok(self.execution_settings()?["projects"][project]
            .as_u64()
            .unwrap_or(1))
    }
    pub fn workspace_ceiling(&self) -> Result<Option<u64>> {
        Ok(self.execution_settings()?["workspaceCeiling"].as_u64())
    }
    /// Sets one project's capacity (`repositoryId` + `capacity`) and/or the
    /// workspace ceiling (`workspaceCeiling`, `null` removes it). Lowering a
    /// value never stops running work; it only delays new admissions.
    pub fn set_execution_capacity(&self, body: &Value) -> Result<Value> {
        strict(body, &["repositoryId", "capacity", "workspaceCeiling"])?;
        if body.get("repositoryId").is_some() != body.get("capacity").is_some() {
            return Err(invalid("repositoryId and capacity must be given together."));
        }
        if body.get("repositoryId").is_none() && body.get("workspaceCeiling").is_none() {
            return Err(invalid("Nothing to change."));
        }
        self.transaction(|s| {
            let mut settings = s.execution_settings()?;
            if let Some(repo) = body.get("repositoryId") {
                let repo = repo
                    .as_str()
                    .ok_or_else(|| invalid("repositoryId must be a string."))?;
                if s
                    .record("repositories", "id", repo)?
                    .is_none_or(|r| r.get("retiredAt").is_some())
                {
                    return Err(crate::Error::NotFound("Repository not found.".into()));
                }
                let capacity = integer(body, "capacity", 1, MAX_CAPACITY)?;
                if capacity == 1 {
                    settings["projects"].as_object_mut().unwrap().remove(repo);
                } else {
                    settings["projects"][repo] = json!(capacity);
                }
            }
            if let Some(ceiling) = body.get("workspaceCeiling") {
                settings["workspaceCeiling"] = if ceiling.is_null() {
                    Value::Null
                } else {
                    json!(integer(body, "workspaceCeiling", 1, MAX_CAPACITY)?)
                };
            }
            s.db.execute(
                "INSERT INTO execution_settings(id,data) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                [settings.to_string()],
            )?;
            s.execution_settings()
        })
    }
    /// Every run holding a slot, in claim order.
    pub fn coding_slots(&self) -> Result<Vec<Value>> {
        let mut statement = self
            .db
            .prepare("SELECT run_id FROM coding_slots ORDER BY rowid")?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| self.get_run(id)).collect()
    }
    /// The earliest-claimed slot holder. Several projects may each hold
    /// slots; use [`Store::coding_slots`] for all of them.
    pub fn coding_slot(&self) -> Result<Option<Value>> {
        Ok(self.coding_slots()?.into_iter().next())
    }
    pub fn holds_slot(&self, run_id: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM coding_slots WHERE run_id=?)",
            [run_id],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn occupy_slot(&self, run_id: &str) -> Result<()> {
        let run = self.get_run(run_id)?;
        if self.holds_slot(run_id)? {
            return Ok(());
        }
        let task = run["taskId"].as_str().unwrap_or("");
        let other: Option<String> = self
            .db
            .query_row(
                "SELECT run_id FROM coding_slots WHERE task_id=?",
                [task],
                |r| r.get(0),
            )
            .optional()?;
        if other.is_some() {
            return Err(conflict(&format!(
                "{} already holds a slot in another run. A ticket has at most one live coordinator; recover that run instead of starting another.",
                key(&run)
            )));
        }
        self.db.execute(
            "INSERT INTO coding_slots(run_id,task_id,project) VALUES(?,?,?)",
            params![run_id, task, run_project(&run)],
        )?;
        Ok(())
    }
    pub(crate) fn vacate_slot(&self, run_id: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM coding_slots WHERE run_id=?", [run_id])?;
        Ok(())
    }
    /// Coding or evaluation runs, other than `except`, whose agents' process
    /// ownership is not confirmed stopped.
    pub(crate) fn unstopped_coding_runs(&self, except: &str) -> Result<Vec<Value>> {
        let mut statement = self.db.prepare("SELECT DISTINCT s.run_id FROM attempts a JOIN assignments s ON a.assignment_id=s.id JOIN runs r ON r.id=s.run_id WHERE s.run_id<>? AND COALESCE(json_extract(r.data,'$.purpose'),'coding') IN ('coding','evaluation') AND a.state<>'admitted' AND a.ownership IS NOT 'stopped' ORDER BY r.rowid")?;
        let ids = statement
            .query_map([except], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| self.get_run(id)).collect()
    }
    /// Runs other than `except` that use capacity: slot holders and runs with
    /// unconfirmed-stopped coding ownership.
    pub(crate) fn occupants(&self, except: &str) -> Result<Vec<Value>> {
        let mut result: Vec<Value> = self
            .coding_slots()?
            .into_iter()
            .filter(|r| r["id"] != except)
            .collect();
        for run in self.unstopped_coding_runs(except)? {
            if !result.iter().any(|r| r["id"] == run["id"]) {
                result.push(run);
            }
        }
        Ok(result)
    }
    fn project_name(&self, project: &str) -> Result<String> {
        Ok(self
            .record("repositories", "id", project)?
            .and_then(|r| r["name"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "this project".into()))
    }
    /// Why `run` cannot take a slot now, measured against `others`.
    fn capacity_reason(&self, run: &Value, others: &[Value]) -> Result<Option<String>> {
        if let Some(evaluation) = others.iter().find(|r| evaluation(r)) {
            return Ok(Some(format!(
                "Waiting for the comparison run {} to finish; comparisons run alone",
                key(evaluation)
            )));
        }
        if evaluation(run) && !others.is_empty() {
            return Ok(Some(format!(
                "Waiting for {} running ticket(s) to stop; comparisons run alone",
                others.len()
            )));
        }
        if let Some(other) = others.iter().find(|r| r["taskId"] == run["taskId"]) {
            return Ok(Some(format!(
                "{} already has a live coordinator in run {}; recover that run instead of starting another",
                key(run),
                other["id"].as_str().unwrap_or("")
            )));
        }
        let project = run_project(run);
        let capacity = self.project_capacity(&project)?;
        let running = others.iter().filter(|r| run_project(r) == project).count() as u64;
        if running >= capacity {
            return Ok(Some(format!(
                "Waiting for a slot in {}: {running} of {capacity} running",
                self.project_name(&project)?
            )));
        }
        if let Some(ceiling) = self.workspace_ceiling()?
            && others.len() as u64 >= ceiling
        {
            return Ok(Some(format!(
                "Waiting for a workspace slot: {} of {ceiling} running",
                others.len()
            )));
        }
        Ok(None)
    }
    /// `None` when `run` may take (or already holds) a slot.
    pub fn capacity_hold(&self, run: &Value) -> Result<Option<String>> {
        let others = self.occupants(run["id"].as_str().unwrap_or(""))?;
        self.capacity_reason(run, &others)
    }
    /// The attempt-level guard (checked under the store lock when an attempt
    /// is claimed, and by the supervisor's in-memory admission reservation):
    /// another run of this ticket with a live agent is a hard conflict, and a
    /// run that uses repository capacity must fit its project and the
    /// workspace among the runs that are executing or being admitted.
    pub fn execution_conflict(&self, run_id: &str, admitting: &[String]) -> Result<Option<String>> {
        let run = self.get_run(run_id)?;
        let live: Vec<String> = {
            let mut statement = self.db.prepare("SELECT DISTINCT s.run_id FROM attempts a JOIN assignments s ON a.assignment_id=s.id JOIN runs r ON r.id=s.run_id WHERE r.task_id=? AND s.run_id<>? AND a.state<>'admitted' AND a.ownership IS NOT 'stopped'")?;
            statement
                .query_map(params![run["taskId"].as_str().unwrap_or(""), run_id], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if let Some(other) = live.first() {
            return Ok(Some(format!(
                "{} already has a live coordinator in run {other}. A ticket has at most one; recover that run instead of starting another.",
                key(&run)
            )));
        }
        let uses_capacity = |id: &str, run: &Value| -> Result<bool> {
            Ok(crate::dependencies::coding(run)
                || evaluation(run)
                || self.record("repository_work", "run_id", id)?.is_some())
        };
        if !uses_capacity(run_id, &run)? {
            return Ok(None);
        }
        let mut others = Vec::new();
        for item in self.executing_runs()?.as_array().unwrap() {
            let id = item["runId"].as_str().unwrap_or("");
            if id != run_id && item["coding"] == true {
                others.push(self.get_run(id)?);
            }
        }
        for id in admitting.iter().filter(|id| id.as_str() != run_id) {
            if others.iter().any(|r| r["id"] == id.as_str()) {
                continue;
            }
            let Ok(other) = self.get_run(id) else {
                continue;
            };
            if evaluation(&other) || self.record("repository_work", "run_id", id)?.is_some() {
                others.push(other);
            }
        }
        self.capacity_reason(&run, &others)
    }
    /// FOR-76: a manual start while the run's project (or the workspace) is
    /// at capacity is queued, not refused. The validated start input is kept
    /// on the queued plan; the scheduler dispatches it when a slot frees,
    /// after re-checking readiness.
    pub fn queue_start_request(&self, run_id: &str, input: &Value) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if !crate::dependencies::coding(&run)
                || run.get("position").is_none()
                || run["status"] != "awaiting_runner"
                || run["desiredState"] == "cancelled"
                || !s.run_records("assignments", run_id)?.is_empty()
            {
                return Err(conflict(
                    "Only an unadmitted queued coding plan can wait for a slot.",
                ));
            }
            run["startRequest"] = json!({"input":input,"requestedAt":s.time()});
            crate::store::remove(&mut run, "launchHold");
            s.save_run(&run)?;
            let mut run = s.get_run(run_id)?;
            if let Some(reason) = s.capacity_hold(&run)? {
                run["holdReason"] = json!(reason);
            }
            Ok(run)
        })
    }
    /// A slot holder whose queued manual start has not admitted any agent.
    pub fn can_dispatch_start_request(&self, run_id: &str) -> Result<bool> {
        let run = self.get_run(run_id)?;
        Ok(self.holds_slot(run_id)?
            && run.get("startRequest").is_some()
            && run.get("launch").is_none()
            && run["status"] == "awaiting_runner"
            && !["paused", "cancelled"].contains(&run["desiredState"].as_str().unwrap_or(""))
            && self.run_records("assignments", run_id)?.is_empty())
    }
    /// Per-project occupancy for the queue view.
    pub(crate) fn capacity_view(&self) -> Result<Value> {
        let occupants = self.occupants("")?;
        let mut projects = Vec::new();
        for repo in self.all("repositories")? {
            if repo.get("retiredAt").is_some() {
                continue;
            }
            let id = repo["id"].as_str().unwrap_or("");
            projects.push(json!({
                "repositoryId": id,
                "name": repo["name"],
                "capacity": self.project_capacity(id)?,
                "occupied": occupants.iter().filter(|r| run_project(r) == id).count(),
            }));
        }
        Ok(json!({
            "workspaceCeiling": self.workspace_ceiling()?,
            "occupied": occupants.len(),
            "projects": projects,
        }))
    }
}
