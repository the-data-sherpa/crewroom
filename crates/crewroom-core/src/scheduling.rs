//! Durable scheduling decisions. Claiming a slot never starts an agent.
use crate::{
    Result,
    dependencies::coding,
    lifecycle::Transition,
    store::{Store, conflict, id, integer, remove, strict},
};
use rusqlite::params;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
struct DependencyMerge {
    key: String,
    commit: String,
}
impl Store {
    pub(crate) fn readiness_hold(&self, run: &Value) -> Result<Option<Value>> {
        let publication = self
            .record("publications", "run_id", run["id"].as_str().unwrap())?
            .unwrap_or(Value::Null);
        if run["slotRelease"]["reason"] != "review"
            || !["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
            || run["desiredState"] != "paused"
            || publication
                .get("merged")
                .is_some_and(|v| !v.is_null() && v != false)
            || (!run
                .get("sourceConflict")
                .is_some_and(|v| !v.is_null() && v != false)
                && !publication
                    .get("reviewInvalidated")
                    .is_some_and(|v| !v.is_null() && v != false))
        {
            return Ok(None);
        }
        Ok(Some(
            json!({"state":if self.holds_slot(run["id"].as_str().unwrap_or(""))?{"held"}else{"pending"},"candidateId":publication.get("candidateId").unwrap_or(&run["slotRelease"]["candidateId"]),"since":run["slotRelease"]["at"]}),
        ))
    }
    fn slot_reason(&self, run: &Value) -> Result<String> {
        let key = run["taskSnapshot"]["key"].as_str().unwrap_or("");
        if run["status"] == "attention_required" {
            return Ok(format!("{key} needs attention"));
        }
        if self
            .readiness_hold(run)?
            .is_some_and(|h| h["state"] == "held")
        {
            return Ok(format!(
                "{key} readiness is unconfirmed; Resume is explicit"
            ));
        }
        if run["status"] == "paused" || run["status"] == "pausing" {
            let state = run["status"].as_str().unwrap();
            return Ok(match run["pauseReason"].as_str() {
                Some("operator") => format!("{state} by operator"),
                Some("budget") => format!("{state} at the active-time limit"),
                Some("recovery") => format!("{state} for recovery"),
                Some("requirements") => format!("{state} for requirements adoption"),
                Some("source") => format!("{state} for a source conflict"),
                Some("review") => format!("{state} awaiting human review"),
                Some("validation") => format!("{state}; readiness is not confirmed"),
                Some("merged") => format!("PR merged; {state}"),
                _ => format!("{state}, awaiting readiness"),
            });
        }
        Ok(if run["status"] == "running" {
            "running"
        } else {
            "awaiting readiness"
        }
        .into())
    }
    pub fn coding_queue(&self) -> Result<Value> {
        let holders = self.coding_slots()?;
        let priority = self.priority_projects()?;
        let mut plans: Vec<_> = self
            .all("runs")?
            .into_iter()
            .filter(|r| coding(r) && r.get("position").is_some())
            .collect();
        plans.sort_by_key(|r| r["position"].as_u64().unwrap_or(0));
        for plan in &mut plans {
            remove(plan, "holdReason");
            let mut reason = self.eligibility_hold(plan)?;
            if reason.is_none() {
                reason = self.capacity_hold(plan)?;
            }
            if reason.is_none() && priority.contains(&crate::capacity::run_project(plan)) {
                reason = Some(
                    "Waiting for a review-ready run in this project to resume or reclaim its slot first".into(),
                );
            }
            if let Some(reason) = reason {
                plan["holdReason"] = json!(reason);
            }
        }
        let revision: i64 =
            self.db
                .query_row("SELECT revision FROM queue_state WHERE id=1", [], |r| {
                    r.get(0)
                })?;
        let mut slots = Vec::new();
        for holder in &holders {
            slots.push(json!({"runId":holder["id"],"key":holder["taskSnapshot"]["key"],"repositoryId":crate::capacity::run_project(holder),"reason":self.slot_reason(holder)?,"decisionId":holder["schedule"]["id"],"decidedAt":holder["schedule"]["at"]}));
        }
        let mut queue = json!({"revision":revision,"plans":plans,"capacity":self.capacity_view()?});
        if let Some(first) = slots.first() {
            // Compatibility: the earliest holder. `slots` lists every project's.
            queue["slot"] = first.clone();
        }
        queue["slots"] = json!(slots);
        Ok(queue)
    }
    pub fn eligibility_hold(&self, run: &Value) -> Result<Option<String>> {
        if let Some(reason) = self.implementation_plan_hold(run)? {
            return Ok(Some(reason));
        }
        let task = self.get_task(run["taskId"].as_str().unwrap())?;
        let snapshot = &run["taskSnapshot"];
        if run.get("sourceConflict").is_some()
            || task["needsReacceptance"] == true
            || snapshot["sourceRevision"] != task["sourceRevision"]
            || snapshot["brief"] != task["brief"]
            || snapshot["criteria"] != task["criteria"]
        {
            return Ok(Some("Requirements changed".into()));
        }
        let Some(repo_id) = snapshot["repositoryId"].as_str() else {
            return Ok(Some("Needs launch details".into()));
        };
        if self
            .record("repositories", "id", repo_id)?
            .is_none_or(|r| r.get("retiredAt").is_some())
        {
            return Ok(Some("Selected repository unavailable".into()));
        }
        if crate::store::trim_text(snapshot["brief"].as_str().unwrap_or("")).is_empty()
            || snapshot["criteria"].as_array().is_none_or(Vec::is_empty)
        {
            return Ok(Some("Needs a current brief".into()));
        }
        if task["dependsOn"].as_array().is_some_and(|a| !a.is_empty()) {
            if let Some(hold) = self.dependency_hold(&task)? {
                return Ok(Some(hold));
            }
            if run["dependencyBaseHold"]["merges"] == self.dependency_merge_key(run)?
                && let Some(reason) = run["dependencyBaseHold"]["reason"]
                    .as_str()
                    .filter(|s| !s.is_empty())
            {
                return Ok(Some(reason.into()));
            }
        }
        Ok(run["launchHold"]["reason"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                (run.get("launch").is_none() && run.get("startRequest").is_none())
                    .then(|| "Needs launch details".into())
            }))
    }
    fn merge_records(&self, run: &Value) -> Result<Vec<DependencyMerge>> {
        let task = self.get_task(run["taskId"].as_str().unwrap())?;
        let runs = self.all("runs")?;
        let publications = self.all("publications")?;
        let mut result = Vec::new();
        for dependency in task["dependsOn"].as_array().into_iter().flatten() {
            let task = self.get_task(dependency.as_str().unwrap())?;
            let delivered = runs
                .iter()
                .filter(|r| {
                    r["taskId"] == *dependency
                        && coding(r)
                        && r["taskSnapshot"]["repositoryId"] == run["taskSnapshot"]["repositoryId"]
                })
                .find_map(|r| {
                    publications
                        .iter()
                        .find(|p| p["runId"] == r["id"] && p.get("merged").is_some())
                });
            if let Some(p) = delivered {
                result.push(DependencyMerge {
                    key: task["key"].as_str().unwrap().into(),
                    commit: p["merged"]["commit"].as_str().unwrap().into(),
                });
            }
        }
        Ok(result)
    }
    pub fn dependency_merges(&self, run: &Value) -> Result<Value> {
        Ok(serde_json::to_value(self.merge_records(run)?)?)
    }
    fn dependency_merge_key(&self, run: &Value) -> Result<String> {
        Ok(crate::serialized::stringify(&json!(
            self.merge_records(run)?
        )))
    }
    pub fn save_dependency_base_hold(
        &self,
        run_id: &str,
        merges: &str,
        reason: Option<&str>,
    ) -> Result<bool> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run.get("position").is_none() || s.dependency_merge_key(&run)? != merges {
                return Ok(false);
            }
            run["dependencyBaseHold"] = json!({"merges":merges});
            if let Some(reason) = reason {
                run["dependencyBaseHold"]["reason"] = json!(reason);
            }
            s.save_run(&run)?;
            Ok(true)
        })
    }
    pub fn set_launch_hold(
        &self,
        run_id: &str,
        launch: &Value,
        reason: Option<&str>,
    ) -> Result<bool> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run.get("position").is_none()
                || run["launch"] != *launch
                || (run.get("launchHold").is_some() && run["launchHold"]["source"] != "preflight")
            {
                return Ok(false);
            }
            if run["launchHold"]["reason"].as_str() == reason {
                return Ok(true);
            }
            remove(&mut run, "launchHold");
            if let Some(reason) = reason.filter(|v| !v.is_empty()) {
                run["launchHold"] = json!({"reason":reason,"at":s.time(),"source":"preflight"});
            }
            s.save_run(&run)?;
            Ok(true)
        })
    }
    /// FOR-76: succeeds when the run holds a slot or its project and the
    /// workspace have room; otherwise the conflict carries the queue reason.
    pub fn assert_coding_slot_available(&self, run_id: &str) -> Result<()> {
        let run = self.get_run(run_id)?;
        if self.holds_slot(run_id)? {
            return Ok(());
        }
        if let Some(reason) = self.capacity_hold(&run)? {
            return Err(conflict(&reason));
        }
        Ok(())
    }
    /// Projects whose released, review-ready runs wait to resume or to
    /// reclaim a slot. They keep priority over new plans in that project.
    fn priority_projects(&self) -> Result<std::collections::HashSet<String>> {
        let mut projects = std::collections::HashSet::new();
        for run in self.all("runs")? {
            if !coding(&run) || self.holds_slot(run["id"].as_str().unwrap_or(""))? {
                continue;
            }
            let resuming = run.get("resumeRequest").is_some_and(|v| !v.is_null())
                && ["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
                && run["desiredState"] != "cancelled";
            let rehold = run["feedbackResumeHold"]["terminal"] != true
                && run["feedbackContinuations"]
                    .as_array()
                    .and_then(|a| a.last())
                    .is_none_or(|v| v["state"] != "blocked")
                && self.readiness_hold(&run)?.is_some();
            if resuming || rehold {
                projects.insert(crate::capacity::run_project(&run));
            }
        }
        Ok(projects)
    }
    /// Claims a slot for the first eligible queued plan whose project and the
    /// workspace have room, in queue order. Call again to fill further slots.
    pub fn claim_next_coding_slot(&self) -> Result<Option<Value>> {
        self.transaction(|s| {
            let priority = s.priority_projects()?;
            for plan in s.coding_queue()?["plans"].as_array().unwrap() {
                if priority.contains(&crate::capacity::run_project(plan)) {
                    continue;
                }
                if s.eligibility_hold(plan)?.is_none() && s.capacity_hold(plan)?.is_none() {
                    return s
                        .claim_coding_slot_in_transaction(plan["id"].as_str().unwrap(), false)
                        .map(Some);
                }
            }
            Ok(None)
        })
    }
    pub fn claim_coding_slot(&self, run_id: &str, manual: bool) -> Result<Value> {
        self.transaction(|s| s.claim_coding_slot_in_transaction(run_id, manual))
    }
    fn claim_coding_slot_in_transaction(&self, run_id: &str, manual: bool) -> Result<Value> {
        if let Some(reason) = self.implementation_plan_hold(&self.get_run(run_id)?)? {
            return Err(conflict(&reason));
        }
        let mut run = self.get_run(run_id)?;
        if !coding(&run) || run["status"] != "awaiting_runner" || run["desiredState"] == "cancelled"
        {
            return Err(conflict("Coding run is not available to start."));
        }
        self.assert_coding_slot_available(run_id)?;
        if run.get("slotRelease").is_some() {
            return Err(conflict(
                "Review-ready work needs an explicit Resume; a new start cannot reclaim its released slot.",
            ));
        }
        if run.get("position").is_some()
            && let Some(hold) = self.eligibility_hold(&run)?
            && (!manual || (hold != "Needs launch details" && run["launchHold"]["reason"] != hold))
        {
            return Err(conflict(&hold));
        }
        remove(&mut run, "launchHold");
        if run.get("schedule").is_none() {
            let mut schedule = json!({"id":id(),"at":self.time()});
            if let Some(position) = run.get("position") {
                schedule["position"] = position.clone();
            }
            run["schedule"] = schedule;
            self.save_run(&run)?;
            run = self.get_run(run_id)?;
        }
        self.occupy_slot(run_id)?;
        Ok(run)
    }
    pub fn release_coding_slot(
        &self,
        run_id: &str,
        reason: &str,
        candidate_id: Option<&str>,
    ) -> Result<Value> {
        self.transaction(|s| s.release_coding_slot_in_transaction(run_id, reason, candidate_id))
    }
    pub(crate) fn release_coding_slot_in_transaction(
        &self,
        run_id: &str,
        reason: &str,
        candidate_id: Option<&str>,
    ) -> Result<Value> {
        if !["review", "cancelled", "merged"].contains(&reason) {
            return Err(crate::store::invalid("Invalid coding slot release reason."));
        }
        let run = self.get_run(run_id)?;
        if run["purpose"] == "evaluation" {
            return Ok(run); // Only a stopped, committed comparison result releases this slot.
        }
        if !self.holds_slot(run_id)? {
            return Ok(run);
        }
        let publication = self
            .record("publications", "run_id", run_id)?
            .unwrap_or(Value::Null);
        if reason == "review"
            && (candidate_id.is_none()
                || publication["ready"] != true
                || publication["candidateId"].as_str() != candidate_id)
        {
            return Err(conflict(
                "Current publication readiness is required for review release.",
            ));
        }
        if reason == "review" || reason == "merged" {
            if reason == "review"
                && run["status"] == "paused"
                && run["desiredState"] == "paused"
                && ["review", "validation"].contains(&run["pauseReason"].as_str().unwrap_or(""))
            {
                self.change_run(run_id, Transition::ReviewRestored)?;
            } else {
                self.change_run(
                    run_id,
                    Transition::Desire {
                        desired_state: "paused".into(),
                    },
                )?;
                self.change_run(
                    run_id,
                    Transition::BeginPause {
                        reason: Some(reason.into()),
                    },
                )?;
            }
        }
        let mut updated = self.get_run(run_id)?;
        updated["slotRelease"] = json!({"reason":reason,"at":self.time()});
        if let Some(candidate) = candidate_id.filter(|v| !v.is_empty()) {
            updated["slotRelease"]["candidateId"] = json!(candidate);
        }
        self.save_run(&updated)?;
        self.vacate_slot(run_id)?;
        self.get_run(run_id)
    }
    pub fn defer_unadmitted_launch(
        &self,
        run_id: &str,
        reason: Option<&str>,
        dependency: bool,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if !s.holds_slot(run_id)?
                || run["status"] != "awaiting_runner"
                || s.record("repository_work", "run_id", run_id)?.is_some()
                || !s.run_records("assignments", run_id)?.is_empty()
                || ["cancelled", "paused"].contains(&run["desiredState"].as_str().unwrap_or(""))
            {
                return Err(conflict(
                    "Cannot defer a coding run after preparation or admission.",
                ));
            }
            s.vacate_slot(run_id)?;
            let queued = s.coding_queue()?;
            let plans = queued["plans"].as_array().unwrap();
            let position = run["schedule"]["position"]
                .as_u64()
                .unwrap_or(plans.len() as u64)
                .min(plans.len() as u64);
            for plan in plans
                .iter()
                .filter(|p| p["position"].as_u64().unwrap() >= position)
            {
                let mut plan = plan.clone();
                plan["position"] = json!(plan["position"].as_u64().unwrap() + 1);
                s.db.execute(
                    "UPDATE runs SET data=? WHERE id=?",
                    params![plan.to_string(), plan["id"].as_str()],
                )?;
            }
            remove(&mut run, "schedule");
            run["position"] = json!(position);
            if dependency {
                run["dependencyBaseHold"] = json!({"merges":s.dependency_merge_key(&run)?});
                if let Some(reason) = reason {
                    run["dependencyBaseHold"]["reason"] = json!(reason);
                }
            } else {
                remove(&mut run, "launchHold");
                if let Some(reason) = reason.filter(|v| !v.is_empty()) {
                    run["launchHold"] =
                        json!({"reason":reason,"at":s.time(),"source":"preparation"});
                }
            }
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn can_replay_initial_launch(
        &self,
        run_id: &str,
        decision: &str,
        revision: u64,
    ) -> Result<bool> {
        let run = self.get_run(run_id)?;
        if !self.holds_slot(run_id)?
            || run["schedule"]["id"] != decision
            || run.get("launchCompletedAt").is_some()
            || run.get("launch").is_none()
            || run["status"] != "awaiting_runner"
            || ["paused", "cancelled"].contains(&run["desiredState"].as_str().unwrap_or(""))
            || run.get("sourceConflict").is_some()
            || run["controlRevision"].as_u64().unwrap_or(0) != revision
        {
            return Ok(false);
        }
        for assignment in self.run_records("assignments", run_id)? {
            if self.get_attempt(assignment["currentAttemptId"].as_str().unwrap())?["state"]
                != "admitted"
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn normalize_launch_execution(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run.get("schedule").is_none()
                || run.get("launchCompletedAt").is_some()
                || !coding(&run)
            {
                return Ok(run);
            }
            let started = s
                .agent_events(run_id)?
                .iter()
                .any(|e| e["event"]["type"] == "agent_start");
            if run["status"] != "running"
                && run.get("activeSince").is_none()
                && run["activeMs"].as_u64().unwrap_or(0) == 0
                && !started
            {
                return Ok(run);
            }
            run["launchCompletedAt"] = json!(s.time());
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn restart_scheduled_launch(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s|{
            let mut run=s.get_run(run_id)?;let runtime=s.runtime(run_id)?;
            if run.get("schedule").is_none() || run.get("launchCompletedAt").is_some() || run.get("launch").is_none() || run["status"]!="paused" || run["pauseReason"]!="recovery" || run["desiredState"]!="running"
                || runtime["attempts"].as_array().unwrap().iter().any(|a|a["state"]!="admitted" && a["ownership"]!="stopped") {return Err(conflict("Interrupted launch is not safely stopped."));}
            for assignment in runtime["assignments"].as_array().unwrap() {
                let old=s.get_attempt(assignment["currentAttemptId"].as_str().unwrap())?;
                if old["state"]=="admitted" {continue;}
                let mut assignment=assignment.clone();
                let attempt=json!({"id":id(),"assignmentId":assignment["id"],"runId":run_id,"generation":old["generation"].as_u64().unwrap()+1,"input":old["input"],"state":"admitted","createdAt":s.time()});
                s.db.execute("INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)",params![attempt["id"].as_str(),assignment["id"].as_str(),attempt.to_string()])?;
                assignment["currentAttemptId"]=attempt["id"].clone();s.save_assignment(&assignment)?;
            }
            run["status"]=json!("awaiting_runner");remove(&mut run,"pauseReason");s.save_run(&run)?;s.get_run(run_id)
        })
    }
    pub fn reclaim_coding_slot(&self, run_id: &str, regressed: bool) -> Result<()> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if !coding(&run) || run.get("slotRelease").is_none() {
                return Ok(());
            }
            if s.holds_slot(run_id)? {
                return Ok(());
            }
            if let Some(reason) = s.capacity_hold(&run)? {
                return Err(conflict(&format!(
                    "{reason}. Resume this run when a slot is free."
                )));
            }
            if regressed {
                run = s.change_run(run_id, Transition::ReviewRegressed)?;
            }
            run["schedule"] = json!({"id":id(),"at":s.time()});
            if ["migration", "paused"]
                .contains(&run["slotRelease"]["reason"].as_str().unwrap_or(""))
            {
                remove(&mut run, "slotRelease");
            }
            s.save_run(&run)?;
            s.occupy_slot(run_id)?;
            Ok(())
        })
    }
    /// FOR-76: a coding run that is paused (desired and actual state) with
    /// every agent's process ownership confirmed stopped frees its project
    /// slot, so the next queued ticket can be admitted. Unconfirmed ownership
    /// keeps the slot. Review-gate pauses (`review`, `validation`, `merged`)
    /// are governed by their own release, and a comparison keeps its slot
    /// until its result is committed. The run stays resumable: an explicit
    /// Resume reclaims a slot, or waits with priority in its project when the
    /// project is full. Returns whether the slot was released.
    pub fn release_paused_coding_slot(&self, run_id: &str) -> Result<bool> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if !coding(&run)
                || !s.holds_slot(run_id)?
                || run["status"] != "paused"
                || run["desiredState"] != "paused"
                || run.get("resumeRequest").is_some_and(|v| !v.is_null())
                || ["review", "validation", "merged"]
                    .contains(&run["pauseReason"].as_str().unwrap_or(""))
            {
                return Ok(false);
            }
            let runtime = s.runtime(run_id)?;
            let attempts = runtime["attempts"].as_array().unwrap();
            if attempts
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            {
                return Ok(false);
            }
            run["slotRelease"] = json!({"reason":"paused","at":s.time()});
            s.save_run(&run)?;
            s.vacate_slot(run_id)?;
            Ok(true)
        })
    }
    pub fn set_desired_state(&self, run_id: &str, desired: &str) -> Result<Value> {
        self.transaction(|s| {
            if desired == "running" {
                s.assert_evaluation_dispatch(run_id)?;
            }
            let run = s.change_run(
                run_id,
                Transition::Desire {
                    desired_state: desired.into(),
                },
            )?;
            if desired == "cancelled" {
                s.release_coding_slot_in_transaction(run_id, "cancelled", None)?;
            }
            Ok(run)
        })
    }
    /// Every explicit stop fences earlier controls, including a repeated Pause.
    pub fn request_pause(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let previous = s.get_run(run_id)?;
            let mut run = s.set_desired_state(run_id, "paused")?;
            if run["controlRevision"] == previous["controlRevision"] {
                run["controlRevision"] = json!(run["controlRevision"].as_u64().unwrap_or(0) + 1);
                s.save_run(&run)?;
            }
            Ok(run)
        })
    }
    pub fn prepare_continuation(&self, run_id: &str, revision: u64) -> Result<Value> {
        self.transaction(|s| {
            for assignment in s.run_records("assignments", run_id)? {
                if assignment.get("retiredAt").is_none()
                    && assignment.get("rebriefRequired").is_none()
                    && s.get_attempt(assignment["currentAttemptId"].as_str().unwrap())?["state"]
                        != "admitted"
                {
                    return Err(conflict("An existing attempt still owns this assignment."));
                }
            }
            s.change_run(
                run_id,
                Transition::Continue {
                    control_revision: revision,
                },
            )
        })
    }
    pub fn require_attention(&self, run_id: &str, reason: &str) -> Result<Value> {
        self.transaction(|s| {
            s.change_run(
                run_id,
                Transition::Attention {
                    reason: reason.into(),
                },
            )
        })
    }
    pub fn start_execution(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            s.assert_evaluation_dispatch(run_id)?;
            s.change_run(run_id, Transition::Start)
        })
    }
    pub fn set_active_limit(&self, run_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["activeLimitMs"])?;
        let limit = integer(input, "activeLimitMs", 1000, u64::MAX)?;
        self.transaction(|s| {
            if s.get_run(run_id)?["purpose"] == "evaluation" {
                return Err(conflict(
                    "Evaluation active limits are immutable; create a new comparison.",
                ));
            }
            s.change_run(
                run_id,
                Transition::SetLimit {
                    active_limit_ms: limit,
                },
            )
        })
    }
}
