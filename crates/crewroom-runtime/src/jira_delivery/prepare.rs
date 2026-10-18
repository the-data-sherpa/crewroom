use super::*;
fn report(input: &Value) -> Result<Value> {
    crate::delivery_evidence::report(input, "Jira")
}
impl<S: StoreAccess> JiraDelivery<S> {
    pub(super) async fn evidence_text(
        &self,
        run_id: &str,
        candidate_id: &str,
        publication: bool,
    ) -> Result<VerifiedText> {
        crate::delivery_evidence::evidence_text(
            &self.store,
            self.evidence.as_ref(),
            "Jira",
            run_id,
            candidate_id,
            publication,
        )
        .await
    }
    pub(super) fn research_text(&self, run_id: &str) -> Result<VerifiedText> {
        crate::delivery_evidence::research_text(&self.store, self.research.as_ref(), "Jira", run_id)
    }
    fn authorize(
        &self,
        s: &Store,
        attempt_id: &str,
        value: &Value,
    ) -> Result<(Value, Value, Option<Value>)> {
        let attempt = s.get_attempt(attempt_id)?;
        let run = s.get_run(text(&attempt, "runId")?)?;
        let runtime = s.runtime(text(&run, "id")?)?;
        let assignment = array(&runtime["assignments"])
            .iter()
            .find(|a| a["currentAttemptId"] == attempt_id);
        let reporter = assignment
            .is_some_and(|a| a["coordinator"] != true && a["role"]["id"] == "delivery-reporter");
        let fallback = assignment.is_some_and(|a| a["coordinator"] == true)
            && value["kind"] == "research_outcome"
            && run["purpose"] == "research"
            && !array(&runtime["assignments"])
                .iter()
                .any(|a| !present(&a["retiredAt"]) && a["role"]["id"] == "delivery-reporter");
        if assignment.is_none_or(|a| present(&a["retiredAt"]))
            || (!reporter && !fallback)
            || attempt["ownership"] != "held"
            || !matches!(attempt["state"].as_str(), Some("idle" | "executing"))
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || present(&run["sourceConflict"])
        {
            return Err(conflict(
                "Only the active delivery reporter, or a research coordinator without one, may request Jira updates.",
            ));
        }
        let task = s.get_task(text(&run, "taskId")?)?;
        if !crewroom_core::ticket_source::belongs_to(
            &task,
            crewroom_core::ticket_source::JIRA,
            &self.client.source["siteUrl"],
        ) {
            return Err(conflict(
                "This run does not belong to the configured Jira site.",
            ));
        }
        let prior = s
            .jira_updates(Some(text(&run, "id")?))?
            .into_iter()
            .find(|u| u["eventKey"] == value["eventKey"]);
        if prior.as_ref().is_some_and(|p| {
            p["kind"] != value["kind"]
                || value
                    .get("candidateId")
                    .is_some_and(|v| p["candidateId"] != *v)
                || value.get("body").is_some_and(|v| p["body"] != *v)
        }) {
            return Err(conflict("Jira event key has different content."));
        }
        if run["purpose"] == "research"
            && !matches!(
                value["kind"].as_str(),
                Some("milestone" | "blocker" | "research_outcome")
            )
        {
            return Err(conflict(
                "Research does not report coding evidence or Jira transitions.",
            ));
        }
        if value["kind"] == "research_outcome"
            && (run["purpose"] != "research"
                || value["eventKey"] != format!("research-outcome-{}", run["taskRevision"]))
        {
            return Err(conflict(
                "Research outcome event key must identify the accepted revision.",
            ));
        }
        Ok((run, task, prior))
    }
    pub async fn prepare(&self, attempt_id: &str, input: &Value) -> Result<Value> {
        let value = report(input)?;
        let (first, _, prior) = self.flight.with_open(|| {
            self.store
                .with_store(|s| self.authorize(s, attempt_id, &value))
        })?;
        if let Some(prior) = prior {
            return Ok(prior);
        }
        let verified = match value["kind"].as_str().unwrap() {
            "evidence" | "pr_link" => Some(
                self.evidence_text(
                    text(&first, "id")?,
                    text(&value, "candidateId")?,
                    value["kind"] == "pr_link",
                )
                .await?,
            ),
            "research_outcome" => Some(self.research_text(text(&first, "id")?)?),
            _ => None,
        };
        self.flight.with_open(||self.store.with_store(|s|{
            let (run,task,prior)=self.authorize(s,attempt_id,&value)?;
            if let Some(prior)=prior { return Ok(prior); }
            if let Some(v)=&verified {v.proof.check(s)?;}
            let mut update=value.clone();
            for (key,value) in json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run["id"],"attemptId":attempt_id,"siteUrl":task["sourceSite"],"issueId":task["sourceId"],"state":"pending","createdAt":time()}).as_object().unwrap() { update[key]=value.clone(); }
            if value["kind"]=="in_progress" {
                let source=&self.client.source;
                if source["inProgressStatusId"].as_str().is_none() || source["inProgressTransitionId"].as_str().is_none() { return Err(conflict("Jira progress transition is not configured.")); }
                update["statusId"]=source["inProgressStatusId"].clone();update["transitionId"]=source["inProgressTransitionId"].clone();
            }
            if let Some(verified)=verified {update["body"]=json!(verified.body);}
            s.save_jira_update(&update)
        }))
    }
}
