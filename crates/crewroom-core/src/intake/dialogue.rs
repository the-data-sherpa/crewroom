use super::*;

impl Store {
    fn intake_caller(&self, attempt_id: &str) -> Result<(Value, Value)> {
        let attempt = self.get_attempt(attempt_id)?;
        let run = self.get_run(attempt["runId"].as_str().unwrap())?;
        let assignment = self.get_assignment(attempt["assignmentId"].as_str().unwrap())?;
        if assignment["currentAttemptId"] != attempt_id
            || assignment["coordinator"] != true
            || attempt["ownership"] != "held"
            || run["desiredState"] != "running"
            || run["status"] != "running"
        {
            return Err(conflict(
                "Only the current running intake coordinator can scope this job.",
            ));
        }
        let mut intake = self.get_intake(
            run["intakeId"]
                .as_str()
                .ok_or_else(|| conflict("This refinement has no intake authority."))?,
        )?;
        self.assert_intake_live(&intake)?;
        if intake["scopingRunId"] != run["id"] {
            return Err(conflict("Intake coordinator identity changed."));
        }
        if let Some(reason) = self.source_hold(
            &self.get_task(intake["taskId"].as_str().unwrap())?,
            &run["intakeSource"],
        )? {
            return Err(conflict(&reason));
        }
        if intake["state"] == "answer_pending" {
            intake["state"] = json!("scoping");
            self.save_intake(&mut intake)?;
        }
        Ok((intake, run))
    }
    pub fn record_intake_inspection(&self, attempt_id: &str, source_id: &str) -> Result<()> {
        self.transaction(|s| {
            let (mut intake, run) = s.intake_caller(attempt_id)?;
            if run["intakeSource"]["id"] != source_id {
                return Err(conflict("Source changed during inspection."));
            }
            intake["inspection"] =
                json!({"sourceId":source_id,"attemptId":attempt_id,"at":s.time()});
            s.save_intake(&mut intake)
        })
    }
    pub fn ask_intake_questions(
        &self,
        attempt_id: &str,
        tool_key: &str,
        input: &Value,
    ) -> Result<Value> {
        strict(input, &["sourceId", "questions"])?;
        let key = format!("intake-questions:{attempt_id}:{tool_key}");
        self.transaction(|s| {
            if let Some(prior) = s.replay(&key, "intake-questions", input)? {
                return Ok(prior);
            }
            let (mut intake, run) = s.intake_caller(attempt_id)?;
            if input["sourceId"] != run["intakeSource"]["id"]
                || intake["inspection"]["sourceId"] != input["sourceId"]
            {
                return Err(conflict(
                    "Inspect the current pinned source before asking questions.",
                ));
            }
            if intake["state"] != "scoping" {
                return Err(conflict("An intake decision is already pending."));
            }
            let questions = input["questions"]
                .as_array()
                .filter(|q| !q.is_empty() && q.len() <= 3)
                .ok_or_else(|| conflict("Ask one to three missing consequential decisions."))?;
            let mut history = intake["questions"].as_array().cloned().unwrap_or_default();
            if history.len() + questions.len() > 100 {
                return Err(conflict("Intake question limit reached."));
            }
            for q in questions {
                strict(q, &["id", "prompt", "rationale", "choices"])?;
                let id = crate::store::trimmed_text(q, "id", 1, 64)?;
                if q["id"] != id
                    || !id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                {
                    return Err(conflict(
                        "Question IDs use letters, numbers, hyphens or underscores.",
                    ));
                }
                crate::store::trimmed_text(q, "prompt", 1, 2000)?;
                crate::store::trimmed_text(q, "rationale", 1, 2000)?;
                let choices = q["choices"]
                    .as_array()
                    .filter(|c| c.len() <= 5)
                    .ok_or_else(|| conflict("Use up to five suggested choices."))?;
                if choices.iter().any(|c| {
                    c.as_str()
                        .is_none_or(|c| c.trim().is_empty() || c.len() > 500)
                }) || history.iter().any(|q| q["id"] == id)
                {
                    return Err(conflict("Invalid choices or reused question ID."));
                }
                let mut q = q.clone();
                q["sourceId"] = input["sourceId"].clone();
                q["askedAt"] = json!(s.time());
                q["attemptId"] = json!(attempt_id);
                history.push(q);
            }
            intake["questions"] = json!(history);
            intake["state"] = json!("waiting");
            s.save_intake(&mut intake)?;
            s.set_desired_state(run["id"].as_str().unwrap(), "paused")?;
            s.begin_pause(run["id"].as_str().unwrap(), Some("intake"))?;
            s.receipt(&key, "intake-questions", input, &intake)?;
            Ok(intake)
        })
    }
    pub fn answer_intake(&self, intake_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "revision", "sourceId", "answers"])?;
        let key = request_id(input)?;
        self.transaction(|s| {
            let payload = json!({"intakeId":intake_id,"input":input});
            if let Some(prior) = s.replay(key, "intake-answer", &payload)? {
                return Ok(prior);
            }
            let mut intake = s.get_intake(intake_id)?;
            s.assert_intake_live(&intake)?;
            if intake["revision"] != input["revision"]
                || intake["sourceId"] != input["sourceId"]
                || intake["state"] != "waiting"
            {
                return Err(conflict("Questions changed. Keep your answers and reload."));
            }
            let task = s.get_task(intake["taskId"].as_str().unwrap())?;
            let run = s.get_run(intake["scopingRunId"].as_str().unwrap())?;
            if let Some(reason) = s.source_hold(&task, &run["intakeSource"])? {
                return Err(conflict(&reason));
            }
            let answers = input["answers"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= 3)
                .ok_or_else(|| conflict("Answer the current question batch."))?;
            let mut history = intake["questions"].as_array().cloned().unwrap_or_default();
            for answer in answers {
                strict(answer, &["id", "body"])?;
                crate::store::trimmed_text(answer, "body", 1, 8000)?;
                let question = history
                    .iter_mut()
                    .find(|q| {
                        q["id"] == answer["id"]
                            && q["sourceId"] == input["sourceId"]
                            && q["answer"].is_null()
                    })
                    .ok_or_else(|| {
                        conflict("Answer refers to a superseded or already answered question.")
                    })?;
                question["answer"] = json!({"body":answer["body"],"at":s.time(),"requestId":key});
            }
            let body = format!(
                "Operator answers for intake {} (source {}): {}",
                intake_id,
                input["sourceId"],
                crate::serialized::stringify(&input["answers"])
            );
            let message = s.steer(
                run["id"].as_str().unwrap(),
                &json!({"requestId":key,"body":body}),
            )?;
            for question in &mut history {
                if question["answer"]["requestId"] == key {
                    question["answer"]["steeringId"] = message["id"].clone();
                }
            }
            intake["questions"] = json!(history);
            if !intake["questions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|q| q["sourceId"] == input["sourceId"] && q["answer"].is_null())
            {
                intake["state"] = json!("answer_pending");
                intake["pendingMessageId"] = message["id"].clone();
            }
            s.save_intake(&mut intake)?;
            s.receipt(key, "intake-answer", &payload, &intake)?;
            Ok(intake)
        })
    }
    pub fn propose_intake_plan(
        &self,
        attempt_id: &str,
        tool_key: &str,
        input: &Value,
    ) -> Result<Value> {
        strict(input, &["sourceId", "content", "requirements"])?;
        let key = format!("intake-plan:{attempt_id}:{tool_key}");
        self.transaction(|s| {
            if let Some(prior)=s.replay(&key,"intake-plan",input)? {return Ok(prior);}
            let(mut intake,run)=s.intake_caller(attempt_id)?;
            if input["sourceId"]!=run["intakeSource"]["id"] || intake["inspection"]["sourceId"]!=input["sourceId"] {return Err(conflict("Inspect the pinned source before proposing a plan."));}
            if intake["state"]!="scoping" {return Err(conflict("An intake decision is already pending."));}
            let task_id=intake["taskId"].as_str().unwrap();
            let mut body=json!({"requestId":id(),"revision":s.get_task(task_id)?["revision"],"sourceId":input["sourceId"],"content":input["content"]});
            if let Some(prior)=s.implementation_plans(task_id)?.last() {body["predecessorId"]=prior["id"].clone();}
            if let Some(r)=input.get("requirements") {body["requirements"]=r.clone();}
            let plan=s.write_implementation_plan(task_id,&body,&json!({"kind":"coordinator","attemptId":attempt_id,"intakeId":intake["id"]}))?;
            intake["state"]=json!("review");intake["proposalId"]=plan["id"].clone();s.save_intake(&mut intake)?;
            s.set_desired_state(run["id"].as_str().unwrap(),"paused")?;
            s.begin_pause(run["id"].as_str().unwrap(),Some("intake"))?;
            s.receipt(&key,"intake-plan",input,&plan)?;Ok(plan)
        })
    }
    /// Marks resume intent consumed only after the existing lifecycle accepted it.
    pub fn intake_resumed(&self, intake_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut intake = s.get_intake(intake_id)?;
            s.assert_intake_live(&intake)?;
            if intake["state"] == "answer_pending"
                && s.get_run(intake["scopingRunId"].as_str().unwrap())?["status"] == "running"
            {
                intake["state"] = json!("scoping");
                s.save_intake(&mut intake)?;
            }
            Ok(intake)
        })
    }
}
