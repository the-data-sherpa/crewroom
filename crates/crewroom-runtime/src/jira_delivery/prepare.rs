use super::*;
fn report(input: &Value) -> Result<Value> {
    let kind = input["kind"].as_str().unwrap_or("");
    let allowed: &[&str] = match kind {
        "milestone" | "blocker" => &["kind", "eventKey", "body"],
        "evidence" | "pr_link" => &["kind", "eventKey", "candidateId"],
        "in_progress" | "research_outcome" => &["kind", "eventKey"],
        _ => return Err(Error::Invalid("Invalid Jira report kind.".into())),
    };
    if input
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(Error::Invalid("Unrecognized Jira report fields.".into()));
    }
    let event = input["eventKey"].as_str().unwrap_or("");
    if event.is_empty()
        || event.len() > 200
        || !event.as_bytes()[0].is_ascii_alphanumeric()
        || !event
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
        || kind == "in_progress" && event != "execution-started"
    {
        return Err(Error::Invalid("Invalid Jira event key.".into()));
    }
    let mut result = json!({"kind":kind,"eventKey":event});
    if allowed.contains(&"body") {
        let body = input["body"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Invalid Jira report body.".into()))?;
        let body = crewroom_core::store::trim_text(body);
        if !(1..=12000).contains(&body.chars().count()) {
            return Err(Error::Invalid("Invalid Jira report body length.".into()));
        }
        result["body"] = json!(body);
    }
    if allowed.contains(&"candidateId") {
        let id = input["candidateId"]
            .as_str()
            .filter(|id| crewroom_core::store::valid_uuid(id))
            .ok_or_else(|| Error::Invalid("Invalid Jira candidate UUID.".into()))?;
        result["candidateId"] = json!(id);
    }
    Ok(result)
}
impl<S: StoreAccess> JiraDelivery<S> {
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
        if task["source"] != "jira" || task["sourceSite"] != self.client.source["siteUrl"] {
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
    pub(super) async fn evidence_text(
        &self,
        run_id: &str,
        candidate_id: &str,
        publication: bool,
    ) -> Result<VerifiedText> {
        let evidence = self
            .evidence
            .as_ref()
            .ok_or_else(|| conflict("Verified evidence is unavailable."))?;
        let proof = Proof::capture(&self.store, run_id, publication)?;
        let assessment = evidence.assess(run_id).await?;
        if assessment["ready"] != true || assessment["candidate"]["id"] != candidate_id {
            return Err(conflict(
                "Evidence must cover the current verified candidate.",
            ));
        }
        self.store.with_store(|s|{
            proof.check(s)?;
            let record=s.coding_record(run_id)?.unwrap();
            let checks=array(&record["requiredChecks"]).iter().filter(|c|c["required"]==true).map(|c|c["id"].as_str().unwrap()).collect::<Vec<_>>().join(", ");
            let mut body=format!("Revision {}\nCandidate {candidate_id}\nRequired checks passed: {checks}\nIndependent review passed for this revision.\nMerge remains an operator decision.",text(&assessment["candidate"]["revision"],"head")?);
            if publication {
                let p=s.publication(run_id)?.unwrap_or(Value::Null);
                let expected=format!("https://bitbucket.org/{}/{}/pull-requests/{}",p["repository"]["workspace"].as_str().unwrap_or(""),p["repository"]["slug"].as_str().unwrap_or(""),p["pr"]["id"]);
                if p["candidateId"]!=candidate_id || p["pr"]["state"]!="confirmed" || p["remoteSourceHead"]!=p["revision"]["head"]
                    || p["observation"]["pr"]["source"]["commit"]["hash"]!=p["revision"]["head"] || p["observation"]["pr"]["state"]!="OPEN" || p["pr"]["url"]!=expected { return Err(conflict("A confirmed PR receipt for this candidate is required.")); }
                body.push_str(&format!("\n\nPull request: {expected}\nHosted validation must be checked against the current PR revision."));
            }
            Ok(VerifiedText{body,proof})
        })
    }
    pub(super) fn research_text(&self, run_id: &str) -> Result<VerifiedText> {
        let research = self
            .research
            .as_ref()
            .ok_or_else(|| conflict("Research evidence is unavailable."))?;
        let proof = Proof::capture(&self.store, run_id, false)?;
        let result = research.read(run_id)?;
        self.store.with_store(|s|{
            proof.check(s)?;let run=s.get_run(run_id)?;
            if run["purpose"]!="research" || result["assessment"]["ready"]!=true || !present(&run["researchCompletion"]) || run["researchCompletion"]["revision"]!=run["taskRevision"] { return Err(conflict("A requested, fully verified research outcome is required before reporting to Jira.")); }
            let artifacts=array(&result["artifacts"]).iter().filter(|a|a["state"]=="verified" && !present(&a["supersededBy"]) && a["requirementRevision"]==run["taskRevision"])
                .map(|a|format!("- {} ({}), SHA-256 {}\n  Limitations: {}",a["title"].as_str().unwrap(),a["kind"].as_str().unwrap(),a["body"]["sha256"].as_str().unwrap(),a["limitations"].as_str().unwrap())).collect::<Vec<_>>().join("\n");
            Ok(VerifiedText{body:format!("Research outcome completed for accepted revision {}.\n{artifacts}\nCrewroom reference: run {run_id}, /api/runs/{run_id}/result\nNo code change or PR is claimed.",run["taskRevision"]),proof})
        })
    }
}
