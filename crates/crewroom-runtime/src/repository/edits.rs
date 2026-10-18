use super::*;
impl<S: StoreAccess> Repository<S> {
    pub(super) fn writer(s: &Store, attempt_id: &str) -> Result<Value> {
        let attempt = s.get_attempt(attempt_id)?;
        let run_id = text(&attempt, "runId")?;
        let work = s
            .repository_work(run_id)?
            .ok_or_else(|| conflict("Only the current writer may edit repository files."))?;
        let run = s.get_run(run_id)?;
        if work["writer"]["attemptId"] != attempt_id
            || attempt["ownership"] != "held"
            || !["idle", "executing"].contains(&attempt["state"].as_str().unwrap_or(""))
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || !array(&s.runtime(run_id)?["assignments"])
                .iter()
                .any(|a| a["currentAttemptId"] == attempt_id && a.get("retiredAt").is_none())
        {
            return Err(conflict(
                "Only the current writer may edit repository files.",
            ));
        }
        s.assert_choreography_writer(attempt_id, None)?;
        Ok(work)
    }
    fn edit_input(input: &Value) -> Result<Value> {
        request(input, &["path", "expectedHash", "content"])?;
        let path = limited(input, "path", 1, 1000)?;
        let content = limited(input, "content", 0, 1024 * 1024)?;
        if !input
            .get("expectedHash")
            .is_some_and(|v| v.is_null() || v.as_str().is_some_and(Self::hash_valid))
        {
            return Err(Error::Invalid("Invalid expectedHash.".into()));
        }
        Ok(json!({"path":path,"expectedHash":input["expectedHash"],"content":content}))
    }
    fn hash_valid(v: &str) -> bool {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }
    pub(super) fn tool_key(key: &str) -> Result<()> {
        if key.is_empty() || key.chars().count() > 200 {
            return Err(Error::Invalid("Invalid tool call identity.".into()));
        }
        Ok(())
    }
    pub async fn claim_writer(&self, attempt_id: &str, claim: Option<&Value>) -> Result<Value> {
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let run_id = text(&attempt, "runId")?;
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        self.claim_writer_unlocked(attempt_id, claim, |_| Ok(()))
            .await
    }
    pub(super) async fn claim_writer_unlocked(
        &self,
        attempt_id: &str,
        claim: Option<&Value>,
        recheck: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<Value> {
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let run_id = text(&attempt, "runId")?;
        if self.unresolved_conflict(run_id)?.is_some() && self.active_conflict(run_id)?.is_none() {
            return Err(conflict(
                "Target conflict requires an operator decision; repository writes are held.",
            ));
        }
        let work = self.status(run_id).await?;
        if work["drift"]
            .as_object()
            .is_none_or(|d| d.iter().any(|(k, v)| k != "base" && v == true))
            || !self
                .validated_conflict_base_drift(run_id, &work, None)
                .await?
        {
            return Err(conflict(
                "Repository or requirements changed. Reconcile drift before writing.",
            ));
        }
        let checkpoint = self
            .checkpoint(&work, "writer_started", Some(attempt_id))
            .await?;
        let latest = self.status(run_id).await?;
        if latest["drift"]
            .as_object()
            .is_none_or(|d| d.iter().any(|(k, v)| k != "base" && v == true))
            || !self
                .validated_conflict_base_drift(run_id, &latest, None)
                .await?
            || latest["revision"] != checkpoint["revision"]
        {
            return Err(conflict(
                "Repository or requirements changed. Reconcile drift before writing.",
            ));
        }
        let scope = if self.active_conflict(run_id)?.is_some() {
            Some(self.conflict_work(run_id, None, false).await?)
        } else {
            None
        };
        self.store(|s| {
            recheck(s)?;
            if let Some(scope) = &scope {
                Self::conflict_unchanged(s, scope)?;
                self.metadata(&scope.work)?;
            }
            self.metadata(&work)?;
            if Self::current_requirements(s, &work)? != checkpoint["revision"]["requirementsHash"] {
                return Err(conflict("Requirements changed before writer admission."));
            }
            s.claim_repository_writer(attempt_id, &checkpoint, claim)
        })
    }
    pub async fn release_writer(&self, attempt_id: &str) -> Result<Value> {
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let run_id = text(&attempt, "runId")?;
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        let work = self.owned(run_id)?;
        let checkpoint = self
            .checkpoint(&work, "interrupted", Some(attempt_id))
            .await?;
        self.store(|s| s.release_repository_writer(attempt_id, &checkpoint))
    }
    pub async fn reconcile_stopped_writer(&self, run_id: &str) -> Result<Value> {
        let work = self.owned(run_id)?;
        if let Some(id) = work["writer"]["attemptId"].as_str() {
            self.release_writer(id).await
        } else {
            Ok(work)
        }
    }
    async fn writable_source(
        &self,
        attempt_id: &str,
        name: &str,
    ) -> Result<(Value, Option<target::resolution::ConflictSource>)> {
        let owner = self.store(|s| {
            s.assert_package_edit(attempt_id)?;
            Self::writer(s, attempt_id)
        })?;
        let run_id = text(&owner, "runId")?;
        let scope = if self.active_conflict(run_id)?.is_some() {
            Some(self.conflict_work(run_id, None, false).await?)
        } else {
            self.no_conflict(run_id)?;
            None
        };
        let current = self.status(run_id).await?;
        if ["requirements", "instructions", "head"]
            .iter()
            .any(|key| current["drift"][key] == true)
            || !self
                .validated_conflict_base_drift(run_id, &current, None)
                .await?
        {
            return Err(conflict(
                "Repository base, head, requirements or instructions changed.",
            ));
        }
        let work = scope.as_ref().map(|s| &s.work).unwrap_or(&owner);
        files::scoped(self.path(work)?, name)?;
        if let Some(scope) = &scope {
            if scope.refresh["resolutionWorkspace"]["state"] != "conflicted" {
                return Err(conflict(
                    "Conflict rebase is ready; submit completion instead of editing again.",
                ));
            }
            if !array(&scope.refresh["conflicts"]).iter().any(|p| p == name) {
                return Err(conflict(
                    "Only recorded conflict files are in the resolution scope.",
                ));
            }
            if ["AGENTS.md", "CLAUDE.md"].contains(
                &Path::new(name)
                    .file_name()
                    .and_then(|p| p.to_str())
                    .unwrap_or(""),
            ) {
                return Err(conflict(
                    "Project instruction conflicts require operator review.",
                ));
            }
        }
        Ok((work.clone(), scope))
    }
    async fn write(&self, attempt_id: &str, input: &Value) -> Result<Value> {
        let input = Self::edit_input(input)?;
        let name = text(&input, "path")?;
        let (work, scope) = self.writable_source(attempt_id, name).await?;
        let run_id = text(&work, "runId")?;
        match self
            .inner
            .git
            .raw(
                self.path(&work)?,
                &["check-ignore", "--quiet", "--no-index", "--", name],
            )
            .await
        {
            Ok(_) => {
                return Err(conflict(
                    "Edits to ignored files are outside the recorded source scope.",
                ));
            }
            Err(e) if e.code == Some(1) => (),
            Err(e) => return Err(e.into()),
        }
        let latest = if let Some(scope) = &scope {
            let latest = self
                .conflict_work(run_id, Some(text(&scope.refresh, "requestId")?), false)
                .await?;
            if latest.original != scope.original
                || latest.refresh != scope.refresh
                || latest.work != work
            {
                return Err(conflict("Conflict workspace changed."));
            }
            Some(latest)
        } else {
            self.no_conflict(run_id)?;
            None
        };
        self.store(|s| {
            s.assert_package_edit(attempt_id)?;
            let current = Self::writer(s, attempt_id)?;
            let owner = scope.as_ref().map(|s| &s.original).unwrap_or(&work);
            if current != *owner
                || Self::current_requirements(s, owner)? != text(owner, "requirementsHash")?
            {
                return Err(conflict(
                    "Repository base, head, requirements or instructions changed.",
                ));
            }
            if let Some(scope) = &latest {
                Self::conflict_unchanged(s, scope)?;
            }
            self.metadata(&current)?;
            self.metadata(&work)?;
            let digest = files::replace(
                self.path(&work)?,
                &self.inner.root,
                name,
                input["expectedHash"].as_str(),
                text(&input, "content")?,
            )?;
            Ok(json!({"path":name,"sha256":digest}))
        })
    }
    pub async fn write_file(&self, attempt_id: &str, input: &Value) -> Result<Value> {
        let run = self.store(|s| s.get_attempt(attempt_id))?;
        let lock = self.lock(text(&run, "runId")?);
        let _guard = lock.lock().await;
        self.write(attempt_id, input).await
    }
    async fn edit_unlocked(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
        request_hash: Option<&str>,
    ) -> Result<Value> {
        Self::tool_key(key)?;
        let input = Self::edit_input(input)?;
        let (run_id,edit)=self.store(|s|{
            s.assert_package_edit(attempt_id)?;let owned=Self::writer(s,attempt_id)?;let run_id=text(&owned,"runId")?.to_owned();let record=s.coding_record(&run_id)?.ok_or_else(||conflict("Configure coding evidence before editing."))?;
            if array(&record["completions"]).iter().any(|c|c["state"]=="requested" && c["attemptId"]==attempt_id) {return Err(conflict("Completion is pending; further edits are closed."));}
            if let Some(edit)=array(&record["edits"]).iter().find(|e|e["attemptId"]==attempt_id && e["toolCallId"]==key) {
                if stringify(&edit["input"])!=stringify(&input) || edit["request"].as_str()!=request_hash {return Err(conflict("Edit operation has different content."));}return Ok((run_id,edit.clone()));
            }
            let mut edit=json!({"id":uuid::Uuid::new_v4().to_string(),"attemptId":attempt_id,"toolCallId":key,"input":input,"afterHash":hash(text(&input,"content")?),"state":"pending"});if let Some(r)=request_hash{edit["request"]=json!(r);}
            s.update_coding_record(&run_id,|previous|{let mut record=previous.ok_or_else(||conflict("Coding record disappeared."))?;let mut edits=array(&record["edits"]).to_vec();edits.push(edit.clone());record["edits"]=json!(edits);Ok(record)})?;Ok((run_id,edit))
        })?;
        if edit["state"] == "rejected" {
            return Err(conflict(
                edit["reason"].as_str().unwrap_or("Edit was rejected."),
            ));
        }
        if edit["state"] != "applied" {
            let result: Result<()> = async {
                self.writable_source(attempt_id, text(&input, "path")?)
                    .await?;
                let current = match self.read_file(&run_id, text(&input, "path")?).await {
                    Ok(v) => v["sha256"].as_str().map(String::from),
                    Err(Error::NotFound(_)) => None,
                    Err(e) => return Err(e),
                };
                if current.as_deref() != edit["afterHash"].as_str() {
                    self.write(attempt_id, &input).await?;
                }
                self.set_edit_state(&run_id, text(&edit, "id")?, "applied", None)?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                if matches!(error, Error::Conflict(_) | Error::NotFound(_)) {
                    self.set_edit_state(
                        &run_id,
                        text(&edit, "id")?,
                        "rejected",
                        Some(&error.to_string()),
                    )?;
                }
                return Err(error);
            }
        }
        Ok(json!({"operationId":edit["id"],"path":input["path"],"sha256":edit["afterHash"]}))
    }
    fn set_edit_state(
        &self,
        run_id: &str,
        id: &str,
        state: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        self.store(|s| {
            s.update_coding_record(run_id, |previous| {
                let mut record = previous.ok_or_else(|| conflict("Coding record disappeared."))?;
                for item in record["edits"]
                    .as_array_mut()
                    .ok_or_else(|| conflict("Edit ledger disappeared."))?
                {
                    if item["id"] == id {
                        item["state"] = json!(state);
                        if let Some(reason) = reason {
                            item["reason"] = json!(reason);
                        }
                    }
                }
                Ok(record)
            })?;
            Ok(())
        })
    }
    pub async fn edit(&self, attempt_id: &str, key: &str, input: &Value) -> Result<Value> {
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let lock = self.lock(text(&attempt, "runId")?);
        let _guard = lock.lock().await;
        self.edit_unlocked(attempt_id, key, input, None).await
    }
    pub async fn patch(&self, attempt_id: &str, key: &str, input: &Value) -> Result<Value> {
        request(input, &["path", "expectedHash", "replacements"])?;
        let path = limited(input, "path", 1, 1000)?;
        let expected = text(input, "expectedHash")?;
        if !Self::hash_valid(expected) {
            return Err(Error::Invalid("Invalid expectedHash.".into()));
        }
        Self::tool_key(key)?;
        let replacements = input["replacements"]
            .as_array()
            .filter(|v| (1..=20).contains(&v.len()))
            .ok_or_else(|| Error::Invalid("Provide one to twenty exact replacements.".into()))?;
        let normalized=replacements.iter().map(|r|{request(r,&["oldText","newText"])?;Ok(json!({"oldText":limited(r,"oldText",1,100000)?,"newText":limited(r,"newText",0,100000)?}))}).collect::<Result<Vec<_>>>()?;
        let request_hash =
            json_hash(&json!({"path":path,"expectedHash":expected,"replacements":normalized}));
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let run_id = text(&attempt, "runId")?;
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        let previous = self.store(|s| {
            Ok(s.coding_record(run_id)?.and_then(|r| {
                array(&r["edits"])
                    .iter()
                    .find(|e| e["attemptId"] == attempt_id && e["toolCallId"] == key)
                    .cloned()
            }))
        })?;
        if let Some(previous) = previous {
            if previous["request"] != request_hash {
                return Err(conflict("Edit operation has different content."));
            }
            return self
                .edit_unlocked(attempt_id, key, &previous["input"], Some(&request_hash))
                .await;
        }
        let current = self.read_file(run_id, path).await?;
        if current["sha256"] != expected {
            return Err(conflict("File changed since it was read."));
        }
        let mut content = text(&current, "content")?.to_owned();
        for (index, r) in normalized.iter().enumerate() {
            let old = text(r, "oldText")?;
            let new = text(r, "newText")?;
            let first = content.find(old);
            // Look for overlapping occurrences too; JavaScript indexOf advances a
            // code unit, and the needle begins on a complete scalar here.
            let repeated = first.is_some_and(|n| {
                content[n + content[n..].chars().next().unwrap().len_utf8()..].contains(old)
            });
            if first.is_none() || repeated {
                return Err(conflict(&format!(
                    "Replacement {} must match exactly one span of the current text; it matched {}.",
                    index + 1,
                    if repeated { "more than one" } else { "none" }
                )));
            }
            let start = first.unwrap();
            content.replace_range(start..start + old.len(), new);
        }
        self.edit_unlocked(
            attempt_id,
            key,
            &json!({"path":path,"expectedHash":expected,"content":content}),
            Some(&request_hash),
        )
        .await
    }
    pub async fn reconcile_stopped_edits(&self, run_id: &str) -> Result<()> {
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        let edits = self.store(|s| {
            let record = s.coding_record(run_id)?.unwrap_or(Value::Null);
            let edits = array(&record["edits"])
                .iter()
                .filter(|e| e["state"] == "pending")
                .cloned()
                .collect::<Vec<_>>();
            for edit in &edits {
                if s.get_attempt(text(edit, "attemptId")?)?["ownership"] != "stopped" {
                    return Err(conflict(
                        "Confirm stopped writer ownership before reconciling an edit.",
                    ));
                }
            }
            Ok(edits)
        })?;
        for edit in edits {
            let current = match self.read_file(run_id, text(&edit["input"], "path")?).await {
                Ok(v) => v["sha256"].as_str().map(String::from),
                Err(Error::NotFound(_)) => None,
                Err(e) => return Err(e),
            };
            if current.as_deref() == edit["afterHash"].as_str() {
                self.set_edit_state(run_id, text(&edit, "id")?, "applied", None)?;
            } else {
                self.set_edit_state(run_id,text(&edit,"id")?,"rejected",Some("Interrupted edit was not confirmed in the retained content. Read the current file before retrying."))?;
            }
        }
        Ok(())
    }
}
