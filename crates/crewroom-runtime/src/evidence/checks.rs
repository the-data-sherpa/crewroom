use super::*;
use crate::container_command::ContainerCommand;
#[derive(Clone)]
pub struct Checks<S: StoreAccess> {
    evidence: Evidence<S>,
    root: PathBuf,
    socket: PathBuf,
}
impl<S: StoreAccess> Checks<S> {
    pub fn new(evidence: Evidence<S>, root: PathBuf, socket: PathBuf) -> Result<Self> {
        crate::create_root(&root)?;
        ContainerCommand::new(root.clone(), &socket)?;
        Ok(Self {
            evidence,
            root,
            socket,
        })
    }
    pub fn job_root(&self, attempt_id: &str, key: &str) -> Result<PathBuf> {
        uuid_field(&json!({"attempt":attempt_id}), "attempt")?;
        limited(&json!({"key":key}), "key", 1, 200)?;
        Ok(self.root.join(attempt_id).join("tools").join(hash(key)))
    }
    fn job(&self, attempt_id: &str, key: &str) -> Result<ContainerCommand> {
        ContainerCommand::new(self.job_root(attempt_id, key)?, &self.socket)
    }
    async fn cleanup(job: &ContainerCommand) -> Result<()> {
        job.remove().await?;
        let state = job.inspect().await;
        if state["state"] != "stopped" || state["id"].as_str().is_none() {
            return Err(conflict(
                "Check evidence is saved, but container cleanup remains unconfirmed.",
            ));
        }
        let journal: Value = serde_json::from_slice(
            &crate::repository::files::regular_bytes(
                &job.root,
                "container.json",
                64 * 1024 * 1024,
            )?
            .0,
        )?;
        if journal["removed"] != true {
            return Err(conflict(
                "Check evidence is saved, but container cleanup remains unconfirmed.",
            ));
        }
        Ok(())
    }
    fn authorize(s: &Store, attempt_id: &str) -> Result<String> {
        let attempt = s.get_attempt(attempt_id)?;
        let id = text(&attempt, "runId")?;
        let run = s.get_run(id)?;
        if attempt["ownership"] != "held"
            || !["idle", "executing"].contains(&attempt["state"].as_str().unwrap_or(""))
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || !array(&s.runtime(id)?["assignments"])
                .iter()
                .any(|a| a["currentAttemptId"] == attempt_id && a.get("retiredAt").is_none())
        {
            return Err(conflict("Only a current attempt may execute a check."));
        }
        Ok(id.into())
    }
    fn spec(operation: &Value) -> Value {
        let d = &operation["definition"];
        let mut spec = json!({"image":d["image"],"command":d["command"],"args":d["args"],"timeoutMs":d["timeoutMs"],"workspace":operation["workspace"]});
        for key in ["memoryMiB", "tmpExecutable"] {
            if let Some(v) = d.get(key) {
                spec[key] = v.clone();
            }
        }
        spec
    }
    fn selected(s: &Store, run_id: &str, operation: &Value) -> Result<Value> {
        let record = Evidence::<S>::record(s, run_id)?;
        if !array(&record["checkOperations"])
            .iter()
            .any(|o| o == operation)
            || !array(&record["requiredChecks"])
                .iter()
                .any(|d| *d == operation["definition"])
            || array(&record["candidates"])
                .last()
                .is_none_or(|c| c["id"] != operation["candidateId"])
        {
            return Err(conflict(
                "Check candidate, policy or operation changed during preparation.",
            ));
        }
        Ok(array(&record["candidates"]).last().unwrap().clone())
    }
    pub async fn prepare(&self, attempt_id: &str, key: &str, input: &Value) -> Result<Value> {
        request(input, &["checkId"])?;
        let check_id = text(input, "checkId")?;
        let root = self.job_root(attempt_id, key)?;
        let run_id = self
            .evidence
            .work
            .store(|s| Self::authorize(s, attempt_id))?;
        let lock = self.evidence.work.lock(&run_id);
        let _guard = lock.lock().await;
        self.evidence.work.no_conflict(&run_id)?;
        let operation=self.evidence.work.store(|s|{
            Self::authorize(s,attempt_id)?;let mut selected=Value::Null;
            s.update_coding_record(&run_id,|previous|{let mut record=previous.ok_or_else(||conflict("Check or candidate is unavailable."))?;
                let candidate=array(&record["candidates"]).last().ok_or_else(||conflict("Check or candidate is unavailable."))?;
                let definition=array(&record["requiredChecks"]).iter().find(|d|d["id"]==check_id).ok_or_else(||conflict("Check or candidate is unavailable."))?;
                if let Some(prior)=array(&record["checkOperations"]).iter().find(|o|o["attemptId"]==attempt_id && o["toolCallId"]==key){if prior["definition"]["id"]!=check_id{return Err(conflict("Check operation has different content."));}selected=prior.clone();return Ok(record);}
                selected=json!({"id":uuid::Uuid::new_v4().to_string(),"attemptId":attempt_id,"toolCallId":key,"candidateId":candidate["id"],"definition":definition,"workspace":"","state":"preparing"});
                let mut ops=array(&record["checkOperations"]).to_vec();ops.push(selected.clone());record["checkOperations"]=json!(ops);Ok(record)
            })?;Ok(selected)
        })?;
        if operation["state"] == "rejected" {
            return Err(conflict(
                operation["reason"]
                    .as_str()
                    .unwrap_or("Check preparation was rejected."),
            ));
        }
        let mut operation = operation;
        if operation["state"] == "preparing" {
            let result: Result<Value> = async {
                let selected = self
                    .evidence
                    .work
                    .store(|s| Self::selected(s, &run_id, &operation))?;
                let destination = root.join(format!("source-{}", uuid::Uuid::new_v4()));
                let workspace = self
                    .evidence
                    .work
                    .export_unlocked(&run_id, &selected["revision"], &destination)
                    .await?;
                let manifest = stringify(&self.evidence.work.list_files(&run_id).await?);
                if hash(&manifest) != selected["revision"]["worktreeHash"] {
                    return Err(conflict(
                        "Source manifest changed during check preparation.",
                    ));
                }
                let source = self.evidence.artifact(&manifest)?;
                let current = self.evidence.work.status(&run_id).await?;
                self.evidence.work.store(|s| {
                    Self::authorize(s, attempt_id)?;
                    Self::selected(s, &run_id, &operation)?;
                    Evidence::<S>::current(s, &run_id, &current)?;
                    if current.get("writer").is_some()
                        || current["revision"] != selected["revision"]
                    {
                        return Err(conflict(
                            "The candidate revision changed during check preparation.",
                        ));
                    }
                    let mut saved = operation.clone();
                    saved["workspace"] = json!(workspace);
                    saved["source"] = source;
                    saved["state"] = json!("prepared");
                    s.update_coding_record(&run_id, |previous| {
                        let mut record = previous.unwrap();
                        *record["checkOperations"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|o| o["id"] == saved["id"])
                            .unwrap() = saved.clone();
                        Ok(record)
                    })?;
                    Ok(saved)
                })
            }
            .await;
            match result {
                Ok(saved) => operation = saved,
                Err(e) => {
                    if matches!(e, Error::Conflict(_) | Error::NotFound(_)) {
                        self.evidence.work.store(|s| {
                            s.update_coding_record(&run_id, |previous| {
                                let mut record = previous.unwrap();
                                if let Some(o) = record["checkOperations"]
                                    .as_array_mut()
                                    .unwrap()
                                    .iter_mut()
                                    .find(|o| **o == operation)
                                {
                                    o["state"] = json!("rejected");
                                    o["reason"] = json!(e.to_string());
                                }
                                Ok(record)
                            })?;
                            Ok(())
                        })?;
                    }
                    return Err(e);
                }
            }
        }
        // A prepared replay can only launch against its original current policy.
        // Recorded receipts are returned by finish and never launch a second job.
        if operation["state"] == "prepared" {
            self.evidence.work.store(|s| {
                Self::authorize(s, attempt_id)?;
                Self::selected(s, &run_id, &operation)?;
                Ok(())
            })?;
        }
        Ok(
            json!({"operationId":operation["id"],"candidateId":operation["candidateId"],"spec":Self::spec(&operation)}),
        )
    }
    pub async fn finish(&self, attempt_id: &str, key: &str) -> Result<Value> {
        let run_id = self
            .evidence
            .work
            .store(|s| Ok(text(&s.get_attempt(attempt_id)?, "runId")?.to_owned()))?;
        let lock = self.evidence.work.lock(&run_id);
        let _guard = lock.lock().await;
        let record = self
            .evidence
            .work
            .store(|s| Evidence::<S>::record(s, &run_id))?;
        let op = array(&record["checkOperations"])
            .iter()
            .find(|o| o["attemptId"] == attempt_id && o["toolCallId"] == key)
            .filter(|o| ["prepared", "recorded"].contains(&o["state"].as_str().unwrap_or("")))
            .ok_or_else(|| Error::NotFound("Prepared check execution is unavailable.".into()))?;
        let job = self.job(attempt_id, key)?;
        if op["state"] == "recorded" {
            let receipt = array(&record["checks"])
                .iter()
                .find(|c| c["id"] == op["evidenceId"])
                .ok_or_else(|| conflict("Check evidence receipt is unavailable."))?
                .clone();
            Self::cleanup(&job).await?;
            return Ok(receipt);
        }
        let actual = job.receipt().await?;
        let expected = Self::spec(op);
        if ["workspace", "image", "command", "args", "timeoutMs"]
            .iter()
            .any(|k| actual["spec"][k] != expected[k])
            || actual["spec"]["memoryMiB"].as_u64().unwrap_or(1024)
                != expected["memoryMiB"].as_u64().unwrap_or(1024)
            || actual["spec"]["tmpExecutable"].as_bool().unwrap_or(false)
                != expected["tmpExecutable"].as_bool().unwrap_or(false)
            || actual["spec"]["readOnlyWorkspace"] == true
        {
            return Err(conflict(
                "Command receipt differs from the prepared execution.",
            ));
        }
        let source = op
            .get("source")
            .ok_or_else(|| conflict("Check source manifest is unavailable."))?
            .clone();
        let artifacts = self.evidence.artifacts.clone();
        let snapshot = PathBuf::from(text(op, "workspace")?);
        let manifest = source.clone();
        let intact = tokio::task::spawn_blocking(move || -> bool {
            artifacts
                .read(&manifest)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .and_then(|v| Repository::<S>::verify_export(&snapshot, &v).ok())
                .unwrap_or(false)
        })
        .await
        .map_err(|_| conflict("Snapshot verification interrupted."))?;
        let output = self.evidence.artifact(text(&actual, "output")?)?;
        let d = &op["definition"];
        let mut environment = json!({"image":actual["spec"]["image"],"network":"none","platform":actual["platform"],"memoryMiB":actual["spec"]["memoryMiB"].as_u64().unwrap_or(1024),"tmpExecutable":actual["spec"]["tmpExecutable"].as_bool().unwrap_or(false)});
        if let Some(user) = actual.get("user") {
            environment["user"] = user.clone();
        }
        let receipt=self.evidence.record_check(&run_id,&json!({"candidateId":op["candidateId"],"attemptId":attempt_id,"checkId":d["id"],"definition":d,"environment":environment,"started":actual["started"],"completed":actual["completed"],"exitCode":actual["exitCode"],"timedOut":actual["timedOut"],"source":source,"sourceIntact":intact,"output":output}),op)?;
        Self::cleanup(&job).await?;
        Ok(receipt)
    }
    pub async fn execute(&self, attempt_id: &str, key: &str, input: &Value) -> Result<Value> {
        let prepared = self.prepare(attempt_id, key, input).await?;
        let job = self.job(attempt_id, key)?;
        if !crate::container_command::present(
            &self.job_root(attempt_id, key)?.join("container.json"),
        )? {
            job.start(prepared["spec"].clone()).await?;
        }
        job.wait().await?;
        self.finish(attempt_id, key).await
    }
    pub async fn reconcile_stopped(&self, run_id: &str) -> Result<()> {
        let record = self
            .evidence
            .work
            .store(|s| Evidence::<S>::record(s, run_id))?;
        for op in array(&record["checkOperations"]) {
            let attempt = text(op, "attemptId")?;
            let key = text(op, "toolCallId")?;
            if op["state"] == "recorded" {
                Self::cleanup(&self.job(attempt, key)?).await?;
                continue;
            }
            if op["state"] == "rejected"
                || self.evidence.work.store(|s| s.get_attempt(attempt))?["ownership"] != "stopped"
            {
                continue;
            }
            if op["state"] == "prepared"
                && crate::container_command::present(
                    &self.job_root(attempt, key)?.join("container.json"),
                )?
            {
                self.finish(attempt, key).await?;
            } else {
                self.evidence.work.store(|s| {
                    s.update_coding_record(run_id, |previous| {
                        let mut record = previous.unwrap();
                        let saved = record["checkOperations"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|o| o["id"] == op["id"])
                            .ok_or_else(|| conflict("Check operation disappeared."))?;
                        if saved != op {
                            return Err(conflict("Check operation changed during reconciliation."));
                        }
                        saved["state"] = json!("rejected");
                        saved["reason"] = json!(
                            "Interrupted before command execution; no check evidence was produced."
                        );
                        Ok(record)
                    })?;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }
}
impl<S: StoreAccess> Evidence<S> {
    // Deliberately not a public agent/HTTP operation. The controller derives
    // this value only from ContainerCommand's actual immutable receipt.
    fn record_check(&self, run_id: &str, value: &Value, operation: &Value) -> Result<Value> {
        self.read_artifact(&value["output"])?;
        self.read_artifact(&value["source"])?;
        self.work.store(|s|{
            let mut receipt=Value::Null;
            s.update_coding_record(run_id,|previous|{
                let mut record=previous.ok_or_else(||conflict("Coding evidence disappeared."))?;
                let attempt=s.get_attempt(text(value,"attemptId")?)?;
                let definition=array(&record["requiredChecks"]).iter().find(|d|d["id"]==value["checkId"]);
                if !array(&record["candidates"]).iter().any(|c|c["id"]==value["candidateId"]) || attempt["runId"]!=run_id || attempt["sessionId"].as_str().is_none_or(str::is_empty) || definition.is_none_or(|d|*d!=value["definition"] || d["image"]!=value["environment"]["image"] || d["memoryMiB"].as_u64().unwrap_or(1024)!=value["environment"]["memoryMiB"].as_u64().unwrap_or(1024) || d["tmpExecutable"].as_bool().unwrap_or(false)!=value["environment"]["tmpExecutable"].as_bool().unwrap_or(false)) || value["environment"]["network"]!="none" || value["environment"]["platform"].as_str().is_none_or(str::is_empty) || value["exitCode"].as_i64().is_none(){return Err(conflict("Executed check does not match its admitted candidate, environment or command."));}
                let saved=array(&record["checkOperations"]).iter().find(|o|o["id"]==operation["id"]);
                if saved!=Some(operation) || operation["state"]!="prepared" || operation["attemptId"]!=value["attemptId"] || operation["candidateId"]!=value["candidateId"] || operation["definition"]!=value["definition"] || operation["source"]!=value["source"] {return Err(conflict("Check receipt does not belong to its prepared operation."));}
                receipt=value.clone();receipt["id"]=operation["id"].clone();receipt["completedAt"]=json!(time());
                record["checks"].as_array_mut().unwrap().push(receipt.clone());let saved=record["checkOperations"].as_array_mut().unwrap().iter_mut().find(|o|o["id"]==operation["id"]).unwrap();saved["state"]=json!("recorded");saved["evidenceId"]=receipt["id"].clone();Ok(record)
            })?;Ok(receipt)
        })
    }
}
