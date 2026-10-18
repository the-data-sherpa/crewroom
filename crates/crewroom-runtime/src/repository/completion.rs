use super::*;
impl<S: StoreAccess> Repository<S> {
    pub(crate) fn committable(s: &Store, run_id: &str, completion: &Value) -> Result<()> {
        Self::completion_stopped(s, run_id, completion, false)
    }
    pub(crate) fn completion_stopped(
        s: &Store,
        run_id: &str,
        completion: &Value,
        recovery: bool,
    ) -> Result<()> {
        let run = s.get_run(run_id)?;
        let work = s.repository_work(run_id)?.unwrap_or(Value::Null);
        let record = s.coding_record(run_id)?.unwrap_or(Value::Null);
        if work.get("writer").is_some()
            || !(run["status"] == "paused" || recovery && run["status"] == "attention_required")
            || if recovery {
                run["desiredState"] == "cancelled"
            } else {
                run["desiredState"] != "running"
            }
            || array(&s.runtime(run_id)?["attempts"])
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
        {
            return Err(conflict(
                "Pause and confirm stopped ownership before committing.",
            ));
        }
        if !array(&record["completions"])
            .iter()
            .any(|c| c == completion && c["state"] == "requested")
        {
            return Err(conflict(
                "Implementation completion changed before Git dispatch.",
            ));
        }
        if Self::current_requirements(s, &work)? != text(&work, "requirementsHash")? {
            return Err(conflict(
                "Repository content or requirements changed after completion was requested.",
            ));
        }
        if completion.get("choreographyRevision").is_some() {
            let state = s
                .choreography(run_id)?
                .ok_or_else(|| conflict("Package choreography disappeared."))?;
            if state["revision"] != completion["choreographyRevision"]
                || state["binding"] != completion["choreographyBinding"]
            {
                return Err(conflict(
                    "Package obligations changed after completion was requested.",
                ));
            }
            if array(&s.runtime(run_id)?["messages"]).iter().any(|m| {
                !["received", "discarded"].contains(&m["disposition"].as_str().unwrap_or(""))
            }) {
                return Err(conflict(
                    "Reconcile external messages before checkpoint or freeze.",
                ));
            }
        }
        Ok(())
    }
    pub async fn request_completion(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
    ) -> Result<Value> {
        request(input, &["summary"])?;
        self.request_completion_kind(attempt_id, key, input, false)
            .await
    }
    pub async fn request_checkpoint(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
    ) -> Result<Value> {
        request(input, &["summary", "successor"])?;
        limited(input, "successor", 1, 61)?;
        self.request_completion_kind(attempt_id, key, input, true)
            .await
    }
    async fn request_completion_kind(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
        checkpoint: bool,
    ) -> Result<Value> {
        Self::tool_key(key)?;
        let summary = crewroom_core::store::trim_text(text(input, "summary")?);
        limited(&json!({"summary":summary}), "summary", 1, 2000)?;
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let run_id = text(&attempt, "runId")?;
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        let previous = |s: &Store| -> Result<Option<Value>> {
            let record = s
                .coding_record(run_id)?
                .ok_or_else(|| Error::NotFound("Coding evidence is not configured.".into()))?;
            let prior = array(&record["completions"])
                .iter()
                .find(|c| c["attemptId"] == attempt_id && c["toolCallId"] == key);
            if prior.is_some_and(|p| {
                p["summary"] != summary
                    || (p["mode"] == "checkpoint") != checkpoint
                    || p.get("successor") != input.get("successor")
            }) {
                return Err(conflict("Completion identity has different content."));
            }
            Ok(prior.cloned())
        };
        if let Some(prior) = self.store(previous)? {
            return Ok(prior);
        }
        let choreography = self.store(|s| {
            if checkpoint {
                let state = s
                    .choreography(run_id)?
                    .ok_or_else(|| conflict("Checkpoint handoff requires package choreography."))?;
                if !array(&state["schedule"]["assignments"])
                    .iter()
                    .any(|m| m["name"] == input["successor"] && m["writes"] == true)
                {
                    return Err(conflict(
                        "The successor must have a scheduled writing responsibility.",
                    ));
                }
                let assignment = s.get_assignment(text(&attempt, "assignmentId")?)?;
                if assignment["name"] == input["successor"] {
                    return Err(conflict(
                        "Checkpoint handoff requires a distinct successor.",
                    ));
                }
            } else {
                let blockers = [
                    s.choreography_blockers(run_id)?,
                    s.insertion_blockers(run_id)?,
                ]
                .concat();
                if !blockers.is_empty() {
                    return Err(conflict(&blockers.join(" ")));
                }
            }
            s.choreography(run_id)
        })?;
        let active = self.active_conflict(run_id)?;
        if checkpoint && active.is_some() {
            return Err(conflict(
                "Complete target-conflict recovery before a package handoff.",
            ));
        }
        if active
            .as_ref()
            .is_some_and(|r| r["resolutionWorkspace"]["state"] != "ready")
        {
            return Err(conflict(
                "Continue the isolated target rebase until it reports ready before submitting completion.",
            ));
        }
        if active.is_none() {
            self.no_conflict(run_id)?;
        }
        let status = self.status(run_id).await?;
        let scope = if let Some(r) = &active {
            Some(self.ready_conflict(run_id, text(r, "requestId")?).await?)
        } else {
            None
        };
        self.store(|s| {
            if let Some(prior) = previous(s)? { return Ok(prior); }
            let owner = Self::writer(s, attempt_id)?;
            if s.choreography(run_id)? != choreography { return Err(conflict("Package obligations changed during completion request.")); }
            if let Some((source,_)) = &scope {
                Self::conflict_unchanged(s,source)?;
                if source.refresh["resolutionWorkspace"]["state"] != "ready" {return Err(conflict("Conflict completion needs the current ready workspace."));}
            }
            if status["writer"]["attemptId"] != attempt_id || Self::current_requirements(s, &owner)? != status["revision"]["requirementsHash"] {
                return Err(conflict("Only the current writer may request completion."));
            }
            s.update_coding_record(run_id, |old| {
                let mut record = old.ok_or_else(|| conflict("Coding evidence is not configured."))?;
                if array(&record["edits"]).iter().any(|e| e["state"] == "pending") || array(&record["completions"]).iter().any(|c| c["state"] == "requested") {
                    return Err(conflict("Reconcile pending edits or completion first."));
                }
                let mut completion = json!({"id":uuid::Uuid::new_v4().to_string(),"attemptId":attempt_id,"toolCallId":key,"summary":summary,"head":status["revision"]["head"],"worktreeHash":status["revision"]["worktreeHash"],"state":"requested"});
                if let Some(state) = &choreography {completion["choreographyRevision"]=state["revision"].clone();completion["choreographyBinding"]=state["binding"].clone();completion["packages"]=s.get_assignment(text(&attempt,"assignmentId")?)?["choreography"]["responsibility"]["packages"].clone();}
                if checkpoint {completion["mode"]=json!("checkpoint");completion["successor"]=input["successor"].clone();}
                if let Some((source, digest)) = &scope { completion["conflictRefreshId"]=source.refresh["requestId"].clone();completion["worktreeHash"]=json!(digest); }
                let mut completions = array(&record["completions"]).to_vec(); completions.push(completion); record["completions"] = json!(completions); Ok(record)
            })?;
            previous(s)?.ok_or_else(|| conflict("Completion receipt disappeared."))
        })
    }
    pub async fn commit(&self, run_id: &str, completion: &Value) -> Result<Value> {
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        self.commit_unlocked(run_id, completion).await
    }
    pub(crate) async fn commit_unlocked(&self, run_id: &str, completion: &Value) -> Result<Value> {
        if completion.get("conflictRefreshId").is_some() {
            return self.commit_conflict(run_id, completion).await;
        }
        self.no_conflict(run_id)?;
        let work = self.status(run_id).await?;
        self.store(|s| Self::committable(s, run_id, completion))?;
        if ["base", "requirements", "instructions"]
            .iter()
            .any(|k| work["drift"][k] == true)
            || work["revision"]["worktreeHash"] != completion["worktreeHash"]
        {
            return Err(conflict(
                "Repository content or requirements changed after completion was requested.",
            ));
        }
        let path = self.path(&work)?;
        let marker = format!("Crewroom-operation: {}", text(completion, "id")?);
        if work["revision"]["head"] != completion["head"] {
            if self
                .inner
                .git
                .run(path, &["log", "-1", "--format=%B"])
                .await?
                .lines()
                .filter(|l| *l == marker)
                .count()
                != 1
                || self.inner.git.reference(path, "HEAD^").await? != text(completion, "head")?
                || work["revision"]["dirty"] != false
            {
                return Err(conflict("Unexpected commit appeared during completion."));
            }
        } else {
            let dispatch = |launch: &mut dyn FnMut() -> Result<tokio::process::Child>| {
                self.store(|s| {
                    Self::committable(s, run_id, completion)?;
                    self.metadata(&work)?;
                    launch()
                })
            };
            self.inner
                .git
                .mutate(run_id, path, &["add", "--all", "--", "."], dispatch)
                .await?;
            let tree = self
                .inner
                .git
                .mutate(run_id, path, &["write-tree"], dispatch)
                .await?;
            if tree.trim_end() != text(&work["revision"], "tree")? {
                self.inner
                    .git
                    .mutate(
                        run_id,
                        path,
                        &[
                            "-c",
                            "user.name=Crewroom",
                            "-c",
                            "user.email=crewroom@localhost",
                            "commit",
                            "--no-gpg-sign",
                            "-m",
                            &format!("{}\n\n{marker}", text(completion, "summary")?),
                        ],
                        dispatch,
                    )
                    .await?;
            }
        }
        let checkpoint = self
            .checkpoint(&work, "completed", Some(text(completion, "attemptId")?))
            .await?;
        if checkpoint["revision"]["dirty"] != false
            || checkpoint["revision"]["worktreeHash"] != completion["worktreeHash"]
        {
            return Err(conflict(
                "Committed revision does not match the completed files.",
            ));
        }
        self.store(|s| {
            Self::committable(s, run_id, completion)?;
            s.checkpoint_repository(run_id, &checkpoint)
        })?;
        Ok(checkpoint["revision"].clone())
    }
    /// Generated build output may be added, but every admitted source entry must
    /// retain its original type, executable bits and raw bytes.
    pub fn verify_export(destination: &Path, input: &Value) -> Result<bool> {
        crate::canonical(destination)?;
        let entries = input
            .as_array()
            .ok_or_else(|| conflict("Invalid snapshot manifest."))?;
        for entry in entries {
            let name = text(entry, "path")?;
            files::name(name)?;
            let path = destination.join(name);
            let mut parent = path.parent().unwrap();
            while parent != destination {
                if fs::symlink_metadata(parent)?.file_type().is_symlink() {
                    return Err(conflict("Snapshot parent became a symbolic link."));
                }
                parent = parent
                    .parent()
                    .ok_or_else(|| conflict("Snapshot path escaped."))?;
            }
            let metadata = fs::symlink_metadata(&path)?;
            let mode = entry["mode"]
                .as_u64()
                .ok_or_else(|| conflict("Invalid snapshot mode."))?;
            if (!metadata.is_file() && !metadata.file_type().is_symlink())
                || metadata.len() > 50 * 1024 * 1024
                || u64::from(metadata.mode() & 0o170111) != mode & 0o170111
            {
                return Ok(false);
            }
            let digest = if metadata.file_type().is_symlink() {
                hash(fs::read_link(&path)?.as_os_str().as_encoded_bytes())
            } else {
                hash(files::regular_bytes(destination, name, 50 * 1024 * 1024)?.0)
            };
            if entry["hash"] != digest {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub async fn export_revision(
        &self,
        run_id: &str,
        revision: &Value,
        destination: &Path,
    ) -> Result<PathBuf> {
        let lock = self.lock(run_id);
        let _guard = lock.lock().await;
        self.export_unlocked(run_id, revision, destination).await
    }
    pub(crate) async fn export_unlocked(
        &self,
        run_id: &str,
        revision: &Value,
        destination: &Path,
    ) -> Result<PathBuf> {
        self.no_conflict(run_id)?;
        let work = self.status(run_id).await?;
        if work.get("writer").is_some()
            || work["revision"]["dirty"] != false
            || work["revision"] != *revision
        {
            return Err(conflict(
                "The candidate revision changed before its check snapshot.",
            ));
        }
        if !destination.is_absolute() || fs::symlink_metadata(destination).is_ok() {
            return Err(conflict("Check snapshot needs a new private destination."));
        }
        let mut directory = PathBuf::from("/");
        for part in destination.components().skip(1) {
            let std::path::Component::Normal(part) = part else {
                return Err(conflict("Invalid snapshot destination."));
            };
            directory.push(part);
            crate::create_root(&directory)?;
        }
        let prefix = format!("--prefix={}/", destination.display());
        self.inner
            .git
            .mutate(
                run_id,
                self.path(&work)?,
                &[
                    "checkout-index",
                    "--all",
                    "--ignore-skip-worktree-bits",
                    &prefix,
                ],
                |launch| {
                    self.store(|s| {
                        if s.repository_work(run_id)?.is_none_or(|w| {
                            w.get("writer").is_some()
                                || w["branch"] != work["branch"]
                                || Self::current_requirements(s, &w).ok().as_deref()
                                    != revision["requirementsHash"].as_str()
                        }) {
                            return Err(conflict("Source changed before snapshot dispatch."));
                        }
                        self.metadata(&work)?;
                        launch()
                    })
                },
            )
            .await?;
        let files = self.files(&work).await?;
        let snapshot = destination.to_owned();
        let expected = json!(files);
        if !tokio::task::spawn_blocking(move || Self::verify_export(&snapshot, &expected))
            .await
            .map_err(|_| conflict("Snapshot verification interrupted."))??
        {
            return Err(conflict(
                "Check snapshot differs from the admitted source revision.",
            ));
        }
        if self.status(run_id).await?["revision"] != *revision {
            return Err(conflict(
                "The candidate revision changed during snapshot creation.",
            ));
        }
        Ok(destination.into())
    }
}
