use super::*;
use context::host_input;

fn idle(state: &Value) -> bool {
    !["isStreaming", "isCompacting", "hasPendingAsyncWork"]
        .iter()
        .any(|key| state[*key] == true)
}
impl<S: StoreAccess> Workflow<S> {
    fn can_deliver(&self, run: &str) -> Result<bool> {
        if self.check_open().is_err() {
            return Ok(false);
        }
        if self.evaluation_is_finalizing(run)? {
            return Ok(false);
        }
        let (current, runtime) = self.store(|s| Ok((s.get_run(run)?, s.runtime(run)?)))?;
        if !matches!(
            current["status"].as_str(),
            Some("running" | "awaiting_runner")
        ) || current["desiredState"] != "running"
            || present(&current, "sourceConflict")
        {
            return Ok(false);
        }
        let active = active(&runtime);
        Ok(!array(&self.0.hosts.observation(run)).iter().any(|v| {
            active
                .iter()
                .any(|a| a["currentAttemptId"] == v["attemptId"])
                && matches!(
                    v["observation"]["state"].as_str(),
                    Some("blocked" | "recovery_required" | "degraded")
                )
        }))
    }
    pub(super) async fn reconcile_deliveries(
        &self,
        run: &str,
        messages: &Messaging<S>,
    ) -> Result<()> {
        for message in array(&self.store(|s| s.runtime(run))?["messages"]) {
            if matches!(
                message["disposition"].as_str(),
                Some("pending" | "uncertain")
            ) {
                messages.reconcile(text(message, "id")?).await?;
            }
        }
        let (runtime, steering, notices) =
            self.store(|s| Ok((s.runtime(run)?, s.steering(run)?, s.delivery_notices(run)?)))?;
        let mut pending = Vec::new();
        for m in array(&runtime["messages"])
            .iter()
            .filter(|m| m["disposition"] == "dispatched")
        {
            pending.push((
                text(m, "id")?.to_owned(),
                text(m, "recipientAttemptId")?.to_owned(),
                "peer",
            ));
        }
        for m in steering.iter().filter(|m| m["delivery"] == "dispatched") {
            pending.push((
                text(m, "inputId")?.to_owned(),
                text(m, "attemptId")?.to_owned(),
                "steering",
            ));
        }
        for m in notices.iter().filter(|m| m["disposition"] == "dispatched") {
            pending.push((
                text(m, "id")?.to_owned(),
                text(m, "attemptId")?.to_owned(),
                "notice",
            ));
        }
        for (id, attempt, kind) in pending {
            let status = if self.0.hosts.observes(&attempt) {
                self.0
                    .hosts
                    .request(&attempt, &json!({"op":"input_status","id":id}))
                    .await?["disposition"]
                    .clone()
            } else {
                crate::host_journal::saved_inputs(&self.0.config.root.join(&attempt))?
                    .into_iter()
                    .find(|v| v["id"] == id)
                    .map_or(json!("absent"), |v| v["disposition"].clone())
            };
            self.store(|s| {
                if matches!(status.as_str(),Some("received"|"completed"|"error"|"aborted")) {
                    match kind {"peer"=>s.record_dispatch(&id,true),"notice"=>s.dispatch_delivery_notice(&attempt,&id,true).map(|_|()),_=>s.receive_steering(&attempt,&id)}
                }else if status=="absent" && s.get_attempt(&attempt)?["ownership"]=="stopped" {s.restore_undispatched_input(&attempt,&id)}
                else {Err(conflict("Input dispatch outcome is uncertain. Reconcile retained receipts before further work."))}
            })?;
        }
        Ok(())
    }
    async fn deliver(&self, run: &str) -> Result<()> {
        let messages = self.messages(run)?;
        self.reconcile_deliveries(run, &messages).await?;
        messages.observe(run).await?;
        if !self.can_deliver(run)? {
            return Ok(());
        }
        for mut message in array(&self.store(|s| s.runtime(run))?["messages"]).to_vec() {
            if matches!(message["disposition"].as_str(), Some("stored" | "accepted")) {
                message = messages.accept(text(&message, "id")?).await?;
            }
            if message["disposition"] != "accepted" {
                continue;
            }
            if !self.can_deliver(run)? {
                return Ok(());
            }
            let runtime = self.store(|s| s.runtime(run))?;
            let recipient = active(&runtime)
                .into_iter()
                .find(|a| a["id"] == message["to"])
                .ok_or_else(|| conflict("Message recipient is no longer active."))?;
            let attempt = text(recipient, "currentAttemptId")?;
            if !self.0.hosts.observes(attempt) {
                continue;
            }
            let state = self
                .0
                .hosts
                .request(attempt, &json!({"op":"state"}))
                .await?;
            if !self.can_deliver(run)? {
                return Ok(());
            }
            if !idle(&state) {
                continue;
            }
            let id = text(&message, "id")?;
            let mut body = json!({"messageId":id,"from":message["from"],"body":message["body"]});
            if let Some(reply) = message.get("replyTo") {
                body["replyTo"] = reply.clone();
            }
            let input = host_input(id, &body, None)?;
            self.store(|s| s.record_dispatch(id, false))?;
            self.0.hosts.request(attempt, &input).await?;
            self.store(|s| s.record_dispatch(id, true))?;
        }
        let runtime = self.store(|s| s.runtime(run))?;
        let Some(coordinator) = active(&runtime)
            .into_iter()
            .find(|a| a["coordinator"] == true)
        else {
            return Ok(());
        };
        let attempt = text(coordinator, "currentAttemptId")?;
        if !self.0.hosts.observes(attempt) {
            return Ok(());
        }
        for message in self
            .store(|s| s.steering(run))?
            .iter()
            .filter(|m| m["delivery"] == "pending")
        {
            if !self.can_deliver(run)? {
                return Ok(());
            }
            let id = format!("steering:{}", text(message, "id")?);
            let input = host_input(
                &id,
                &json!({"instruction":"Reconcile this operator instruction and report its disposition through steering_receipt.","steering":message}),
                Some("steer"),
            )?;
            self.store(|s| s.dispatch_steering(attempt, &[text(message, "id")?.into()], &id))?;
            self.0.hosts.request(attempt, &input).await?;
            self.store(|s| s.receive_steering(attempt, &id))?;
        }
        if self.store(|s| s.get_run(run))?["status"] == "running"
            && self.can_deliver(run)?
            && let Some(notice) = self
                .store(|s| s.delivery_notices(run))?
                .into_iter()
                .find(|n| n["disposition"] == "pending")
        {
            let state = self
                .0
                .hosts
                .request(attempt, &json!({"op":"state"}))
                .await?;
            if idle(&state) && self.can_deliver(run)? {
                let id = text(&notice, "id")?;
                let input = host_input(
                    id,
                    &json!({"type":"delivery_update","instruction":notice_instruction(&notice),"notice":notice}),
                    None,
                )?;
                self.store(|s| s.dispatch_delivery_notice(attempt, id, false))?;
                self.0.hosts.request(attempt, &input).await?;
                self.store(|s| s.dispatch_delivery_notice(attempt, id, true))?;
            }
        }
        Ok(())
    }
    pub(super) async fn reconcile(&self, run: &str) -> Result<Value> {
        if self.check_open().is_err() {
            return Ok(Value::Null);
        }
        if self.evaluation_is_finalizing(run)? {
            self.finalize_evaluation(run).await?;
            return Ok(Value::Null);
        }
        let current = self.store(|s| s.account_time(run))?;
        let runtime = self.store(|s| s.runtime(run))?;
        if !matches!(
            current["status"].as_str(),
            Some("running" | "awaiting_runner" | "pausing")
        ) && array(&runtime["attempts"])
            .iter()
            .all(|a| a["state"] == "admitted" || a["ownership"] == "stopped")
        {
            return Ok(Value::Null);
        }
        let active = active(&runtime);
        let observations = self.0.hosts.observation(run);
        let mut blocked = current["sourceConflict"].as_str().map(String::from);
        let mut failed = HashSet::new();
        for observed in array(&observations) {
            let Some(assignment) = active
                .iter()
                .find(|a| a["currentAttemptId"] == observed["attemptId"])
            else {
                continue;
            };
            match observed["observation"]["state"].as_str() {
                Some("blocked") => {
                    blocked = Some(
                        observed["observation"]["reason"]
                            .as_str()
                            .unwrap_or("Process ownership or model policy requires attention.")
                            .into(),
                    )
                }
                Some("recovery_required") => {
                    failed.insert(text(assignment, "id")?.into());
                }
                _ => {}
            }
        }
        if let Some(blocked) = blocked {
            self.store(|s| s.clear_resume_request(run, "held"))?;
            self.stop_run(run, None).await?;
            return self.store(|s| s.require_attention(run, &blocked));
        }
        if current["status"] == "pausing"
            && matches!(current["pauseReason"].as_str(), Some("budget" | "merged"))
        {
            self.store(|s| {
                if s.get_run(run)?["desiredState"] != "cancelled" {
                    s.set_desired_state(run, "paused")?;
                }
                Ok(())
            })?;
            return self.stop_run(run, None).await;
        }
        if !failed.is_empty()
            && current["desiredState"] == "running"
            && matches!(
                current["status"].as_str(),
                Some("running" | "awaiting_runner")
            )
        {
            if let Err(e) = self.continue_run(run, revision(&current)?, &failed).await
                && self.store(|s| s.get_run(run))?["desiredState"] == "running"
            {
                self.store(|s| s.require_attention(run, &e.to_string()))?;
            }
            return self.snapshot(run);
        }
        if !self.can_deliver(run)? {
            return Ok(Value::Null);
        }
        if let Err(e) = self.deliver(run).await {
            self.failure(run, &e);
            self.stop_run(run, None).await?;
            return self.store(|s| s.require_attention(run, &e.to_string()));
        }
        if current["purpose"] == "evaluation" {
            self.finish_evaluation_if_ready(run).await?;
            return Ok(Value::Null);
        }
        self.apply_pending_coding(run).await?;
        self.apply_pending_team(run).await?;
        self.apply_pending_research(run).await?;
        Ok(Value::Null)
    }
    pub(super) async fn agents_idle(&self, run: &str) -> Result<bool> {
        let runtime = self.store(|s| s.runtime(run))?;
        if array(&runtime["messages"])
            .iter()
            .any(|m| m["disposition"] != "received")
        {
            return Ok(false);
        }
        for assignment in active(&runtime) {
            let attempt = text(assignment, "currentAttemptId")?;
            if self.store(|s| s.get_attempt(attempt))?["state"] != "idle" {
                return Ok(false);
            }
            if !self.0.hosts.observes(attempt) {
                return Err(conflict("Transition lacks confirmed agent ownership."));
            }
            if !idle(
                &self
                    .0
                    .hosts
                    .request(attempt, &json!({"op":"state"}))
                    .await?,
            ) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
fn notice_instruction(notice: &Value) -> String {
    match notice["payload"]["kind"].as_str() {
        Some("target_refresh")=>"The controller rebased the owned branch onto a newer target. The candidate in this notice needs its required checks and independent review; earlier evidence does not cover it. This notice is not operator steering or peer mail.".into(),
        Some("target_conflict_rejected")=>"The live target changed before conflict promotion. The completion is terminally rejected; the isolated resolution remains and the original currently matches its recorded clean source. Require an explicit reasoned retry; do not replay the completion or resume agents.".into(),
        Some("target_conflict_interrupted")=>"The checked reset stopped without promoting the conflict. Its exact completion is terminal; the isolated resolution remains for an explicit reasoned retry, even at the same target. Do not replay the completion or resume agents.".into(),
        Some("operator_specialist_proposed")=>"An operator specialist proposal was recorded. Follow its recorded status and instruction below within the accepted requirements. Crewroom admits a pending proposal at a safe coordination point; do not duplicate it. After reconciling its results, retire the inserted assignment through propose_team with a reason so readiness can be reassessed. A rejected proposal is historical; wait for a new operator request.".into(),
        Some("github")=>"A saved GitHub issue delivery receipt changed. Inspect github_updates; pending, uncertain or blocked is not delivered, and the issue's GitHub open/closed state is separate from workflow progress. This notice is not operator steering or peer mail.".into(),
        Some("team_proposal_rejected")=>format!("The team proposal was rejected for the recorded reason: {}. No assignment was admitted. {}",notice["payload"]["reason"].as_str().unwrap_or("unavailable"),if notice["payload"]["origin"]=="operator"{"Wait for a new operator proposal; do not recreate the rejected request."}else{"Choose an available team consistent with the accepted requirements before proposing another change."}),
        _=>"A saved provider observation changed. Inspect publication_status or jira_updates as appropriate. Reconcile confirmed actions before continuing; this notice is not operator steering or peer mail.".into()
    }
}
