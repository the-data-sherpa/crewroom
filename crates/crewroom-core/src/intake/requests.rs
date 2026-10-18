use super::*;

impl Store {
    pub fn intake_answer_replay(&self, intake_id: &str, input: &Value) -> Result<Option<Value>> {
        self.replay(
            request_id(input)?,
            "intake-answer",
            &json!({"intakeId":intake_id,"input":input}),
        )
    }
    pub(crate) fn assert_intake_dispatch(&self, run_id: &str) -> Result<()> {
        let run = self.get_run(run_id)?;
        if run["purpose"] != "grill" || !run["intakeId"].is_string() {
            return Ok(());
        }
        let intake = self.get_intake(run["intakeId"].as_str().unwrap())?;
        self.assert_intake_live(&intake)?;
        if !["scoping", "answer_pending"].contains(&intake["state"].as_str().unwrap_or("")) {
            return Err(conflict(
                "Resolve the pending intake decision before dispatch.",
            ));
        }
        for other in self.intakes()? {
            if other["scopingRunId"] == run_id {
                continue;
            }
            let other_id = other["scopingRunId"].as_str().unwrap();
            if self.runtime(other_id)?["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["ownership"] != "stopped")
            {
                return Err(conflict(
                    "Another intake coordinator has pending or unresolved ownership.",
                ));
            }
        }
        Ok(())
    }

    pub fn intakes(&self) -> Result<Vec<Value>> {
        self.all("intake_requests")
    }
    pub fn queued_intake_replay(&self, task_id: &str, body: &Value) -> Result<Option<Value>> {
        let Some(key) = body.get("requestId") else {
            return Ok(None);
        };
        let key = key
            .as_str()
            .ok_or_else(|| conflict("Invalid request identity."))?;
        let mut payload = json!({"taskId":task_id,"revision":body["revision"],"activeLimitMs":body["activeLimitMs"].as_u64().unwrap_or(3_600_000)});
        if let Some(launch) = body.get("launch") {
            payload["launch"] = crate::launch::parse_launch(launch, None)?;
        }
        if let Some(playbook) = body.get("playbookId") {
            payload["playbookId"] = playbook.clone();
        }
        self.replay(key, "queue", &payload)
    }
    pub fn record_intake_hold(&self, intake_id: &str, reason: Option<&str>) -> Result<()> {
        self.transaction(|s| {
            let mut intake = s.get_intake(intake_id)?;
            if intake["holdReason"].as_str() != reason {
                intake["holdReason"] = json!(reason);
                s.save_intake(&mut intake)?;
            }
            Ok(())
        })
    }
    pub fn get_intake(&self, intake_id: &str) -> Result<Value> {
        self.record("intake_requests", "id", intake_id)?
            .ok_or_else(|| Error::NotFound("Intake not found.".into()))
    }
    pub fn active_intake(&self, task_id: &str) -> Result<Option<Value>> {
        Ok(self
            .task_intake_records("intake_requests", task_id)?
            .into_iter()
            .find(|r| !["cancelled", "handed_off"].contains(&r["state"].as_str().unwrap_or(""))))
    }
    pub(super) fn save_intake(&self, intake: &mut Value) -> Result<()> {
        intake["revision"] = json!(intake["revision"].as_u64().unwrap_or(0) + 1);
        self.db.execute(
            "UPDATE intake_requests SET data=? WHERE id=?",
            params![intake.to_string(), intake["id"].as_str()],
        )?;
        Ok(())
    }
    pub(super) fn assert_intake_live(&self, intake: &Value) -> Result<()> {
        if ["cancelled", "handed_off"].contains(&intake["state"].as_str().unwrap_or("")) {
            return Err(conflict("This intake is no longer active."));
        }
        if let Some(id) = intake["scopingRunId"].as_str() {
            let run = self.get_run(id)?;
            if run["desiredState"] == "cancelled" || run["status"] == "cancelled" {
                return Err(conflict(
                    "Intake was cancelled; its queue intent cannot be revived.",
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn queue_intake(&self, task: &Value, payload: &Value) -> Result<Value> {
        if self.active_intake(task["id"].as_str().unwrap())?.is_some() {
            return Err(conflict("This ticket already has an active intake."));
        }
        let ordinal = self.queue_view()?["revision"].clone();
        self.bump_queue()?;
        let intake_id = id();
        let mut run = self.create_alternate_run(
            task.clone(),
            "grill",
            payload["activeLimitMs"].as_u64().unwrap_or(3_600_000),
            None,
        )?;
        run["intakeId"] = json!(intake_id);
        if let Some(source) = self.plan_source(task["id"].as_str().unwrap())? {
            run["intakeSource"] = source;
        }
        self.save_run(&run)?;
        let intake = json!({"id":intake_id,"taskId":task["id"],"revision":1,"state":"scoping","createdAt":self.time(),"queueOrdinal":ordinal,"scopingRunId":run["id"],"queue":payload,"questions":[],"sourceId":run["intakeSource"]["id"]});
        self.db.execute(
            "INSERT INTO intake_requests VALUES(?,?,?)",
            params![intake_id, task["id"].as_str(), intake.to_string()],
        )?;
        Ok(run)
    }
    /// Replayed by the existing scheduler after it has stopped the Grill process.
    pub fn handoff_intake(
        &self,
        intake_id: &str,
        roles: &[Value],
        playbooks: &[Value],
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut intake = s.get_intake(intake_id)?;
            if let Some(id) = intake["codingRunId"].as_str() {
                return s.get_run(id);
            }
            s.assert_intake_live(&intake)?;
            if intake["state"] != "handoff" {
                return Err(conflict("Accept a current plan before handing off intake."));
            }
            let task_id = intake["taskId"].as_str().unwrap().to_owned();
            let plan = s
                .current_implementation_plan(&task_id)?
                .filter(|p| p["id"] == intake["acceptedPlanId"])
                .ok_or_else(|| conflict("Accepted plan needs revalidation before handoff."))?;
            let run_id = intake["scopingRunId"].as_str().unwrap();
            s.close_refinement(run_id)?;
            let task = s.get_task(&task_id)?;
            let mut input = intake["queue"].clone();
            input.as_object_mut().unwrap().remove("taskId");
            input["revision"] = task["revision"].clone();
            input["requestId"] = json!(intake_id);
            let mut run = s.queue_with_playbooks(&task_id, &input, roles, playbooks)?;
            run["intakeId"] = json!(intake_id);
            run["implementationPlan"] = plan;
            run["queuedAt"] = intake["createdAt"].clone();
            run["queueOrdinal"] = intake["queueOrdinal"].clone();
            // Reinsert at the original request's order, before later untouched queue entries.
            let queue = s.coding_queue()?;
            let before = queue["plans"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| {
                    p["id"] != run["id"]
                        && p["queueOrdinal"].as_u64().unwrap_or(0)
                            > intake["queueOrdinal"].as_u64().unwrap_or(0)
                })
                .filter_map(|p| p["position"].as_u64())
                .min();
            if let Some(position) = before {
                for mut other in queue["plans"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|p| {
                        p["id"] != run["id"]
                            && p["position"].as_u64().is_some_and(|v| v >= position)
                    })
                    .cloned()
                {
                    other["position"] = json!(other["position"].as_u64().unwrap() + 1);
                    s.db.execute(
                        "UPDATE runs SET data=? WHERE id=?",
                        params![other.to_string(), other["id"].as_str()],
                    )?;
                }
                run["position"] = json!(position);
            }
            s.save_run(&run)?;
            intake["state"] = json!("handed_off");
            intake["codingRunId"] = run["id"].clone();
            s.save_intake(&mut intake)?;
            Ok(run)
        })
    }
    pub fn cancel_intake(&self, intake_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "revision"])?;
        let key = request_id(input)?;
        self.transaction(|s| {
            let payload = json!({"intakeId":intake_id,"input":input});
            if let Some(prior) = s.replay(key, "cancel-intake", &payload)? {
                return Ok(prior);
            }
            let mut intake = s.get_intake(intake_id)?;
            if intake["revision"] != input["revision"] {
                return Err(conflict("Intake changed. Reload before cancelling."));
            }
            if intake["state"] == "handed_off" {
                return Err(conflict("Cancel the linked coding run instead."));
            }
            intake["state"] = json!("cancelled");
            s.save_intake(&mut intake)?;
            s.set_desired_state(intake["scopingRunId"].as_str().unwrap(), "cancelled")?;
            s.receipt(key, "cancel-intake", &payload, &intake)?;
            Ok(intake)
        })
    }
    /// Admission gate for new coding assignments. Existing admitted histories remain valid.
    pub fn implementation_plan_hold(&self, run: &Value) -> Result<Option<String>> {
        if !crate::dependencies::coding(run)
            || !self
                .run_records("assignments", run["id"].as_str().unwrap())?
                .is_empty()
        {
            return Ok(None);
        }
        let Some(plan_id) = run["implementationPlan"]["id"].as_str() else {
            return Ok(Some("Needs an accepted implementation plan.".into()));
        };
        let task_id = run["taskId"].as_str().unwrap();
        let plan = self
            .implementation_plans(task_id)?
            .into_iter()
            .find(|p| p["id"] == plan_id)
            .ok_or_else(|| conflict("Captured implementation plan is missing."))?;
        if plan["decision"]["decision"] != "accept"
            || plan["sha256"] != run["implementationPlan"]["sha256"]
        {
            return Ok(Some("Captured plan is not accepted.".into()));
        }
        Ok(plan["holdReason"].as_str().map(str::to_owned))
    }
    pub fn intake_view(&self, task_id: &str) -> Result<Value> {
        let history = self.task_intake_records("intake_requests", task_id)?;
        let intake = history
            .iter()
            .find(|r| !["cancelled", "handed_off"].contains(&r["state"].as_str().unwrap_or("")))
            .cloned();
        let plans = self.implementation_plans(task_id)?;
        let current = plans
            .iter()
            .rev()
            .find(|p| p["decision"]["decision"] == "accept")
            .filter(|p| p["holdReason"].is_null())
            .cloned();
        let mut view = json!({"intake":intake,"history":history,"plans":plans,"currentPlan":current,"source":self.plan_source(task_id)?});
        let state = if let Some(intake) = &intake {
            let run = self.get_run(intake["scopingRunId"].as_str().unwrap())?;
            view["run"] = run.clone();
            if run["desiredState"] == "cancelled" {
                "Cancelled"
            } else if self
                .source_hold(&self.get_task(task_id)?, &run["intakeSource"])?
                .is_some()
            {
                "Needs revalidation"
            } else {
                match intake["state"].as_str() {
                    Some("waiting") if run["status"] == "paused" => "Waiting for you",
                    Some("review") if run["status"] == "paused" => "Plan review",
                    Some("waiting" | "review" | "handoff") => "Stopping scoping",
                    _ => "Scoping",
                }
            }
        } else if self
            .open_run(task_id)?
            .is_some_and(|r| crate::dependencies::coding(&r) && r["status"] == "awaiting_runner")
        {
            if self
                .open_run(task_id)?
                .is_some_and(|r| r["implementationPlan"].is_null())
            {
                "Needs plan"
            } else {
                "Queued for execution"
            }
        } else if current.is_some() {
            "Plan ready"
        } else if plans
            .last()
            .is_some_and(|p| p["decision"].is_null() && p["holdReason"].is_null())
        {
            "Plan review"
        } else if history.last().is_some_and(|i| i["state"] == "cancelled") {
            "Cancelled"
        } else if plans.is_empty() {
            "Saved idea"
        } else if plans
            .last()
            .is_some_and(|p| p["decision"]["decision"] == "reject" && p["holdReason"].is_null())
        {
            // Without an active intake no agent revises a rejected plan.
            "Plan rejected"
        } else {
            "Needs revalidation"
        };
        view["state"] = json!(state);
        Ok(view)
    }
    pub fn revalidate_intake(&self, intake_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "revision", "sourceId"])?;
        let key = request_id(input)?;
        self.transaction(|s| {
            let payload=json!({"intakeId":intake_id,"input":input});
            if let Some(prior)=s.replay(key,"revalidate-intake",&payload)? {return Ok(prior);}
            let mut intake=s.get_intake(intake_id)?;s.assert_intake_live(&intake)?;
            if intake["revision"]!=input["revision"] {return Err(conflict("Intake changed. Reload before revalidation."));}
            let task_id=intake["taskId"].as_str().unwrap();
            let source=s.plan_source(task_id)?.ok_or_else(||conflict("Refresh the source before revalidation."))?;
            let task=s.get_task(task_id)?;
            if source["id"]!=input["sourceId"] {return Err(conflict("Source changed. Reload before revalidation."));}
            if let Some(reason)=s.source_hold(&task,&source)? {return Err(conflict(&reason));}
            let mut run=s.get_run(intake["scopingRunId"].as_str().unwrap())?;
            if !["paused","awaiting_runner"].contains(&run["status"].as_str().unwrap_or("")) || s.runtime(run["id"].as_str().unwrap())?["attempts"].as_array().unwrap().iter().any(|a|a["state"]!="admitted"&&a["ownership"]!="stopped") {return Err(conflict("Stop scoping before adopting a new source basis."));}
            run["intakeSource"]=source.clone();run["taskRevision"]=task["revision"].clone();run["taskSnapshot"]=task;
            crate::store::remove(&mut run,"sourceConflict");s.save_run(&run)?;
            intake["sourceId"]=source["id"].clone();crate::store::remove(&mut intake,"inspection");crate::store::remove(&mut intake,"acceptedPlanId");
            intake["state"]=json!(if run["status"]=="awaiting_runner" {"scoping"} else {"answer_pending"});
            let message=s.steer(run["id"].as_str().unwrap(),&json!({"requestId":key,"body":format!("The operator requests source revalidation. Current basis: {}. Retained questions and answers: {}. Inspect what changed and reuse decisions that remain valid; propose a fresh plan.",source,intake["questions"])}))?;
            intake["pendingMessageId"]=message["id"].clone();s.save_intake(&mut intake)?;s.receipt(key,"revalidate-intake",&payload,&intake)?;Ok(intake)
        })
    }
    pub fn intake_source_for_run(&self, run_id: &str) -> Result<Value> {
        let run = self.get_run(run_id)?;
        let intake = self.get_intake(
            run["intakeId"]
                .as_str()
                .ok_or_else(|| conflict("No intake source authority."))?,
        )?;
        self.assert_intake_live(&intake)?;
        if intake["scopingRunId"] != run_id {
            return Err(conflict(
                "Only the linked scoping run can inspect this source.",
            ));
        }
        if let Some(reason) = self.source_hold(
            &self.get_task(intake["taskId"].as_str().unwrap())?,
            &run["intakeSource"],
        )? {
            return Err(conflict(&reason));
        }
        Ok(run["intakeSource"].clone())
    }
}
