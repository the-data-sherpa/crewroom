//! Target refresh owns every Git child and preserves the admitted source until
//! a clean effect and a current target observation can be saved together.
use super::*;
use futures_util::future::BoxFuture;
mod cleanup;
mod continuation;
mod promotion;
pub(super) mod resolution;

pub(super) type TargetObserver =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Option<String>>> + Send + Sync>;

fn refresh(work: &Value, id: &str) -> Result<Value> {
    array(&work["refreshes"])
        .iter()
        .find(|r| r["requestId"] == id)
        .cloned()
        .ok_or_else(|| conflict("Target refresh is not recorded."))
}
fn response(work: Value, id: &str) -> Result<Value> {
    Ok(json!({"refresh":refresh(&work,id)?,"work":work}))
}
fn names(raw: &str) -> Result<Vec<String>> {
    let mut paths = raw
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect::<Vec<_>>();
    for path in &paths {
        files::name(path)?;
    }
    paths.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    paths.dedup();
    Ok(paths)
}
impl<S: StoreAccess> Repository<S> {
    /// Configure the read-only provider boundary. An absent or unavailable
    /// observer never validates a fetched snapshot as the live target.
    pub fn set_target_observer<F, Fut>(&self, observer: F)
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Option<String>>> + Send + 'static,
    {
        *self.inner.target_observer.write().unwrap() =
            Some(Arc::new(move |w| Box::pin(observer(w))));
    }
    pub(super) async fn verify_target(
        &self,
        work: &Value,
        refresh: &Value,
    ) -> Result<Option<String>> {
        let git = &self.inner.git;
        let checkout = Path::new(text(&work["repository"], "path")?);
        let target = text(refresh, "targetRef")?;
        if git.reference(checkout, target).await? != text(refresh, "toBase")? {
            return Err(conflict(
                "The pinned target snapshot changed; original branch is retained.",
            ));
        }
        let live = if target.starts_with("refs/crewroom/") {
            let observer = self
                .inner
                .target_observer
                .read()
                .unwrap()
                .clone()
                .ok_or_else(|| {
                    conflict(
                        "The live target observer is unavailable; original branch is retained.",
                    )
                })?;
            observer(work.clone())
                .await?
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    conflict("The live target is unavailable; original branch is retained.")
                })?
        } else {
            git.reference(checkout, target).await?
        };
        Ok((refresh["toBase"] != live).then_some(live))
    }
    /// Recheck durable ownership after every asynchronous observation and before
    /// dispatch. Current instruction bytes are checked separately by the caller.
    pub(super) fn target_guard(s: &Store, work: &Value, refresh: &Value) -> Result<()> {
        let id = text(work, "runId")?;
        let saved = s
            .repository_work(id)?
            .ok_or_else(|| conflict("Repository ownership disappeared."))?;
        let run = s.get_run(id)?;
        let coding = s.coding_record(id)?.unwrap_or(Value::Null);
        let publication = s.publication(id)?.unwrap_or(Value::Null);
        let source = array(&coding["candidates"]).last().unwrap_or(&Value::Null);
        if [
            "path",
            "branch",
            "commonDir",
            "gitFile",
            "base",
            "targetRef",
            "repository",
            "requirementsHash",
            "instructions",
        ]
        .iter()
        .any(|k| saved.get(*k) != work.get(*k))
            || saved.get("writer").is_some()
            || ["push", "pr"].iter().any(|step| {
                ["sending", "uncertain", "blocked"]
                    .contains(&publication[*step]["state"].as_str().unwrap_or(""))
            })
            || run["status"] != "paused"
            || run["desiredState"] == "cancelled"
            || array(&s.runtime(id)?["attempts"])
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            || Self::current_requirements(s, &saved)? != text(work, "requirementsHash")?
            || source["id"] != refresh["sourceCandidateId"]
            || source["revision"]["head"] != refresh["fromHead"]
            || source["revision"]["base"] != refresh["fromBase"]
            || saved["base"] != refresh["fromBase"]
            || array(&coding["completions"])
                .iter()
                .any(|c| c["state"] == "requested")
        {
            return Err(conflict(
                "Refresh source, accepted requirements or stopped ownership changed.",
            ));
        }
        Ok(())
    }
    fn recorded_target_guard(&self, work: &Value, expected: &Value) -> Result<()> {
        self.target_dispatch(work, expected, || Ok(()))
    }
    fn target_dispatch<T>(
        &self,
        work: &Value,
        expected: &Value,
        launch: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.store(|s| {
            Self::target_guard(s, work, expected)?;
            if refresh(
                &s.repository_work(text(work, "runId")?)?.unwrap(),
                text(expected, "requestId")?,
            )? != *expected
            {
                return Err(conflict("Target refresh intent changed."));
            }
            self.metadata(work)?;
            launch()
        })
    }
    pub(super) async fn rebase_in_progress(&self, work: &Value) -> Result<bool> {
        for name in ["rebase-merge", "rebase-apply"] {
            let path = self
                .inner
                .git
                .run(
                    self.path(work)?,
                    &["rev-parse", "--path-format=absolute", "--git-path", name],
                )
                .await?;
            match fs::symlink_metadata(path) {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => return Ok(true),
                Ok(_) => return Err(conflict("Rebase metadata identity changed.")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(false)
    }
    async fn owned_rebase(&self, work: &Value, r: &Value) -> Result<()> {
        let mut found = false;
        for name in ["rebase-merge", "rebase-apply"] {
            let path = PathBuf::from(
                self.inner
                    .git
                    .run(
                        self.path(work)?,
                        &["rev-parse", "--path-format=absolute", "--git-path", name],
                    )
                    .await?,
            );
            if !path.exists() {
                continue;
            }
            crate::canonical(&path)?;
            for (name, expected) in [
                ("orig-head", text(r, "fromHead")?.to_owned()),
                ("onto", text(r, "toBase")?.to_owned()),
                ("head-name", format!("refs/heads/{}", text(work, "branch")?)),
            ] {
                let (bytes, _) = files::regular_bytes(&path, name, 4096)?;
                if bytes != format!("{expected}\n").as_bytes() {
                    return Err(conflict(
                        "Retained rebase does not match its admitted source and target.",
                    ));
                }
            }
            if found {
                return Err(conflict(
                    "Multiple rebase metadata directories are present.",
                ));
            }
            found = true;
        }
        if !found {
            return Err(conflict("The recorded rebase is unavailable."));
        }
        Ok(())
    }
    async fn changed_paths(&self, work: &Value, from: &str, to: &str) -> Result<Vec<String>> {
        names(
            &self
                .inner
                .git
                .metadata(
                    self.path(work)?,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--name-only",
                        "--no-renames",
                        "-z",
                        from,
                        to,
                        "--",
                    ],
                )
                .await?,
        )
    }
    async fn conflict_paths(&self, work: &Value) -> Result<Vec<String>> {
        names(
            &self
                .inner
                .git
                .metadata(
                    self.path(work)?,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--name-only",
                        "--diff-filter=U",
                        "-z",
                    ],
                )
                .await?,
        )
    }
    pub(super) async fn conflict_paths_within_scope(
        &self,
        work: &Value,
        r: &Value,
        paths: &[String],
    ) -> Result<bool> {
        if paths.is_empty()
            || paths.iter().any(|p| {
                [Some("AGENTS.md"), Some("CLAUDE.md")]
                    .contains(&Path::new(p).file_name().and_then(|p| p.to_str()))
            })
        {
            return Ok(false);
        }
        let (source, target) = tokio::try_join!(
            self.changed_paths(work, text(r, "fromBase")?, text(r, "fromHead")?),
            self.changed_paths(work, text(r, "fromBase")?, text(r, "toBase")?)
        )?;
        Ok(paths
            .iter()
            .all(|p| source.contains(p) && target.contains(p)))
    }
    async fn no_merge_drivers(&self, work: &Value) -> Result<()> {
        match self
            .inner
            .git
            .raw(
                self.path(work)?,
                &[
                    "config",
                    "--includes",
                    "--get-regexp",
                    "^merge\\..*\\.driver$",
                ],
            )
            .await
        {
            Err(e) if e.code == Some(1) => Ok(()),
            Ok(v) if v.is_empty() => Ok(()),
            Ok(_) => Err(conflict(
                "The repository configures custom merge drivers; refresh the branch manually.",
            )),
            Err(e) => Err(e.into()),
        }
    }
    async fn rebase_target(
        &self,
        original: &Value,
        work: &Value,
        r: &Value,
    ) -> Result<git::MutationOutcome> {
        self.no_merge_drivers(work).await?;
        let id = text(original, "runId")?;
        let intent = self
            .inner
            .git
            .describe_mutation(
                id,
                self.path(work)?,
                &[
                    "-c",
                    "user.name=Crewroom",
                    "-c",
                    "user.email=crewroom@localhost",
                    "-c",
                    "commit.gpgSign=false",
                    "-c",
                    "rebase.autoStash=false",
                    "-c",
                    "rebase.autoSquash=false",
                    "-c",
                    "rebase.updateRefs=false",
                    "rebase",
                    "--no-rerere-autoupdate",
                    "--onto",
                    text(r, "toBase")?,
                    text(r, "fromBase")?,
                ],
            )
            .await?;
        self.validate(work).await?;
        let current = self.revision(work).await?;
        if current["head"] != r["fromHead"]
            || current["dirty"] != false
            || current["instructionsHash"] != json_hash(&original["instructions"])
        {
            return Err(conflict("Rebase source changed before dispatch."));
        }
        self.inner
            .git
            .execute_mutation(id, intent, |launch| {
                self.store(|s| {
                    Self::target_guard(s, original, r)?;
                    if refresh(&s.repository_work(id)?.unwrap(), text(r, "requestId")?)? != *r {
                        return Err(conflict("Rebase intent changed before dispatch."));
                    }
                    self.metadata(work)?;
                    launch()
                })
            })
            .await
    }
    /// Keyed replay precedes mutable target reads, including an unavailable
    /// provider. New admission names the exact candidate, even at a shared head.
    pub async fn refresh_base(
        &self,
        id: &str,
        key: &str,
        target: &str,
        source: &str,
        supersedes: Option<&Value>,
    ) -> Result<Value> {
        if !crewroom_core::store::valid_uuid(key) || !crewroom_core::store::valid_uuid(source) {
            return Err(conflict(
                "Target refresh needs valid request and candidate UUIDs.",
            ));
        }
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        let owned = self.owned(id)?;
        if array(&owned["refreshes"])
            .iter()
            .any(|r| r["requestId"] == key)
        {
            return response(owned, key);
        }
        self.store(|s| s.preflight_target_refresh(id, supersedes))?;
        let current = self.status(id).await?;
        if current["revision"]["dirty"] != false
            || current["drift"]["requirements"] == true
            || current["drift"]["instructions"] == true
            || self.rebase_in_progress(&current).await?
        {
            return Err(conflict(
                "The recorded candidate must be clean with accepted requirements and instructions before refreshing.",
            ));
        }
        let mut r = json!({"requestId":key,"at":crewroom_core::timestamp(crewroom_core::now_ms()),"fromBase":current["base"],"fromHead":current["revision"]["head"],"targetRef":target,"sourceCandidateId":source,"state":"rebasing"});
        if let Some(v) = supersedes {
            request(v, &["requestId", "reason"])?;
            limited(v, "reason", 1, 4000)?;
            r["supersedes"] = v.clone();
        }
        self.store(|s| Self::target_guard(s, &current, &r))?;
        let to = self
            .inner
            .git
            .reference(Path::new(text(&current["repository"], "path")?), target)
            .await?;
        r["toBase"] = json!(to);
        if r["fromBase"] == r["toBase"] {
            r["state"] = json!("completed");
            r["toHead"] = r["fromHead"].clone();
            r["candidateId"] = json!(source);
        } else {
            self.inner
                .git
                .run(
                    self.path(&current)?,
                    &["merge-base", "--is-ancestor", text(&r, "fromBase")?, &to],
                )
                .await
                .map_err(|_| {
                    conflict("The target no longer contains the validated base; it was rewritten.")
                })?;
            self.no_merge_drivers(&current).await?;
        }
        // Re-observe after ref/configuration awaits, then admit under the store
        // mutex. No changed source may be silently overwritten by rebase.
        let latest = self.status(id).await?;
        if latest != current {
            return Err(conflict("Refresh source changed before admission."));
        }
        let admitted = self.store(|s| {
            Self::target_guard(s, &current, &r)?;
            s.begin_target_refresh(id, &r)
        })?;
        if r["state"] == "completed" {
            return response(admitted, key);
        }
        let outcome = self.rebase_target(&admitted, &admitted, &r).await?;
        self.store(|s| {
            Self::target_guard(s, &admitted, &r)?;
            s.record_target_rebase_recovery(id, key, "rebase_stopped")
        })?;
        let r = refresh(&self.owned(id)?, key)?;
        if outcome.code == 0 {
            return self.complete_refresh(&admitted, &r).await;
        }
        let conflicts = self.conflict_paths(&admitted).await?;
        let head = self
            .inner
            .git
            .reference(self.path(&admitted)?, "HEAD")
            .await?;
        self.undo_refresh(&admitted, &r, &head).await?;
        let r = refresh(&self.owned(id)?, key)?;
        if conflicts.is_empty() {
            self.finish_target_hold(&admitted, &r, "conflicted", &[])?;
            return Err(conflict(
                "Rebasing onto the target failed. The branch is unchanged.",
            ));
        }
        let conflicts = self
            .create_conflict_workspace(&admitted, &r, &conflicts)
            .await?;
        let saved = self.finish_target_hold(
            &admitted,
            &refresh(&self.owned(id)?, key)?,
            "conflicted",
            &conflicts,
        )?;
        response(saved, key)
    }
    fn finish_target_hold(
        &self,
        work: &Value,
        r: &Value,
        state: &str,
        conflicts: &[String],
    ) -> Result<Value> {
        self.store(|s| {
            Self::target_guard(s, work, r)?;
            if refresh(
                &s.repository_work(text(work, "runId")?)?.unwrap(),
                text(r, "requestId")?,
            )? != *r
            {
                return Err(conflict("Target hold changed before receipt."));
            }
            self.metadata(work)?;
            s.finish_target_refresh(
                text(work, "runId")?,
                text(r, "requestId")?,
                &json!({"state":state,"conflicts":conflicts}),
                None,
            )
        })
    }
    async fn undo_refresh(&self, work: &Value, r: &Value, expected: &str) -> Result<()> {
        let id = text(work, "runId")?;
        let key = text(r, "requestId")?;
        self.inner.git.reconcile(id).await?;
        self.metadata(work)?;
        self.recorded_target_guard(work, r)?;
        let rebasing = self.rebase_in_progress(work).await?;
        if rebasing {
            self.owned_rebase(work, r).await?;
        } else {
            self.validate(work).await?;
        }
        let head = self.inner.git.reference(self.path(work)?, "HEAD").await?;
        if head != expected
            || (!rebasing
                && !self
                    .inner
                    .git
                    .run(
                        self.path(work)?,
                        &[
                            "status",
                            "--porcelain=v1",
                            "--untracked-files=all",
                            "--ignore-submodules=none",
                        ],
                    )
                    .await?
                    .is_empty())
        {
            return Err(conflict(
                "Original source changed; rollback must not discard new work.",
            ));
        }
        self.store(|s| {
            Self::target_guard(s, work, r)?;
            s.record_target_rebase_recovery(id, key, "undo_started")
        })?;
        let stopped = refresh(&self.owned(id)?, key)?;
        if rebasing {
            let manifest = self.files(work).await?;
            let intent = self
                .inner
                .git
                .describe_mutation(id, self.path(work)?, &["rebase", "--abort"])
                .await?;
            self.owned_rebase(work, r).await?;
            if self.inner.git.reference(self.path(work)?, "HEAD").await? != expected
                || self.files(work).await? != manifest
            {
                return Err(conflict("Rebase source changed before rollback dispatch."));
            }
            let outcome = self
                .inner
                .git
                .execute_mutation(id, intent, |launch| {
                    self.target_dispatch(work, &stopped, launch)
                })
                .await?;
            if outcome.code != 0 {
                return Err(conflict("Target rebase abort failed; retain its intent."));
            }
        }
        let head = self.inner.git.reference(self.path(work)?, "HEAD").await?;
        if head != text(r, "fromHead")? {
            let intent = self
                .inner
                .git
                .describe_mutation(
                    id,
                    self.path(work)?,
                    &[
                        "reset",
                        "--hard",
                        "--no-recurse-submodules",
                        text(r, "fromHead")?,
                    ],
                )
                .await?;
            self.validate(work).await?;
            let current = self.revision(work).await?;
            if head != expected || current["head"] != head || current["dirty"] != false {
                return Err(conflict("Rollback source changed; retain new work."));
            }
            let outcome = self
                .inner
                .git
                .execute_mutation(id, intent, |launch| {
                    self.target_dispatch(work, &stopped, launch)
                })
                .await?;
            if outcome.code != 0 {
                return Err(conflict("Target rollback failed; retain its intent."));
            }
        }
        self.restored_source(work, r).await?;
        self.store(|s| {
            Self::target_guard(s, work, r)?;
            s.record_target_rebase_recovery(id, key, "restored")
        })?;
        Ok(())
    }
    async fn restored_source(&self, work: &Value, r: &Value) -> Result<()> {
        let current = self.status(text(work, "runId")?).await?;
        if self.rebase_in_progress(work).await?
            || current["revision"]["head"] != r["fromHead"]
            || current["revision"]["dirty"] != false
            || current["drift"]["requirements"] == true
            || current["drift"]["instructions"] == true
        {
            return Err(conflict(
                "Target rebase restoration is uncertain; retain its intent.",
            ));
        }
        self.store(|s| Self::target_guard(s, work, r))
    }
    async fn complete_refresh(&self, work: &Value, r: &Value) -> Result<Value> {
        let id = text(work, "runId")?;
        let key = text(r, "requestId")?;
        self.validate(work).await?;
        let head = self.inner.git.reference(self.path(work)?, "HEAD").await?;
        let source = self
            .changed_paths(work, text(r, "fromBase")?, text(r, "fromHead")?)
            .await?;
        let changed = self.changed_paths(work, text(r, "toBase")?, &head).await?;
        let contained = self
            .inner
            .git
            .run(
                self.path(work)?,
                &["merge-base", "--is-ancestor", text(r, "toBase")?, &head],
            )
            .await
            .is_ok();
        let current = self.status(id).await?;
        if current["revision"]["head"] != head || current["revision"]["dirty"] != false {
            return Err(conflict("Rebased source changed; retain its intent."));
        }
        if !contained || source != changed {
            self.undo_refresh(work, r, &head).await?;
            return response(
                self.finish_target_hold(work, &refresh(&self.owned(id)?, key)?, "conflicted", &[])?,
                key,
            );
        }
        let result: Result<Value> = async {
            if self.verify_target(work, r).await?.is_some() {
                return Err(conflict(
                    "The live target changed before the refresh receipt.",
                ));
            }
            let current = self.status(id).await?;
            self.recorded_target_guard(work, r)?;
            if current["revision"]["head"] != head
                || current["revision"]["dirty"] != false
                || current["drift"]["requirements"] == true
                || current["drift"]["instructions"] == true
            {
                return Err(conflict("Refresh source changed before completion."));
            }
            let mut moved = work.clone();
            moved["base"] = r["toBase"].clone();
            moved["targetRef"] = r["targetRef"].clone();
            let checkpoint = self.checkpoint(&moved, "refreshed", None).await?;
            if checkpoint["revision"]["head"] != head
                || checkpoint["revision"]["dirty"] != false
                || checkpoint["revision"]["instructionsHash"] != json_hash(&work["instructions"])
            {
                return Err(conflict("Refresh checkpoint changed before completion."));
            }
            let saved = self.store(|s| {
                Self::target_guard(s, work, r)?;
                s.finish_target_refresh(
                    id,
                    key,
                    &json!({"state":"completed","toHead":head}),
                    Some(&checkpoint),
                )
            })?;
            response(saved, key)
        }
        .await;
        if result.is_err() {
            // Undo only an unchanged, clean effect with the same stopped source
            // ownership. An observer failure never grants permission to reset.
            if let Ok(current) = self.status(id).await
                && current["revision"]["head"] == head
                && current["revision"]["dirty"] == false
                && self.recorded_target_guard(work, r).is_ok()
            {
                self.undo_refresh(work, r, &head).await?;
                self.finish_target_hold(work, &refresh(&self.owned(id)?, key)?, "conflicted", &[])?;
            }
        }
        result
    }
    async fn create_conflict_workspace(
        &self,
        work: &Value,
        r: &Value,
        conflicts: &[String],
    ) -> Result<Vec<String>> {
        let id = text(work, "runId")?;
        let key = text(r, "requestId")?;
        self.restored_source(work, r).await?;
        let path = self.path(work)?;
        let parent = fs::canonicalize(
            path.parent()
                .ok_or_else(|| conflict("Owned worktree parent unavailable."))?,
        )?;
        let path = parent.join(format!(
            "{}-refresh-{key}",
            path.file_name()
                .unwrap()
                .to_str()
                .ok_or_else(|| conflict("Worktree path requires UTF-8."))?
        ));
        let branch = format!("{}-refresh-{key}", text(work, "branch")?);
        let mut workspace = json!({"path":path,"branch":branch,"state":"preparing","requirementsHash":work["requirementsHash"],"instructionsHash":json_hash(&work["instructions"])});
        self.store(|s| {
            Self::target_guard(s, work, r)?;
            s.record_target_refresh_workspace(id, key, &workspace, Some(&json!(conflicts)))
        })?;
        let r = refresh(&self.owned(id)?, key)?;
        let checkout = Path::new(text(&work["repository"], "path")?);
        self.inner
            .git
            .run(
                checkout,
                &["check-ref-format", &format!("refs/heads/{branch}")],
            )
            .await?;
        if fs::symlink_metadata(&path).is_ok()
            || self
                .branches(checkout)
                .await?
                .iter()
                .any(|b| b["branch"] == format!("refs/heads/{branch}"))
        {
            return Err(conflict(
                "Target conflict workspace already exists without a completed record; inspect before retrying.",
            ));
        }
        let intent = self
            .inner
            .git
            .describe_mutation(
                id,
                checkout,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    path.to_str().unwrap(),
                    text(&r, "fromHead")?,
                ],
            )
            .await?;
        self.restored_source(work, &r).await?;
        let outcome = self
            .inner
            .git
            .execute_mutation(id, intent, |launch| self.target_dispatch(work, &r, launch))
            .await?;
        if outcome.code != 0 {
            return Err(conflict(
                "Conflict workspace preparation failed; retain its intent.",
            ));
        }
        let mut isolated = work.clone();
        isolated["path"] = json!(path);
        isolated["branch"] = json!(branch);
        isolated["targetRef"] = r["targetRef"].clone();
        isolated["startHead"] = r["fromHead"].clone();
        isolated.as_object_mut().unwrap().remove("gitFile");
        let git_file = self.validate(&isolated).await?;
        isolated["gitFile"] = json!(git_file);
        workspace["gitFile"] = json!(git_file);
        self.store(|s| {
            Self::target_guard(s, work, &r)?;
            s.record_target_refresh_workspace(id, key, &workspace, None)
        })?;
        let r = refresh(&self.owned(id)?, key)?;
        let outcome = self.rebase_target(work, &isolated, &r).await?;
        let found = self.conflict_paths(&isolated).await?;
        let entries = self.files(&isolated).await?;
        let instructions = json_hash(&self.instructions(&isolated, &entries).await?)
            == workspace["instructionsHash"];
        let valid = outcome.code != 0
            && found == conflicts
            && instructions
            && self
                .conflict_paths_within_scope(&isolated, &r, &found)
                .await?
            && self.rebase_in_progress(&isolated).await?;
        if valid {
            self.owned_rebase(&isolated, &r).await?;
        }
        workspace["state"] = json!(if valid {
            "conflicted"
        } else {
            "operator_required"
        });
        self.store(|s| {
            Self::target_guard(s, work, &r)?;
            s.record_target_refresh_workspace(id, key, &workspace, None)
        })?;
        Ok(if found.is_empty() {
            conflicts.to_vec()
        } else {
            found
        })
    }
    /// A stopped child alone cannot prove a successful rebase. An unknown exit
    /// or rollback remains held; an observed stopped effect is checked from Git.
    pub async fn reconcile_refresh(&self, id: &str) -> Result<()> {
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        let work = self.owned(id)?;
        self.inner.git.reconcile(id).await?;
        if array(&work["refreshes"]).iter().any(|r| {
            ["preparing", "continuing", "promoting"]
                .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
        }) {
            return Err(conflict(
                "A retained workspace has unverified Git ownership; retain it for inspection.",
            ));
        }
        let Some(r) = array(&work["refreshes"])
            .iter()
            .rev()
            .find(|r| r["state"] == "rebasing")
        else {
            return Ok(());
        };
        self.recorded_target_guard(&work, r)?;
        if !["rebase_stopped", "restored"].contains(&r["rebaseRecovery"].as_str().unwrap_or("")) {
            return Err(conflict(
                "Git rebase exit or rollback is unproved; retain its intent.",
            ));
        }
        if !r["resolutionWorkspace"].is_null() {
            if r["rebaseRecovery"] != "restored" {
                return Err(conflict(
                    "Conflict workspace recovery lacks a completed original rollback.",
                ));
            }
            self.restored_source(&work, r).await?;
            let w = &r["resolutionWorkspace"];
            if !["conflicted", "operator_required"].contains(&w["state"].as_str().unwrap_or("")) {
                return Err(conflict("Retained conflict workspace needs inspection."));
            }
            // Workspace preparation already recorded its numeric stopped effect.
            // Preserve it; scoped source validation is required before any writer.
            self.finish_target_hold(
                &work,
                r,
                "conflicted",
                &array(&r["conflicts"])
                    .iter()
                    .map(|p| p.as_str().unwrap_or("").to_owned())
                    .collect::<Vec<_>>(),
            )?;
            return Ok(());
        }
        let rebasing = self.rebase_in_progress(&work).await?;
        let head = self.inner.git.reference(self.path(&work)?, "HEAD").await?;
        if r["rebaseRecovery"] == "restored" {
            self.restored_source(&work, r).await?;
        } else if !rebasing && head != text(r, "fromHead")? {
            self.complete_refresh(&work, r).await?;
            return Ok(());
        } else {
            if rebasing {
                self.owned_rebase(&work, r).await?;
            }
            if self
                .inner
                .git
                .reference(
                    Path::new(text(&work["repository"], "path")?),
                    &format!("refs/heads/{}", text(&work, "branch")?),
                )
                .await?
                != text(r, "fromHead")?
            {
                return Err(conflict("Interrupted rebase source branch changed."));
            }
            self.undo_refresh(&work, r, &head).await?;
        }
        self.finish_target_hold(
            &work,
            &refresh(&self.owned(id)?, text(r, "requestId")?)?,
            "interrupted",
            &[],
        )?;
        Ok(())
    }
}
