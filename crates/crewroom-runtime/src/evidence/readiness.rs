use super::*;
impl<S: StoreAccess> Evidence<S> {
    pub(super) fn check_blockers(&self, record: &Value, candidate: &Value) -> (Vec<String>, bool) {
        let mut blockers = Vec::new();
        let mut invalid = false;
        let required: Vec<_> = array(&record["requiredChecks"])
            .iter()
            .filter(|c| c["required"] == true)
            .collect();
        if required.is_empty() {
            blockers.push("No required repository checks have been configured.".into());
        }
        for check in required {
            let id = check["id"].as_str().unwrap_or("unknown");
            let result = array(&record["checks"])
                .iter()
                .rev()
                .find(|r| r["candidateId"] == candidate["id"] && r["checkId"] == check["id"]);
            if result.is_none_or(|r| {
                r["started"] != true
                    || r["completed"] != true
                    || r["sourceIntact"] != true
                    || r["exitCode"] != 0
                    || r["timedOut"] != false
            }) {
                blockers.push(format!("Required check {id} is missing or failed."));
                if result.is_some_and(|r| {
                    r["started"] == true
                        && (r["timedOut"] == true
                            || r["sourceIntact"] != true
                            || r["exitCode"] != 0)
                }) {
                    invalid = true;
                }
                continue;
            }
            let result = result.unwrap();
            if stringify(&result["definition"]) != stringify(check) {
                blockers.push(format!(
                    "Required check {id} has a different command policy."
                ));
                invalid = true;
            }
            for key in ["output", "source"] {
                if let Err(e) = self.read_artifact(&result[key]) {
                    blockers.push(format!("Required check {id}: {e}"));
                    break;
                }
            }
        }
        (blockers, invalid)
    }
    pub async fn assess(&self, run_id: &str) -> Result<Value> {
        if self
            .work
            .store(|s| Ok(array(&Self::record(s, run_id)?["candidates"]).is_empty()))?
        {
            return Ok(
                json!({"ready":false,"blockers":["No candidate revision has been recorded."],"confirmedInvalidation":false}),
            );
        }
        let current = self.work.status(run_id).await;
        // Read landed check/review receipts after the asynchronous Git observation.
        // Artifact IO occurs outside the global store lock; a final comparison
        // prevents a newly landed failure or policy change from being overlooked.
        let (record, runtime, work, choreography, mut blockers) = self.work.store(|s| {
            Ok((
                Self::record(s, run_id)?,
                s.runtime(run_id)?,
                s.repository_work(run_id)?,
                s.choreography(run_id)?,
                [
                    s.insertion_blockers(run_id)?,
                    s.choreography_blockers(run_id)?,
                ]
                .concat(),
            ))
        })?;
        let candidate = array(&record["candidates"])
            .last()
            .ok_or_else(|| conflict("Candidate disappeared."))?;
        let insertion = blockers.clone();
        let mut invalid = false;
        let binding = choreography.as_ref().map(|c| json!({"revision":c["revision"],"binding":c["binding"],"sha256":hash(stringify(c))}));
        if candidate.get("choreography") != binding.as_ref() {
            blockers.push("Package obligations changed after candidate freeze.".into());
            invalid = true;
        }
        if array(&work.as_ref().unwrap_or(&Value::Null)["refreshes"])
            .iter()
            .any(|r| {
                r["state"] == "rebasing"
                    || ["preparing", "continuing"]
                        .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
                    || r["state"] == "conflicted" && r.get("supersededBy").is_none()
            })
        {
            blockers.push(
                "A target conflict is unresolved; the previous candidate cannot be published."
                    .into(),
            );
            invalid = true;
        }
        match &current {
            Err(e) => blockers.push(e.to_string()),
            Ok(current) => {
                if current.get("continuationTargetRef").is_some() {
                    blockers.push("Refresh from the target after recording fresh implementation for this continuation.".into());
                }
                if current.get("writer").is_some()
                    || current["revision"]["dirty"] != false
                    || current["revision"] != candidate["revision"]
                {
                    blockers
                        .push("The repository revision or accepted requirements changed.".into());
                    invalid = true;
                }
            }
        }
        let pending: Vec<_> = array(&runtime["assignments"])
            .iter()
            .filter(|a| a.get("retiredAt").is_none() && a["rebriefRequired"] == true)
            .map(|a| a["name"].as_str().unwrap_or("unknown"))
            .collect();
        if !pending.is_empty() {
            blockers.push(format!(
                "Re-brief affected assignments: {}.",
                pending.join(", ")
            ));
            invalid = true;
        }
        let (checks, bad) = self.check_blockers(&record, candidate);
        blockers.extend(checks);
        invalid |= bad;
        let mut latest: Vec<&Value> = Vec::new();
        for review in array(&record["reviews"])
            .iter()
            .filter(|r| r["candidateId"] == candidate["id"])
        {
            if let Some(index) = latest
                .iter()
                .position(|r| r["assignmentId"] == review["assignmentId"])
            {
                latest[index] = review;
            } else {
                latest.push(review);
            }
        }
        if latest.is_empty() {
            blockers.push("Independent review is missing for this revision.".into());
        }
        for review in latest {
            if review["checkPolicyRevision"].as_u64().unwrap_or(0)
                != array(&record["checkPolicyChanges"]).len() as u64
            {
                blockers.push("Independent review predates the current check environment.".into());
                invalid = true;
            }
            if review["verdict"] != "pass" {
                blockers.push("Independent review requests changes.".into());
                invalid = true;
            }
            if let Err(e) = self.read_artifact(&review["artifact"]) {
                blockers.push(format!("Review: {e}"));
            }
        }
        self.work
            .store(|s| {
                if Self::record(s, run_id)? != record
                    || s.runtime(run_id)? != runtime
                    || s.repository_work(run_id)? != work
                    || s.choreography(run_id)? != choreography
                    || [
                        s.insertion_blockers(run_id)?,
                        s.choreography_blockers(run_id)?,
                    ]
                    .concat()
                        != insertion
                {
                    return Err(conflict(
                        "Evidence changed while readiness was being verified.",
                    ));
                }
                if let Ok(current) = &current {
                    Self::current(s, run_id, current)?;
                }
                Ok(())
            })
            .unwrap_or_else(|e| blockers.push(e.to_string()));
        Ok(
            json!({"ready":blockers.is_empty(),"candidate":candidate,"blockers":blockers,"confirmedInvalidation":invalid}),
        )
    }
}
