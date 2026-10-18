use super::*;
impl<S: StoreAccess> Repository<S> {
    fn implementation_claim(
        s: &Store,
        coordinator_id: &str,
        key: &str,
        input: &Value,
        id: &str,
    ) -> Result<(Value, bool)> {
        let coordinator = s.get_attempt(coordinator_id)?;
        let run_id = text(&coordinator, "runId")?;
        let run = s.get_run(run_id)?;
        let runtime = s.runtime(run_id)?;
        let team = array(&runtime["assignments"]);
        if coordinator["ownership"] != "held"
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || !team.iter().any(|a| {
                a["coordinator"] == true
                    && a["currentAttemptId"] == coordinator_id
                    && a.get("retiredAt").is_none()
            })
        {
            return Err(conflict(
                "Only the active coordinator may assign implementation.",
            ));
        }
        let writer = team
            .iter()
            .find(|a| {
                a["name"] == input["assignee"]
                    && a["coordinator"] == false
                    && a.get("retiredAt").is_none()
            })
            .ok_or_else(|| conflict("Specialist or coding policy is unavailable."))?;
        let record = s
            .coding_record(run_id)?
            .ok_or_else(|| conflict("Specialist or coding policy is unavailable."))?;
        let work = s
            .repository_work(run_id)?
            .ok_or_else(|| conflict("Repository work is unavailable."))?;
        let scope = Self::active_scope(&work);
        if Self::unresolved_scope(&work).is_some() && scope.is_none() {
            return Err(conflict(
                "Target conflict requires an operator decision before implementation.",
            ));
        }
        if scope.is_some_and(|r| r["resolutionWorkspace"]["state"] == "ready") {
            return Err(conflict(
                "Conflict rebase is ready; submit completion instead of assigning another writer.",
            ));
        }
        let candidate = array(&record["candidates"]).last();
        let publication = s.publication(run_id)?.unwrap_or(Value::Null);
        let feedback = array(&run["feedbackContinuations"]).iter().rev().find(|f| {
            f["publicationId"] == publication["id"]
                && f["prId"] == publication["pr"]["id"]
                && f["branch"] == publication["branch"]
                && candidate.is_some_and(|c| {
                    (["starting", "handed_off"].contains(&f["state"].as_str().unwrap_or(""))
                        && f["sourceCandidateId"] == c["id"]
                        && f["sourceHead"] == c["revision"]["head"])
                        || (f["state"] == "candidate"
                            && f["candidateId"] == c["id"]
                            && c["feedbackFrom"]["continuationId"] == f["id"]
                            && f["sourceCandidateId"] == publication["candidateId"])
                })
        });
        let allowed = feedback.is_some()
            && scope.is_none()
            && !publication.is_null()
            && publication
                .get("merged")
                .is_none_or(|v| v == false || v.is_null())
            && publication["pr"]["providerState"] == "OPEN"
            && publication["pr"]["state"] == "confirmed"
            && publication["push"]["state"] == "confirmed";
        let feedback_id = feedback.filter(|_| allowed).and_then(|f| f.get("id"));
        if let Some(prior) = array(&record["claims"])
            .iter()
            .find(|c| c["coordinatorAttemptId"] == coordinator_id && c["toolCallId"] == key)
        {
            let same_scope = prior.get("targetRefreshId") == scope.and_then(|r| r.get("requestId"))
                || scope.is_none()
                    && array(&work["refreshes"]).iter().any(|r| {
                        Some(&r["requestId"]) == prior.get("targetRefreshId")
                            && r["state"] == "completed"
                    });
            if s.get_attempt(text(prior, "writerAttemptId")?)?["assignmentId"] != writer["id"]
                || prior["instruction"] != input["instruction"]
                || !same_scope
                || prior.get("feedbackContinuationId") != feedback_id
            {
                return Err(conflict(
                    "Implementation claim has different content or workspace scope.",
                ));
            }
            return Ok((prior.clone(), true));
        }
        if work.get("writer").is_some() {
            return Err(conflict(
                "Another implementation claim still owns repository writes.",
            ));
        }
        if array(&record["completions"])
            .iter()
            .any(|c| c["state"] == "requested")
        {
            return Err(conflict(
                "Reconcile pending implementation completion first.",
            ));
        }
        let adopted = candidate
            .is_some_and(|c| c["revision"]["requirementsHash"] != work["requirementsHash"]);
        let first_feedback =
            allowed && candidate.is_some_and(|c| c["id"] == publication["candidateId"]);
        if let Some(candidate) = candidate {
            let failed = array(&record["requiredChecks"])
                .iter()
                .filter(|c| c["required"] == true)
                .any(|check| {
                    array(&record["checks"])
                        .iter()
                        .rev()
                        .find(|r| {
                            r["candidateId"] == candidate["id"] && r["checkId"] == check["id"]
                        })
                        .is_some_and(|r| {
                            r["started"] != true
                                || r["completed"] != true
                                || r["sourceIntact"] != true
                                || r["exitCode"] != 0
                                || r["timedOut"] == true
                        })
                });
            let mut reviews = HashMap::new();
            for review in array(&record["reviews"])
                .iter()
                .filter(|r| r["candidateId"] == candidate["id"])
            {
                reviews.insert(text(review, "assignmentId")?, &review["verdict"]);
            }
            if scope.is_none()
                && !adopted
                && !failed
                && !reviews.values().any(|v| **v == "changes_requested")
                && !first_feedback
            {
                return Err(conflict(
                    "Complete validation of the current revision before assigning automatic rework.",
                ));
            }
        }
        let mut unfinished = false;
        for claim in array(&record["claims"])
            .iter()
            .rev()
            .filter(|c| c.get("candidateId") == candidate.and_then(|c| c.get("id")))
        {
            if s.get_attempt(text(claim, "writerAttemptId")?)?["assignmentId"] == writer["id"]
                && !array(&record["completions"]).iter().any(|c| {
                    c["attemptId"] == claim["writerAttemptId"] && c["state"] != "requested"
                })
            {
                unfinished = true;
                break;
            }
        }
        let mut claim = json!({"id":id,"coordinatorAttemptId":coordinator_id,"toolCallId":key,"writerAttemptId":writer["currentAttemptId"],"instruction":input["instruction"],"automatic":!first_feedback && scope.is_none() && candidate.is_some() && !unfinished && !adopted});
        if let Some(handoff) = array(&record["completions"]).iter().rev().find(|c| {
            c["mode"] == "checkpoint"
                && c["state"] == "completed"
                && c["successor"] == writer["name"]
                && !array(&record["claims"])
                    .iter()
                    .any(|claim| claim["handoffId"] == c["id"])
        }) {
            claim["handoffId"] = handoff["id"].clone();
            claim["automatic"] = json!(false);
        }
        if let Some(scope) = scope {
            claim["targetRefreshId"] = scope["requestId"].clone();
        }
        if let Some(candidate) = candidate {
            claim["candidateId"] = candidate["id"].clone();
        }
        if let Some(id) = feedback_id {
            claim["feedbackContinuationId"] = id.clone();
        }
        Ok((claim, false))
    }
    pub async fn claim_implementation(
        &self,
        coordinator_id: &str,
        key: &str,
        input: &Value,
    ) -> Result<Value> {
        request(input, &["assignee", "instruction"])?;
        limited(input, "assignee", 1, usize::MAX)?;
        let instruction = crewroom_core::store::trim_text(text(input, "instruction")?);
        let normalized = json!({"assignee":input["assignee"],"instruction":instruction});
        limited(&normalized, "instruction", 1, 40000)?;
        Self::tool_key(key)?;
        let attempt = self.store(|s| s.get_attempt(coordinator_id))?;
        let run_id = text(&attempt, "runId")?;
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let (claim, replay) =
            self.store(|s| Self::implementation_claim(s, coordinator_id, key, &normalized, &id))?;
        if replay {
            return Ok(claim);
        }
        self.claim_writer_unlocked(text(&claim, "writerAttemptId")?, Some(&claim), |s| {
            let current = Self::implementation_claim(s, coordinator_id, key, &normalized, &id)?;
            if current != (claim.clone(), false) {
                return Err(conflict(
                    "Implementation claim changed before writer admission.",
                ));
            }
            Ok(())
        })
        .await?;
        Ok(claim)
    }
    fn restore_claim(s: &Store, run_id: &str, id: &str) -> Result<Option<Value>> {
        if !s.choreography_current(run_id)? {
            return Ok(None);
        }
        let Some(record) = s.coding_record(run_id)? else {
            return Ok(None);
        };
        let Some(claim) = array(&record["claims"]).last() else {
            return Ok(None);
        };
        let work = s.repository_work(run_id)?.unwrap_or(Value::Null);
        let unresolved = Self::unresolved_scope(&work);
        if unresolved.is_some_and(|r| {
            Self::active_scope(&work).is_none() || claim["targetRefreshId"] != r["requestId"]
        }) || unresolved.is_none() && claim.get("targetRefreshId").is_some()
            || claim.get("candidateId")
                != array(&record["candidates"])
                    .last()
                    .and_then(|c| c.get("id"))
            || array(&record["completions"])
                .iter()
                .any(|c| c["attemptId"] == claim["writerAttemptId"])
        {
            return Ok(None);
        }
        let prior = s.get_attempt(text(claim, "writerAttemptId")?)?;
        let runtime = s.runtime(run_id)?;
        let team = array(&runtime["assignments"]);
        let writer = team
            .iter()
            .find(|a| a["id"] == prior["assignmentId"] && a.get("retiredAt").is_none());
        if writer.is_none() && prior["ownership"] == "stopped" {
            return Ok(None);
        }
        if writer.is_some_and(|a| !a["rebriefRequired"].is_null()) {
            return Ok(None);
        }
        let writer = writer
            .filter(|_| prior["ownership"] == "stopped")
            .ok_or_else(|| conflict("Interrupted implementation ownership is unresolved."))?;
        if writer["currentAttemptId"] == prior["id"] {
            return Ok(None);
        }
        let coordinator = team
            .iter()
            .find(|a| a["coordinator"] == true && a.get("retiredAt").is_none())
            .ok_or_else(|| conflict("Coordinator unavailable."))?;
        let mut restored = claim.clone();
        restored["id"] = json!(id);
        restored["writerAttemptId"] = writer["currentAttemptId"].clone();
        restored["coordinatorAttemptId"] = coordinator["currentAttemptId"].clone();
        restored["toolCallId"] = json!(format!("restore:{}", text(writer, "currentAttemptId")?));
        restored["automatic"] = json!(false);
        restored["restoredFrom"] = prior["id"].clone();
        Ok(Some(restored))
    }
    pub async fn restore_writer(&self, run_id: &str) -> Result<Option<Value>> {
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let Some(claim) = self.store(|s| Self::restore_claim(s, run_id, &id))? else {
            return Ok(None);
        };
        self.claim_writer_unlocked(text(&claim, "writerAttemptId")?, Some(&claim), |s| {
            if Self::restore_claim(s, run_id, &id)?.as_ref() != Some(&claim) {
                return Err(conflict(
                    "Interrupted implementation claim changed before restoration.",
                ));
            }
            Ok(())
        })
        .await?;
        Ok(Some(claim))
    }
}
