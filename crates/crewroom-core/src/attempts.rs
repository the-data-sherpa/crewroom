//! Controller-owned assignment and attempt receipts. No method launches a process.
use crate::{
    Error, Result,
    lifecycle::{Transition, transition},
    roles::validate_role,
    store::{
        Store, conflict, id, integer, invalid, remove, request_id, strict, text, trimmed_text,
    },
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

impl Store {
    pub(crate) fn run_records(&self, table: &str, run_id: &str) -> Result<Vec<Value>> {
        let mut query = self.db.prepare(&format!(
            "SELECT data FROM {table} WHERE run_id=? ORDER BY rowid"
        ))?;
        query
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| crate::store::parse(r?))
            .collect()
    }
    pub fn get_assignment(&self, assignment_id: &str) -> Result<Value> {
        self.record("assignments", "id", assignment_id)?
            .ok_or_else(|| Error::NotFound("Assignment not found.".into()))
    }
    pub fn get_attempt(&self, attempt_id: &str) -> Result<Value> {
        self.record("attempts", "id", attempt_id)?
            .ok_or_else(|| Error::NotFound("Attempt not found.".into()))
    }
    pub(crate) fn save_attempt(&self, attempt: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE attempts SET data=? WHERE id=?",
            params![attempt.to_string(), attempt["id"].as_str()],
        )?;
        Ok(())
    }
    pub(crate) fn save_assignment(&self, assignment: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE assignments SET data=? WHERE id=?",
            params![assignment.to_string(), assignment["id"].as_str()],
        )?;
        Ok(())
    }
    pub(crate) fn change_run(&self, run_id: &str, action: Transition) -> Result<Value> {
        let run = transition(&self.get_run(run_id)?, &action, self.clock_ms())?;
        self.save_run(&run)?;
        self.get_run(run_id)
    }
    pub(crate) fn current_guidance(&self, run_id: &str) -> Result<Option<Value>> {
        let run = self.get_run(run_id)?;
        let Some(repo_id) = run["taskSnapshot"]["repositoryId"].as_str() else {
            return Ok(None);
        };
        Ok(self.record("repositories", "id", repo_id)?.and_then(|r| {
            r["guidance"]
                .as_array()
                .and_then(|a| a.last())
                .map(|g| json!({"version":g["version"],"text":g["text"]}))
        }))
    }

    pub fn admit_assignment(&self, run_id: &str, input: &Value) -> Result<Value> {
        self.transaction(|s| s.admit_assignment_in_transaction(run_id, input))
    }
    pub(crate) fn admit_assignment_in_transaction(
        &self,
        run_id: &str,
        input: &Value,
    ) -> Result<Value> {
        let name = trimmed_text(input, "name", 1, 100)?;
        let role = validate_role(&input["role"])?;
        if role["enabled"] != true {
            return Err(conflict("Disabled roles cannot receive new assignments."));
        }
        let instruction = text(input, "input", 1, 100000)?;
        let coordinator = input["coordinator"]
            .as_bool()
            .ok_or_else(|| invalid("coordinator must be boolean."))?;
        if !input["model"].is_object() {
            return Err(invalid("A resolved assignment model is required."));
        }
        self.assert_intake_dispatch(run_id)?;
        let mut run = self.get_run(run_id)?;
        crate::run_purpose(&run)?;
        self.assert_evaluation_admission(&run, input)?;
        if run["status"] != "awaiting_runner" {
            return Err(conflict("Run is not accepting assignments."));
        }
        if let Some(reason) = self.implementation_plan_hold(&run)? {
            return Err(conflict(&reason));
        }
        if crate::dependencies::coding(&run)
            && self.run_records("assignments", run_id)?.is_empty()
            && run["continuedFrom"].is_null()
            && let Some(work) = self.repository_work(run_id)?
            && work["base"] != run["implementationPlan"]["decision"]["basis"]["commit"]
        {
            return Err(conflict(
                "Prepared work does not match the accepted plan source. Cancel and queue against the current plan.",
            ));
        }

        let current = self.run_records("assignments", run_id)?;
        if !crate::dependencies::coding(&run)
            && run["purpose"] != "research"
            && run["purpose"] != "evaluation"
            && (!coordinator || !current.is_empty())
        {
            return Err(conflict("Refinement runs admit one coordinator only."));
        }
        if current.iter().any(|a| a["name"] == name) {
            return Err(conflict("Assignment name already admitted."));
        }
        if if coordinator {
            current.iter().any(|a| a["coordinator"] == true)
        } else {
            current
                .iter()
                .filter(|a| a["coordinator"] != true && a.get("retiredAt").is_none())
                .count()
                >= if run["purpose"] == "evaluation" { 1 } else { 3 }
        } {
            return Err(conflict("Assignment concurrency limit reached."));
        }
        let attempt_id = id();
        let assignment_id = id();
        let mut assignment = json!({"id":assignment_id,"runId":run_id,"name":name,"role":role,"model":input["model"],"coordinator":coordinator,
            "currentAttemptId":attempt_id,"brief":input.get("brief").cloned().unwrap_or_else(||json!({"criteria":[],"dependsOn":[]}))});
        if !coordinator
            && let Some(binding) = self.choreography_assignment(run_id, &input["name"], &role)?
        {
            assignment["choreography"] = binding;
        }
        if let Some(plan) = run.get("implementationPlan") {
            assignment["implementationPlan"] = plan.clone();
        }
        if run["purpose"] == "evaluation" {
            if let Some(guidance) = run.get("evaluationGuidance") {
                assignment["projectGuidance"] = guidance.clone();
            }
        } else if let Some(guidance) = self.current_guidance(run_id)? {
            assignment["projectGuidance"] = guidance;
        }
        let attempt = json!({"id":attempt_id,"assignmentId":assignment_id,"runId":run_id,"generation":1,"input":instruction,"model":input["model"],"state":"admitted","createdAt":self.time()});
        self.db.execute(
            "INSERT INTO assignments VALUES(?,?,?,?)",
            params![assignment_id, run_id, name, assignment.to_string()],
        )?;
        self.db.execute(
            "INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)",
            params![attempt_id, assignment_id, attempt.to_string()],
        )?;
        if current.is_empty() && run.get("position").is_some() {
            remove(&mut run, "position");
            self.save_run(&run)?;
        }
        Ok(json!({"assignment":assignment,"attempt":attempt}))
    }

    pub fn admit_initial_team(&self, run_id: &str, team: &[Value]) -> Result<Value> {
        self.transaction(|s| {
            let run = s.get_run(run_id)?;
            if run["status"] != "awaiting_runner"
                || run["desiredState"] == "cancelled"
                || run["desiredState"] == "paused"
            {
                return Err(conflict("Run stopped before initial team admission."));
            }
            let existing = s.run_records("assignments", run_id)?;
            if !existing.is_empty() {
                if existing.len() != team.len() {
                    return Err(conflict(
                        "Admitted team differs from the durable launch decision.",
                    ));
                }
                let mut replay = Vec::new();
                for (assignment, input) in existing.into_iter().zip(team) {
                    let attempt =
                        s.get_attempt(assignment["currentAttemptId"].as_str().unwrap())?;
                    if assignment["name"] != input["name"]
                        || assignment["role"]["id"] != input["role"]["id"]
                        || assignment["coordinator"] != input["coordinator"]
                        || attempt["input"] != input["input"]
                    {
                        return Err(conflict(
                            "Admitted team differs from the durable launch decision.",
                        ));
                    }
                    replay.push(json!({"assignment":assignment,"attempt":attempt}));
                }
                return Ok(json!(replay));
            }
            s.validate_insertion_team(run_id, team)?;
            team.iter()
                .map(|input| s.admit_assignment_in_transaction(run_id, input))
                .collect::<Result<Vec<_>>>()
                .map(|v| json!(v))
        })
    }

    pub fn executing_runs(&self) -> Result<Value> {
        let mut statement = self.db.prepare("SELECT DISTINCT s.run_id, (EXISTS(SELECT 1 FROM repository_work w WHERE w.run_id=s.run_id) OR COALESCE(json_extract(r.data,'$.purpose'),'coding') IN ('coding','evaluation')) FROM attempts a JOIN assignments s ON a.assignment_id=s.id JOIN runs r ON r.id=s.run_id WHERE a.state<>'admitted' AND a.ownership IS NOT 'stopped'")?;
        let runs = statement
            .query_map([], |r| {
                Ok(json!({"runId":r.get::<_,String>(0)?,"coding":r.get::<_,bool>(1)?}))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(json!(runs))
    }
    pub fn unstopped_coding_run(&self, except_run_id: &str) -> Result<Option<Value>> {
        let run_id: Option<String> = self.db.query_row("SELECT s.run_id FROM attempts a JOIN assignments s ON a.assignment_id=s.id JOIN runs r ON r.id=s.run_id WHERE s.run_id<>? AND COALESCE(json_extract(r.data,'$.purpose'),'coding') IN ('coding','evaluation') AND a.state<>'admitted' AND a.ownership IS NOT 'stopped' LIMIT 1",[except_run_id],|r|r.get(0)).optional()?;
        run_id.as_deref().map(|id| self.get_run(id)).transpose()
    }
    pub fn claim_attempt(&self, attempt_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut attempt = s.get_attempt(attempt_id)?;
            if attempt["state"] != "admitted" { return Err(conflict("Attempt already claimed; reconcile ownership before replacement.")); }
            let run_id = attempt["runId"].as_str().unwrap().to_owned();
            s.assert_intake_dispatch(&run_id)?;
            s.assert_evaluation_dispatch(&run_id)?;
            let run=s.get_run(&run_id)?;
            if (crate::dependencies::coding(&run) || run["purpose"]=="evaluation" || s.record("repository_work","run_id",&run_id)?.is_some()) && s.executing_runs()?.as_array().unwrap().iter().any(|r| r["runId"] != run_id && r["coding"] == true) {
                return Err(conflict("Another coding ticket is executing. Crewroom executes one coding ticket at a time; wait until its agents stop."));
            }
            attempt["state"] = json!("starting"); s.save_attempt(&attempt)?; s.account_time_in_transaction(&run_id)?; Ok(attempt)
        })
    }
    /// FOR-69: the harness executable changed after admission and its
    /// launch-time re-probe passed. Only the frozen selection's executable
    /// identity may change; route, offering and effort stay as admitted. The
    /// receipt is appended to the attempt's `reprobes`.
    pub fn record_attempt_reprobe(
        &self,
        attempt_id: &str,
        model: &Value,
        receipt: &Value,
    ) -> Result<Value> {
        strict(
            receipt,
            &[
                "admittedVersion",
                "admittedBinarySha256",
                "version",
                "binarySha256",
                "compatibility",
            ],
        )?;
        for key in [
            "admittedVersion",
            "admittedBinarySha256",
            "version",
            "binarySha256",
            "compatibility",
        ] {
            text(receipt, key, 1, 4000)?;
        }
        self.transaction(|s| {
            let mut attempt = s.get_attempt(attempt_id)?;
            if attempt["state"] != "admitted" {
                return Err(conflict("Only an unclaimed attempt can record a re-probe."));
            }
            let assignment = s.get_assignment(attempt["assignmentId"].as_str().unwrap())?;
            let current = attempt
                .get("model")
                .filter(|v| !v.is_null())
                .unwrap_or(&assignment["model"])
                .clone();
            let mut unchanged = model.clone();
            if let (Some(next), Some(frozen)) = (
                unchanged.pointer_mut("/selection/identity"),
                current.pointer("/selection/identity"),
            ) {
                *next = frozen.clone();
            } else {
                return Err(conflict("A re-probe needs a frozen catalog selection."));
            }
            if unchanged != current
                || current["selection"]["identity"]["version"] != receipt["admittedVersion"]
                || current["selection"]["identity"]["binarySha256"]
                    != receipt["admittedBinarySha256"]
                || model["selection"]["identity"]["version"] != receipt["version"]
                || model["selection"]["identity"]["binarySha256"] != receipt["binarySha256"]
            {
                return Err(conflict(
                    "A re-probe may change only the frozen executable identity.",
                ));
            }
            attempt["model"] = model.clone();
            if !attempt["reprobes"].is_array() {
                attempt["reprobes"] = json!([]);
            }
            let mut entry = receipt.clone();
            entry["at"] = json!(s.time());
            attempt["reprobes"].as_array_mut().unwrap().push(entry);
            s.save_attempt(&attempt)?;
            Ok(attempt)
        })
    }
    pub fn record_attempt_identity(&self, attempt_id: &str, identity: &Value) -> Result<Value> {
        strict(
            identity,
            &[
                "hostPid",
                "ompPid",
                "runnerKind",
                "runnerPid",
                "runnerVersion",
                "runnerProfile",
                "observedModel",
                "capabilities",
                "sessionId",
                "hostUrl",
                "pane",
                "recoveryMode",
                "sessionDirectory",
                "runnerCwd",
            ],
        )?;
        for key in [
            "hostPid",
            if identity["runnerKind"] == "codex" || identity["runnerKind"] == "claude-code" {
                "runnerPid"
            } else {
                "ompPid"
            },
        ] {
            integer(identity, key, 1, u32::MAX.into())?;
        }
        for key in ["sessionId", "hostUrl", "pane"] {
            text(identity, key, 1, 4000)?;
        }
        self.transaction(|s| {
            let mut attempt = s.get_attempt(attempt_id)?;
            crate::verify_runner_identity(&attempt["model"], identity)?;
            if attempt["state"] != "starting" {
                return Err(conflict("Attempt is not awaiting identity verification."));
            }
            attempt
                .as_object_mut()
                .unwrap()
                .extend(identity.as_object().unwrap().clone());
            attempt["state"] = json!("idle");
            attempt["ownership"] = json!("held");
            attempt["startedAt"] = json!(s.time());
            s.save_attempt(&attempt)?;
            Ok(attempt)
        })
    }
    pub fn record_ownership(
        &self,
        attempt_id: &str,
        stopped: bool,
        reason: Option<&str>,
    ) -> Result<()> {
        self.transaction(|s| {
            let mut attempt = s.get_attempt(attempt_id)?;
            if stopped && attempt["ownership"] != "stopped" {
                attempt["stoppedAt"] = json!(s.time());
            }
            attempt["ownership"] = json!(if stopped { "stopped" } else { "uncertain" });
            attempt["state"] = json!(if stopped {
                "exited"
            } else {
                "attention_required"
            });
            if let Some(reason) = reason.filter(|r| !r.is_empty())
                && attempt["ownershipReason"] != reason
            {
                if !attempt["ownershipNotes"].is_array() {
                    attempt["ownershipNotes"] = json!([]);
                }
                attempt["ownershipNotes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"at":s.time(),"reason":reason}));
            }
            remove(&mut attempt, "ownershipReason");
            if let Some(reason) = reason {
                attempt["ownershipReason"] = json!(reason);
            }
            s.save_attempt(&attempt)?;
            s.account_time_in_transaction(attempt["runId"].as_str().unwrap())?;
            Ok(())
        })
    }
    pub fn replace_attempt(&self, assignment_id: &str, input: &Value) -> Result<Value> {
        let automatic = input["automatic"]
            .as_bool()
            .ok_or_else(|| invalid("automatic must be boolean."))?;
        self.transaction(|s| {
            let mut assignment = s.get_assignment(assignment_id)?;
            if assignment.get("retiredAt").is_some() { return Err(conflict("Retired assignments cannot resume.")); }
            let previous = s.get_attempt(assignment["currentAttemptId"].as_str().unwrap())?;
            if previous["ownership"] != "stopped" { return Err(conflict("Resolve old process ownership before replacement.")); }
            let run_id = assignment["runId"].as_str().unwrap().to_owned(); let run = s.get_run(&run_id)?;
            if run["status"] != "paused" { return Err(conflict("Pause and reconcile the run before replacement.")); }
            if run["activeMs"].as_u64().unwrap_or(0) >= run["activeLimitMs"].as_u64().unwrap_or(3_600_000) { return Err(conflict("The active execution budget is exhausted.")); }
            if s.run_records("peer_messages",&run_id)?.iter().any(|m| ["pending","dispatched","uncertain"].contains(&m["disposition"].as_str().unwrap_or(""))) { return Err(conflict("Reconcile uncertain message effects before replacement.")); }
            if s.run_records("steering",&run_id)?.iter().any(|m| m["delivery"] == "dispatched") { return Err(conflict("Reconcile uncertain steering delivery before replacement.")); }
            if s.run_records("delivery_notices",&run_id)?.iter().any(|m| m["disposition"] == "dispatched") { return Err(conflict("Reconcile uncertain controller delivery before replacement.")); }
            let cycles = assignment["automaticCycles"].as_u64().unwrap_or(0);
            let limit = 2 + assignment["repairAllowances"].as_array().map_or(0,Vec::len) as u64;
            if automatic && cycles >= limit { return Err(conflict(&format!("{} automatic recovery cycles are exhausted. Allow another cycle explicitly to continue.", if limit == 2 {"The two".into()} else {format!("All {limit}")}))); }
            let previous_model = previous.get("model").unwrap_or(&assignment["model"]);
            let mut attempt = json!({"id":id(),"assignmentId":assignment_id,"runId":run_id,"generation":previous["generation"].as_u64().unwrap()+1,
                "input":previous["input"],"model":previous_model,"state":"admitted","createdAt":s.time(),"previousAttemptId":previous["id"]});
            if let Some(active_input) = previous.get("activeInputMessageId") { attempt["activeInputMessageId"] = active_input.clone(); }
            if let Some(fallback) = input.get("fallback") {
                if run["purpose"]=="evaluation" { return Err(conflict("An evaluation cannot change its frozen model or route during recovery.")); }
                let events = s.agent_events(&run_id)?;
                let failure = events.iter().find(|e| e["attemptId"] == previous["id"] && e["sequence"] == fallback["eventSequence"] && e["event"]["type"] == "prompt_result");
                if !automatic || failure.is_none_or(|f| f["event"]["status"] != "error" || f["event"]["error"]["retryable"] != true || f["event"]["error"]["provider"] != previous_model["resolved"]["provider"] || f["event"]["error"]["model"] != previous_model["resolved"]["id"])
                    || fallback["model"]["fallbackFrom"] != previous_model["resolved"]["selector"] || fallback["model"]["resolved"]["selector"] == previous_model["resolved"]["selector"] {
                    return Err(conflict("A model fallback requires the prior attempt's recorded retryable provider failure."));
                }
                attempt["model"] = fallback["model"].clone();
                attempt["fallback"] = json!({"fromAttemptId":previous["id"],"fromModel":previous_model["resolved"]["selector"],"eventSequence":fallback["eventSequence"],"reason":failure.unwrap()["event"]["error"]["message"].as_str().unwrap_or("Retryable provider failure")});
            }
            assignment["currentAttemptId"] = attempt["id"].clone(); assignment["automaticCycles"] = json!(cycles+u64::from(automatic));
            s.db.execute("INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)",params![attempt["id"].as_str(),assignment_id,attempt.to_string()])?;
            s.save_assignment(&assignment)?; Ok(attempt)
        })
    }
    pub fn grant_repair_cycle(
        &self,
        run_id: &str,
        assignment_id: &str,
        input: &Value,
    ) -> Result<Value> {
        strict(input, &["reason", "requestId"])?;
        let key = request_id(input)?;
        let reason = trimmed_text(input, "reason", 1, 2000)?;
        self.transaction(|s| {
            let mut assignment = s.get_assignment(assignment_id)?;
            if s.get_run(run_id)?["purpose"] == "evaluation" {
                return Err(conflict(
                    "An evaluation cannot add discretionary repair allowances.",
                ));
            }
            if assignment["runId"] != run_id {
                return Err(Error::NotFound("Assignment not found.".into()));
            }
            if assignment.get("retiredAt").is_some() {
                return Err(conflict(
                    "Retired assignments cannot receive repair cycles.",
                ));
            }
            if s.get_run(run_id)?["desiredState"] == "cancelled" {
                return Err(conflict("This run is cancelled."));
            }
            if assignment["repairAllowances"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v["id"] == key))
            {
                return Ok(assignment);
            }
            if assignment["automaticCycles"].as_u64().unwrap_or(0)
                < 2 + assignment["repairAllowances"]
                    .as_array()
                    .map_or(0, Vec::len) as u64
            {
                return Err(conflict(
                    "Automatic cycles remain for this assignment; no extra allowance is needed.",
                ));
            }
            if !assignment["repairAllowances"].is_array() {
                assignment["repairAllowances"] = json!([]);
            }
            assignment["repairAllowances"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":key,"grantedAt":s.time(),"reason":reason}));
            s.save_assignment(&assignment)?;
            Ok(assignment)
        })
    }
    pub fn begin_pause(&self, run_id: &str, reason: Option<&str>) -> Result<Value> {
        self.transaction(|s| {
            s.change_run(
                run_id,
                Transition::BeginPause {
                    reason: reason.map(str::to_owned),
                },
            )
        })
    }
    pub fn finish_pause(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let unresolved = s.runtime(run_id)?["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped");
            s.change_run(run_id, Transition::FinishPause { unresolved })
        })
    }
}
