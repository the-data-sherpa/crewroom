use super::*;
impl<S: StoreAccess> Evidence<S> {
    /// Register the source-bound refreshed candidate and its target receipt in
    /// one transaction. An interrupted caller may replay without duplicating it.
    pub async fn associate_refresh(&self, run_id: &str, request_id: &str) -> Result<Value> {
        let lock = self.work.lock(run_id);
        let _guard = lock.lock().await;
        let saved = self.work.store(|s| {
            let work = s.repository_work(run_id)?.unwrap_or(Value::Null);
            array(&work["refreshes"])
                .iter()
                .find(|r| r["requestId"] == request_id)
                .cloned()
                .ok_or_else(|| conflict("Target refresh is not recorded."))
        })?;
        if saved["state"] != "completed" {
            return Err(conflict("Target refresh has not completed."));
        }
        if saved.get("resolutionWorkspace").is_some() {
            return Err(conflict(
                "Conflict refresh evidence must be recorded through its completion transaction.",
            ));
        }
        if let Some(id) = saved["candidateId"].as_str() {
            return self.work.store(|s| {
                let record = Self::record(s, run_id)?;
                array(&record["candidates"])
                    .iter()
                    .find(|c| c["id"] == id)
                    .cloned()
                    .ok_or_else(|| conflict("Refreshed candidate is unavailable."))
            });
        }
        let current = self.work.status(run_id).await?;
        if current["revision"]["head"] != saved["toHead"]
            || current["revision"]["base"] != saved["toBase"]
        {
            return Err(conflict(
                "Refreshed source changed before candidate registration.",
            ));
        }
        self.work.store(|s| {
            s.transaction(|s| {
                Self::current(s, run_id, &current)?;
                if !array(&current["refreshes"]).contains(&saved) {
                    return Err(conflict(
                        "Target receipt changed before candidate registration.",
                    ));
                }
                let mut result = Value::Null;
                s.update_coding_record(run_id, |previous| {
                    let mut record =
                        previous.ok_or_else(|| conflict("Coding evidence is not configured."))?;
                    let source = array(&record["candidates"])
                        .last()
                        .filter(|c| {
                            c["id"] == saved["sourceCandidateId"]
                                && c["revision"]["head"] == saved["fromHead"]
                                && c["revision"]["base"] == saved["fromBase"]
                        })
                        .ok_or_else(|| {
                            conflict("Refresh source candidate changed before association.")
                        })?;
                    let authors = serde_json::from_value::<Vec<String>>(
                        source["implementationAttempts"].clone(),
                    )?;
                    result = Self::candidate(
                        s,
                        run_id,
                        &mut record,
                        &authors,
                        &current,
                        Some(text(&saved, "sourceCandidateId")?),
                        None,
                    )?;
                    Ok(record)
                })?;
                s.finish_target_refresh(
                    run_id,
                    request_id,
                    &json!({"candidateId":result["id"]}),
                    None,
                )?;
                if saved["fromBase"] != saved["toBase"] || saved["fromHead"] != saved["toHead"] {
                    s.notice_delivery(run_id, &json!({
                        "kind":"target_refresh", "subjectId":request_id,
                        "fromBase":saved["fromBase"], "toBase":saved["toBase"],
                        "candidateId":result["id"], "head":result["revision"]["head"],
                        "instruction":format!("The target moved. The owned branch was rebased onto it as candidate {}. Run its required checks and independent review before publishing it; earlier evidence does not cover it.", text(&result,"id")?)
                    }))?;
                }
                Ok(result)
            })
        })
    }
    /// Revalidate the store half of an asynchronous source observation before
    /// accepting proof. No await may separate this guard from its transaction.
    pub(crate) fn current(s: &Store, run_id: &str, current: &Value) -> Result<()> {
        let saved = s
            .repository_work(run_id)?
            .ok_or_else(|| conflict("Repository ownership disappeared."))?;
        let mut observed = current.clone();
        let fields = observed
            .as_object_mut()
            .ok_or_else(|| conflict("Repository observation unavailable."))?;
        fields.remove("revision");
        fields.remove("drift");
        if saved != observed
            || Repository::<S>::current_requirements(s, &saved)?
                != current["revision"]["requirementsHash"]
        {
            return Err(conflict(
                "Repository ownership or accepted requirements changed during observation.",
            ));
        }
        Ok(())
    }
    pub(super) fn candidate(
        s: &Store,
        run_id: &str,
        record: &mut Value,
        attempts: &[String],
        current: &Value,
        refreshed: Option<&str>,
        feedback: Option<&Value>,
    ) -> Result<Value> {
        Self::current(s, run_id, current)?;
        let choreography_blockers = [
            s.choreography_blockers(run_id)?,
            s.insertion_blockers(run_id)?,
        ]
        .concat();
        if !choreography_blockers.is_empty() {
            return Err(conflict(&choreography_blockers.join(" ")));
        }
        if current.get("writer").is_some()
            || current["revision"]["dirty"] != false
            || ["base", "requirements", "instructions"]
                .iter()
                .any(|k| current["drift"][k] == true)
        {
            return Err(conflict(
                "A clean candidate with stable base, requirements and project instructions is required.",
            ));
        }
        if attempts.is_empty() || attempts.iter().collect::<HashSet<_>>().len() != attempts.len() {
            return Err(conflict(
                "Candidate requires its implementation attempt identities.",
            ));
        }
        for id in attempts {
            let attempt = s.get_attempt(id)?;
            if attempt["runId"] != run_id || attempt["sessionId"].as_str().is_none_or(str::is_empty)
            {
                return Err(conflict(
                    "Implementation attempt is outside this run or has no verified session.",
                ));
            }
            if attempt["ownership"] != "stopped" {
                return Err(conflict(
                    "Implementation ownership must be stopped before recording a candidate.",
                ));
            }
        }
        let choreography = s.choreography(run_id)?;
        let choreography_binding = choreography.as_ref().map(|c| json!({"revision":c["revision"],"binding":c["binding"],"sha256":hash(stringify(c))}));
        let previous = array(&record["candidates"]).last();
        if let Some(previous) = previous.filter(|p| {
            p["revision"] == current["revision"]
                && p["implementationAttempts"] == json!(attempts)
                && p.get("choreography") == choreography_binding.as_ref()
        }) {
            return Ok(previous.clone());
        }
        if refreshed.is_some_and(|id| previous.is_none_or(|p| p["id"] != id)) {
            return Err(conflict(
                "A refreshed candidate must follow the candidate it was rebased from.",
            ));
        }
        if let Some(f) = feedback {
            request(f, &["continuationId", "sourceCandidateId"])?;
            uuid_field(f, "continuationId")?;
            uuid_field(f, "sourceCandidateId")?;
            if refreshed.is_some()
                || previous.is_none_or(|p| {
                    p["id"] != f["sourceCandidateId"]
                        || p["revision"]["head"] == current["revision"]["head"]
                })
            {
                return Err(conflict(
                    "A feedback candidate requires a changed head from its recorded source candidate.",
                ));
            }
        }
        let mut candidate = json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run_id,"revision":current["revision"],"implementationAttempts":attempts,"createdAt":time()});
        if let Some(binding) = choreography_binding {
            candidate["choreography"] = binding;
        }
        if let Some(id) = refreshed {
            candidate["refreshedFrom"] = json!(id);
        }
        if let Some(f) = feedback {
            candidate["feedbackFrom"] = f.clone();
        }
        record["candidates"]
            .as_array_mut()
            .ok_or_else(|| conflict("Candidate ledger unavailable."))?
            .push(candidate.clone());
        Ok(candidate)
    }
    pub async fn register_candidate(
        &self,
        run_id: &str,
        attempts: &[String],
        refreshed: Option<&str>,
        feedback: Option<&Value>,
    ) -> Result<Value> {
        let lock = self.work.lock(run_id);
        let _guard = lock.lock().await;
        self.verify_package_artifacts(run_id).await?;
        let current = self.work.status(run_id).await?;
        self.work.store(|s| {
            let mut candidate = Value::Null;
            s.update_coding_record(run_id, |previous| {
                let mut record =
                    previous.ok_or_else(|| conflict("Coding evidence is not configured."))?;
                candidate = Self::candidate(
                    s,
                    run_id,
                    &mut record,
                    attempts,
                    &current,
                    refreshed,
                    feedback,
                )?;
                Ok(record)
            })?;
            Ok(candidate)
        })
    }
    pub async fn complete(&self, run_id: &str, id: &str) -> Result<Value> {
        let lock = self.work.lock(run_id);
        let _guard = lock.lock().await;
        let record = self.work.store(|s| Self::record(s, run_id))?;
        let completion = array(&record["completions"])
            .iter()
            .find(|c| c["id"] == id)
            .ok_or_else(|| Error::NotFound("Completion is unavailable.".into()))?
            .clone();
        if completion["state"] == "completed" {
            if completion["mode"] == "checkpoint" {
                return Ok(completion["checkpointRevision"].clone());
            }
            let candidate = array(&record["candidates"])
                .iter()
                .find(|c| c["id"] == completion["candidateId"])
                .cloned()
                .ok_or_else(|| conflict("Completed candidate is unavailable."))?;
            self.cleanup_completed_conflict(run_id, &completion).await;
            return Ok(candidate);
        }
        if completion["state"] != "requested" {
            return Err(conflict(
                "Completion was rejected; this completion cannot be replayed.",
            ));
        }
        if completion["mode"] != "checkpoint" {
            self.verify_package_artifacts(run_id).await?;
        }
        self.work.commit_unlocked(run_id, &completion).await?;
        let current = self.work.status(run_id).await?;
        if completion["mode"] == "checkpoint" {
            return self.work.store(|s| {
                s.transaction(|s| {
                    Self::current(s, run_id, &current)?;
                    s.update_coding_record(run_id, |old| {
                        let mut record =
                            old.ok_or_else(|| conflict("Coding evidence disappeared."))?;
                        let saved = record["completions"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|c| c["id"] == id)
                            .ok_or_else(|| conflict("Checkpoint intent disappeared."))?;
                        if *saved != completion {
                            return Err(conflict(
                                "Checkpoint intent changed before reconciliation.",
                            ));
                        }
                        saved["state"] = json!("completed");
                        saved["checkpointRevision"] = current["revision"].clone();
                        Ok(record)
                    })?;
                    Ok(current["revision"].clone())
                })
            });
        }
        if completion.get("conflictRefreshId").is_some() {
            let candidate = self.complete_conflict_record(run_id, &completion, &current)?;
            self.cleanup_completed_conflict(run_id, &completion).await;
            return Ok(candidate);
        }
        self.work.store(|s| {
            s.transaction(|s| {
                let mut result = Value::Null;
                let mut feedback_receipt: Option<(String, Value)> = None;
                s.update_coding_record(run_id, |previous| {
                    let mut record =
                        previous.ok_or_else(|| conflict("Coding evidence disappeared."))?;
                    if !array(&record["completions"]).contains(&completion) {
                        return Err(conflict(
                            "Completion changed before candidate registration.",
                        ));
                    }
                    let claim = array(&record["claims"]).iter().rev().find(|c| {
                        c["writerAttemptId"] == completion["attemptId"]
                            && c.get("feedbackContinuationId").is_some()
                    });
                    let run = s.get_run(run_id)?;
                    let feedback = claim.and_then(|c| {
                        array(&run["feedbackContinuations"])
                            .iter()
                            .find(|f| f["id"] == c["feedbackContinuationId"])
                    });
                    let source = feedback.and_then(|f| {
                        array(&record["candidates"])
                            .iter()
                            .find(|c| c["id"] == f["sourceCandidateId"])
                    });
                    let latest = array(&record["candidates"]).last();
                    if claim.is_some()
                        && (feedback.is_none()
                            || source.is_none()
                            || source.unwrap()["revision"]["head"]
                                != feedback.unwrap()["sourceHead"]
                            || latest.is_none_or(|p| {
                                p["id"] != source.unwrap()["id"]
                                    && p["feedbackFrom"]["continuationId"]
                                        != feedback.unwrap()["id"]
                            }))
                    {
                        return Err(conflict(
                            "The feedback source candidate changed before completion.",
                        ));
                    }
                    let mut authors = Vec::new();
                    if feedback.is_some() {
                        for a in array(&latest.unwrap()["implementationAttempts"]) {
                            authors.push(
                                a.as_str()
                                    .ok_or_else(|| conflict("Invalid author identity."))?
                                    .to_owned(),
                            );
                        }
                    } else {
                        for edit in array(&record["edits"])
                            .iter()
                            .filter(|e| e["state"] == "applied")
                        {
                            let author = text(edit, "attemptId")?.to_owned();
                            if !authors.contains(&author) {
                                authors.push(author);
                            }
                        }
                    }
                    let author = text(&completion, "attemptId")?.to_owned();
                    if !authors.contains(&author) {
                        authors.push(author);
                    }
                    let lineage = feedback.map(
                    |f| json!({"continuationId":f["id"],"sourceCandidateId":latest.unwrap()["id"]}),
                );
                    let unchanged = feedback.is_some()
                        && latest
                            .is_some_and(|p| p["revision"]["head"] == current["revision"]["head"]);
                    let original_source_id = source.map(|p| p["id"].clone());
                    result = if unchanged {
                        Self::current(s, run_id, &current)?;
                        latest.unwrap().clone()
                    } else {
                        Self::candidate(
                            s,
                            run_id,
                            &mut record,
                            &authors,
                            &current,
                            None,
                            lineage.as_ref(),
                        )?
                    };
                    if let Some(f) = feedback {
                        feedback_receipt = Some((
                            text(f, "id")?.to_owned(),
                            if unchanged && original_source_id.as_ref() == Some(&result["id"]) {
                                json!({"noChange":true})
                            } else {
                                json!({"candidateId":result["id"]})
                            },
                        ));
                    }
                    let saved = record["completions"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|c| c["id"] == id)
                        .unwrap();
                    saved["state"] = json!("completed");
                    saved["candidateId"] = result["id"].clone();
                    Ok(record)
                })?;
                if let Some((id, outcome)) = feedback_receipt {
                    s.finish_feedback_continuation(run_id, &id, &outcome)?;
                }
                Ok(result)
            })
        })
    }
    async fn verify_package_artifacts(&self, run_id: &str) -> Result<()> {
        let Some(state) = self.work.store(|s| s.choreography(run_id))? else {
            return Ok(());
        };
        let mut seen = HashSet::new();
        let mut paths = HashSet::new();
        for report in array(&state["reports"]).iter().rev().filter(|r| {
            array(&state["packages"])
                .iter()
                .any(|p| p["id"] == r["packageId"] && p["generation"] == r["generation"])
        }) {
            if !seen.insert((
                report["packageId"].clone().to_string(),
                report["assignmentId"].clone().to_string(),
            )) {
                continue;
            }
            for artifact in array(&report["artifacts"]) {
                // A later verified contribution supersedes the current obligation
                // for a shared path. Earlier source snapshots remain immutable.
                if !paths.insert(text(artifact, "path")?.to_owned()) {
                    continue;
                }
                let file = self.work.read_file(run_id, text(artifact, "path")?).await?;
                if file["sha256"] != artifact["sha256"] {
                    return Err(conflict(
                        "Package report artifacts changed before candidate freeze.",
                    ));
                }
            }
        }
        if self.work.store(|s| s.choreography(run_id))?.as_ref() != Some(&state) {
            return Err(conflict(
                "Package reports changed during artifact verification.",
            ));
        }
        Ok(())
    }
    async fn cleanup_completed_conflict(&self, id: &str, completion: &Value) {
        if let Some(key) = completion["conflictRefreshId"].as_str()
            && self.work.cleanup_promoted_unlocked(id, key).await.is_err()
        {
            eprintln!(
                "Promoted conflict workspace retained; cleanup needs inspection for run {id}."
            );
        }
    }
    fn complete_conflict_record(
        &self,
        id: &str,
        completion: &Value,
        current: &Value,
    ) -> Result<Value> {
        self.work.store(|s| {
            s.transaction(|s| {
                Self::current(s, id, current)?;
                let r = array(&current["refreshes"])
                    .iter()
                    .find(|r| {
                        r["requestId"] == completion["conflictRefreshId"]
                            && r["state"] == "completed"
                            && r["resolutionWorkspace"]["operationId"] == completion["id"]
                    })
                    .ok_or_else(|| conflict("The target conflict was not promoted."))?;
                if current["revision"]["head"] != r["toHead"]
                    || current["revision"]["base"] != r["toBase"]
                    || current["revision"]["worktreeHash"] != completion["worktreeHash"]
                {
                    return Err(conflict("Promoted source changed before candidate association."));
                }
                let mut result = Value::Null;
                s.update_coding_record(id, |old| {
                    let mut record = old.ok_or_else(|| conflict("Coding evidence disappeared."))?;
                    if !array(&record["completions"]).contains(completion)
                        || completion["state"] != "requested"
                    {
                        return Err(conflict("Conflict completion changed before candidate registration."));
                    }
                    let source = array(&record["candidates"])
                        .last()
                        .filter(|c| {
                            c["id"] == r["sourceCandidateId"]
                                && c["revision"]["head"] == r["fromHead"]
                                && c["revision"]["base"] == r["fromBase"]
                        })
                        .ok_or_else(|| conflict("The target source candidate is missing or ambiguous; operator recovery is required."))?;
                    let source_id = text(source, "id")?.to_owned();
                    let mut authors = serde_json::from_value::<Vec<String>>(source["implementationAttempts"].clone())?;
                    for edit in array(&record["edits"]).iter().filter(|e| e["state"] == "applied") {
                        let author = text(edit, "attemptId")?;
                        if array(&record["claims"]).iter().any(|c| {
                            c["writerAttemptId"] == author && c["targetRefreshId"] == r["requestId"]
                        }) && !authors.iter().any(|a| a == author) {
                            authors.push(author.into());
                        }
                    }
                    let author = text(completion, "attemptId")?.to_owned();
                    if !authors.contains(&author) {
                        authors.push(author);
                    }
                    result = Self::candidate(s, id, &mut record, &authors, current, Some(&source_id), None)?;
                    let saved = record["completions"]
                        .as_array_mut().unwrap().iter_mut()
                        .find(|c| c["id"] == completion["id"]).unwrap();
                    saved["state"] = json!("completed");
                    saved["candidateId"] = result["id"].clone();
                    Ok(record)
                })?;
                s.finish_target_refresh(id, text(r, "requestId")?, &json!({"candidateId":result["id"]}), None)?;
                s.notice_delivery(id, &json!({
                    "kind":"target_refresh", "subjectId":r["requestId"],
                    "fromBase":r["fromBase"], "toBase":r["toBase"],
                    "candidateId":result["id"], "head":result["revision"]["head"],
                    "instruction":format!("The target conflict was resolved on the owned branch as candidate {}. Run its required checks and independent review before publishing it.",text(&result,"id")?)
                }))?;
                Ok(result)
            })
        })
    }
}
