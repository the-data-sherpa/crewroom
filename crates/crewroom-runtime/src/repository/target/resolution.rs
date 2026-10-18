//! A conflict workspace is a separate source scope, never an alternate path
//! supplied by an agent. Revalidate its original branch and pinned target too.
use super::*;

pub(crate) struct ConflictSource {
    pub original: Value,
    pub work: Value,
    pub refresh: Value,
}

impl<S: StoreAccess> Repository<S> {
    pub fn unresolved_conflict(&self, id: &str) -> Result<Option<Value>> {
        let work = self.owned(id)?;
        Ok(Self::unresolved_scope(&work).cloned())
    }
    pub(crate) fn unresolved_scope(work: &Value) -> Option<&Value> {
        let entries = array(&work["refreshes"]);
        entries
            .iter()
            .find(|r| {
                r["state"] == "rebasing"
                    || r["resolutionWorkspace"]["legacyOwnershipUnknown"] == true
                    || ["preparing", "continuing"]
                        .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
            })
            .or_else(|| {
                entries
                    .iter()
                    .rev()
                    .find(|r| r["state"] == "conflicted" && r.get("supersededBy").is_none())
            })
    }
    pub(crate) fn active_scope(work: &Value) -> Option<&Value> {
        Self::unresolved_scope(work).filter(|r| {
            r["state"] == "conflicted"
                && ["conflicted", "ready"]
                    .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
        })
    }
    pub fn active_conflict(&self, id: &str) -> Result<Option<Value>> {
        Ok(self.unresolved_conflict(id)?.filter(|r| {
            r["state"] == "conflicted"
                && ["conflicted", "ready"]
                    .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
        }))
    }
    pub(crate) fn conflict_unchanged(s: &Store, source: &ConflictSource) -> Result<()> {
        let id = text(&source.original, "runId")?;
        let coding = s.coding_record(id)?.unwrap_or(Value::Null);
        let candidate = array(&coding["candidates"]).last().unwrap_or(&Value::Null);
        if s.repository_work(id)?.as_ref() != Some(&source.original)
            || Self::current_requirements(s, &source.original)?
                != source.original["requirementsHash"]
            || candidate["id"] != source.refresh["sourceCandidateId"]
            || candidate["revision"]["head"] != source.refresh["fromHead"]
            || candidate["revision"]["base"] != source.refresh["fromBase"]
        {
            return Err(conflict(
                "Conflict ownership, source candidate or accepted requirements changed.",
            ));
        }
        Ok(())
    }
    pub(crate) async fn conflict_work(
        &self,
        id: &str,
        request: Option<&str>,
        continuing: bool,
    ) -> Result<ConflictSource> {
        self.inner.git.reconcile(id).await?;
        let original = self.owned(id)?;
        let entries = array(&original["refreshes"]);
        let r = entries
            .iter()
            .rev()
            .find(|r| r["state"] == "conflicted" && r.get("supersededBy").is_none())
            .ok_or_else(|| conflict("Conflict resolution workspace is not available."))?
            .clone();
        let w = &r["resolutionWorkspace"];
        let state = w["state"].as_str().unwrap_or("");
        if request.is_some_and(|key| r["requestId"] != key)
            || !(["conflicted", "ready", "promoting"].contains(&state)
                || continuing && state == "continuing")
            || request.is_none() && state == "promoting"
            || w["legacyOwnershipUnknown"] == true
            || entries.iter().any(|other| {
                other["state"] == "rebasing"
                    || other["resolutionWorkspace"]["state"] == "preparing"
                    || other["resolutionWorkspace"]["state"] == "continuing"
                        && (!continuing || other["requestId"] != r["requestId"])
            })
        {
            return Err(conflict(
                "Conflict resolution ownership is unverified; retain the workspace.",
            ));
        }
        let key = text(&r, "requestId")?;
        if !crewroom_core::store::valid_uuid(key) {
            return Err(conflict("Conflict request identity changed."));
        }
        let original_path = self.path(&original)?;
        let parent = fs::canonicalize(
            original_path
                .parent()
                .ok_or_else(|| conflict("Conflict workspace parent unavailable."))?,
        )?;
        let expected_path = parent.join(format!(
            "{}-refresh-{key}",
            original_path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| conflict("Conflict workspace path requires UTF-8."))?
        ));
        let branch = format!("{}-refresh-{key}", text(&original, "branch")?);
        if w["path"] != json!(expected_path)
            || w["branch"] != branch
            || w["gitFile"].as_str().is_none_or(str::is_empty)
            || original["base"] != r["fromBase"]
        {
            return Err(conflict(
                "Conflict workspace identity or accepted base changed.",
            ));
        }
        let mut work = original.clone();
        work["path"] = w["path"].clone();
        work["branch"] = w["branch"].clone();
        work["gitFile"] = w["gitFile"].clone();
        let source = ConflictSource {
            original,
            work,
            refresh: r,
        };
        self.store(|s| Self::conflict_unchanged(s, &source))?;
        self.validate(&source.original).await?;
        self.metadata(&source.work)?;
        let checkout = Path::new(text(&source.original["repository"], "path")?);
        let root = self.path(&source.work)?;
        let r = &source.refresh;
        let w = &r["resolutionWorkspace"];
        self.inner
            .git
            .run(
                checkout,
                &[
                    "merge-base",
                    "--is-ancestor",
                    text(r, "fromBase")?,
                    text(r, "fromHead")?,
                ],
            )
            .await
            .map_err(|_| conflict("Conflict source no longer contains its accepted base."))?;
        let listed = self.branches(checkout).await?;
        let isolated = listed.iter().find(|b| b["worktree"] == source.work["path"]);
        let rebasing = self.rebase_in_progress(&source.work).await?;
        if rebasing {
            self.owned_rebase(&source.work, r).await?;
        }
        let state = text(w, "state")?;
        let branch_ref = format!("refs/heads/{}", text(w, "branch")?);
        let isolated_valid = isolated.is_some_and(|b| {
            b["branch"] == branch_ref
                || ["conflicted", "continuing"].contains(&state)
                    && rebasing
                    && b["detached"] == true
        });
        let branch_head = self.inner.git.reference(checkout, &branch_ref).await?;
        let workspace_head = self.inner.git.reference(root, "HEAD").await?;
        let current = self.status(id).await?;
        let manifest = self.files(&source.work).await?;
        let instructions = json_hash(&self.instructions(&source.work, &manifest).await?);
        if !isolated_valid
            || self.common(root).await? != Path::new(text(&source.original, "commonDir")?)
            || (current["revision"]["head"] != r["fromHead"]
                && !(state == "promoting" && current["revision"]["head"] == w["toHead"]))
            || (branch_head != text(r, "fromHead")? && branch_head != workspace_head)
            || self
                .inner
                .git
                .reference(checkout, text(r, "targetRef")?)
                .await?
                != text(r, "toBase")?
            || current["revision"]["dirty"] != false
            || current["drift"]["requirements"] == true
            || current["drift"]["instructions"] == true
            || instructions
                != w.get("instructionsHash")
                    .cloned()
                    .unwrap_or_else(|| json!(json_hash(&source.original["instructions"])))
            || &source.original["requirementsHash"]
                != w.get("requirementsHash")
                    .unwrap_or(&source.original["requirementsHash"])
        {
            return Err(conflict(
                "Conflict source, workspace, instructions or pinned target changed; stop for the operator.",
            ));
        }
        self.metadata(&source.original)?;
        self.metadata(&source.work)?;
        self.store(|s| {
            Self::conflict_unchanged(s, &source)?;
            if state == "promoting" {
                let coding = s.coding_record(id)?.unwrap_or(Value::Null);
                if w["operationId"].is_null()
                    || w["toHead"].is_null()
                    || !array(&coding["completions"]).iter().any(|c| {
                        c["id"] == w["operationId"] && c["conflictRefreshId"] == r["requestId"]
                    })
                {
                    return Err(conflict("Conflict promotion lost its recorded completion."));
                }
            }
            Ok(())
        })?;
        Ok(source)
    }
    pub(crate) async fn validated_conflict_base_drift(
        &self,
        id: &str,
        current: &Value,
        request: Option<&str>,
    ) -> Result<bool> {
        let active = if let Some(key) = request {
            self.unresolved_conflict(id)?
                .is_some_and(|r| r["requestId"] == key)
        } else {
            self.active_conflict(id)?.is_some()
        };
        if !active {
            return Ok(request.is_none() && current["drift"]["base"] != true);
        }
        let source = self.conflict_work(id, request, false).await?;
        if source.original["base"] != current["base"]
            || source.original["targetRef"] != current["targetRef"]
            || (current["revision"]["head"] != source.refresh["fromHead"]
                && !(request.is_some()
                    && current["revision"]["head"]
                        == source.refresh["resolutionWorkspace"]["toHead"]))
        {
            return Ok(false);
        }
        Ok(current["drift"]["base"] != true
            || source.original["targetRef"] == source.refresh["targetRef"]
                && current["revision"]["base"] == source.refresh["toBase"])
    }
    pub(super) async fn resolved_conflict_head(
        &self,
        source: &ConflictSource,
    ) -> Result<Option<String>> {
        let r = &source.refresh;
        let work = &source.work;
        let root = self.path(work)?;
        if self.rebase_in_progress(work).await? {
            return Ok(None);
        }
        let head = self.inner.git.reference(root, "HEAD").await?;
        let source_paths = self
            .changed_paths(work, text(r, "fromBase")?, text(r, "fromHead")?)
            .await?;
        let rebased_paths = self.changed_paths(work, text(r, "toBase")?, &head).await?;
        if self
            .inner
            .git
            .run(
                root,
                &["merge-base", "--is-ancestor", text(r, "toBase")?, &head],
            )
            .await
            .is_err()
            || source_paths != rebased_paths
            || !self
                .inner
                .git
                .run(
                    root,
                    &[
                        "status",
                        "--porcelain=v1",
                        "--untracked-files=all",
                        "--ignore-submodules=none",
                    ],
                )
                .await?
                .is_empty()
        {
            return Ok(None);
        }
        Ok(Some(head))
    }
    pub async fn validate_conflict_workspace(&self, id: &str) -> Result<Value> {
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        self.validate_conflict_unlocked(id).await
    }
    pub(crate) async fn validate_conflict_unlocked(&self, id: &str) -> Result<Value> {
        // Unknown checked children must remain an ownership barrier, not become
        // an operator-required workspace that could later be superseded.
        self.inner.git.reconcile(id).await?;
        let original = self.owned(id)?;
        let r = self
            .active_conflict(id)?
            .ok_or_else(|| conflict("No safe isolated target conflict is available."))?;
        let mut workspace = r["resolutionWorkspace"].clone();
        let source = match self.conflict_work(id, None, false).await {
            Ok(source) => source,
            Err(error) => {
                workspace["state"] = json!("operator_required");
                self.store(|s| {
                    if s.repository_work(id)?.as_ref() != Some(&original) {
                        return Err(conflict("Conflict ownership changed during validation."));
                    }
                    s.record_target_refresh_workspace(id, text(&r, "requestId")?, &workspace, None)
                })?;
                return Err(error);
            }
        };
        if workspace["state"] == "conflicted" {
            if self.rebase_in_progress(&source.work).await? {
                let conflicts = self.conflict_paths(&source.work).await?;
                let valid = self
                    .conflict_paths_within_scope(&source.work, &r, &conflicts)
                    .await?;
                if !valid {
                    workspace["state"] = json!("operator_required");
                }
                self.store(|s| {
                    Self::conflict_unchanged(s, &source)?;
                    s.record_target_refresh_workspace(
                        id,
                        text(&r, "requestId")?,
                        &workspace,
                        Some(&json!(conflicts)),
                    )
                })?;
                if !valid {
                    return Err(conflict(
                        "Conflict paths exceed the accepted scope; operator decision required.",
                    ));
                }
                return Ok(
                    json!({"requestId":r["requestId"],"state":"conflicted","conflicts":conflicts}),
                );
            }
            let resumed = |s: &Store| -> Result<()> {
                Self::conflict_unchanged(s, &source)?;
                let run = s.get_run(id)?;
                if run["status"] != "awaiting_runner"
                    || run["desiredState"] != "running"
                    || source.original.get("writer").is_some()
                    || array(&s.runtime(id)?["attempts"])
                        .iter()
                        .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
                {
                    return Err(conflict(
                        "Explicit Resume and stopped ownership are required to recover the isolated rebase.",
                    ));
                }
                Ok(())
            };
            self.store(resumed)?;
            let head = self.resolved_conflict_head(&source).await?;
            workspace["state"] = json!(if head.is_some() {
                "ready"
            } else {
                "operator_required"
            });
            if let Some(head) = &head {
                workspace["toHead"] = json!(head);
            }
            self.store(|s| {
                resumed(s)?;
                s.record_target_refresh_workspace(id, text(&r, "requestId")?, &workspace, None)
            })?;
            if head.is_none() {
                return Err(conflict(
                    "The interrupted rebase changed source scope; operator decision required.",
                ));
            }
        } else {
            let head = self.resolved_conflict_head(&source).await?;
            if head.is_none() || head.as_deref() != workspace["toHead"].as_str() {
                return Err(conflict(
                    "Recorded ready state no longer matches the isolated Git workspace.",
                ));
            }
            self.store(|s| Self::conflict_unchanged(s, &source))?;
        }
        Ok(
            json!({"requestId":r["requestId"],"state":workspace["state"],"conflicts":r["conflicts"]}),
        )
    }
}
