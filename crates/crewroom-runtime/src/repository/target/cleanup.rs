//! Delete only an unchanged promoted temporary workspace after all evidence and
//! its notice are durable. A lost cleanup reply is reconciled from actual Git.
use super::*;
impl<S: StoreAccess> Repository<S> {
    fn cleanup_guard(&self, s: &Store, work: &Value, r: &Value) -> Result<()> {
        let id = text(work, "runId")?;
        let run = s.get_run(id)?;
        let coding = s.coding_record(id)?.unwrap_or(Value::Null);
        if s.repository_work(id)?.as_ref() != Some(work)
            || work.get("writer").is_some()
            || !["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
            || run["desiredState"] == "cancelled"
            || array(&s.runtime(id)?["attempts"])
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            || Self::current_requirements(s, work)? != work["requirementsHash"]
            || r["state"] != "completed"
            || r["resolutionWorkspace"]["state"] != "promoted"
            || !array(&coding["completions"]).iter().any(|c| {
                c["id"] == r["resolutionWorkspace"]["operationId"]
                    && c["state"] == "completed"
                    && c["candidateId"] == r["candidateId"]
                    && c["conflictRefreshId"] == r["requestId"]
            })
            || !array(&coding["candidates"]).iter().any(|c| {
                c["id"] == r["candidateId"]
                    && c["refreshedFrom"] == r["sourceCandidateId"]
                    && c["revision"]["head"] == r["toHead"]
            })
            || !s.delivery_notices(id)?.iter().any(|n| {
                n["payload"]["kind"] == "target_refresh"
                    && n["payload"]["subjectId"] == r["requestId"]
                    && n["payload"]["candidateId"] == r["candidateId"]
            })
        {
            return Err(conflict(
                "Promoted workspace cleanup requires stopped ownership and durable candidate, completion and notice.",
            ));
        }
        self.metadata(work)?;
        Ok(())
    }
    async fn clean_promoted_source(&self, id: &str, work: &Value, r: &Value) -> Result<()> {
        self.no_conflict(id)?;
        let current = self.status(id).await?;
        let candidate = self.store(|s| {
            let coding = s.coding_record(id)?.unwrap_or(Value::Null);
            array(&coding["candidates"])
                .iter()
                .find(|c| c["id"] == r["candidateId"])
                .cloned()
                .ok_or_else(|| conflict("Promoted candidate is unavailable."))
        })?;
        if current["revision"] != candidate["revision"] {
            return Err(conflict("Promoted original source changed before cleanup."));
        }
        self.store(|s| self.cleanup_guard(s, work, r))
    }
    async fn clean_promoted_workspace(&self, work: &Value, r: &Value) -> Result<Value> {
        let w = &r["resolutionWorkspace"];
        let mut isolated = work.clone();
        isolated["path"] = w["path"].clone();
        isolated["branch"] = w["branch"].clone();
        isolated["gitFile"] = w["gitFile"].clone();
        self.validate(&isolated).await?;
        if self.rebase_in_progress(&isolated).await?
            || self
                .inner
                .git
                .reference(self.path(&isolated)?, "HEAD")
                .await?
                != text(w, "toHead")?
            || !self
                .inner
                .git
                .run(
                    self.path(&isolated)?,
                    &[
                        "status",
                        "--porcelain=v1",
                        "--untracked-files=all",
                        "--ignored",
                        "--ignore-submodules=none",
                    ],
                )
                .await?
                .is_empty()
        {
            return Err(conflict(
                "Promoted conflict workspace changed or contains untracked files; retain it.",
            ));
        }
        Ok(isolated)
    }
    pub async fn cleanup_promoted_conflict(&self, id: &str, key: &str) -> Result<()> {
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        self.cleanup_promoted_unlocked(id, key).await
    }
    pub(crate) async fn cleanup_promoted_unlocked(&self, id: &str, key: &str) -> Result<()> {
        let work = self.owned(id)?;
        let r = refresh(&work, key)?;
        let w = &r["resolutionWorkspace"];
        self.clean_promoted_source(id, &work, &r).await?;
        let original = self.path(&work)?;
        let expected = fs::canonicalize(
            original
                .parent()
                .ok_or_else(|| conflict("Conflict workspace parent unavailable."))?,
        )?
        .join(format!(
            "{}-refresh-{key}",
            original
                .file_name()
                .and_then(|p| p.to_str())
                .ok_or_else(|| conflict("Conflict workspace path requires UTF-8."))?
        ));
        if !crewroom_core::store::valid_uuid(key)
            || w["path"] != json!(expected)
            || w["branch"] != format!("{}-refresh-{key}", text(&work, "branch")?)
            || w["toHead"] != r["toHead"]
        {
            return Err(conflict("Promoted workspace identity changed."));
        }
        let checkout = Path::new(text(&work["repository"], "path")?);
        let path = Path::new(text(w, "path")?);
        match fs::symlink_metadata(path) {
            Ok(_) => {
                let intent = self
                    .inner
                    .git
                    .describe_mutation(id, checkout, &["worktree", "remove", text(w, "path")?])
                    .await?;
                self.clean_promoted_source(id, &work, &r).await?;
                let isolated = self.clean_promoted_workspace(&work, &r).await?;
                let outcome = self
                    .inner
                    .git
                    .execute_mutation(id, intent, |launch| {
                        self.store(|s| {
                            self.cleanup_guard(s, &work, &r)?;
                            self.metadata(&isolated)?;
                            launch()
                        })
                    })
                    .await?;
                if outcome.code != 0 {
                    return Err(conflict(
                        "Promoted worktree removal was not confirmed; retain cleanup for inspection.",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        self.clean_promoted_source(id, &work, &r).await?;
        if self.branches(checkout).await?.iter().any(|b| {
            b["worktree"] == w["path"]
                || b["branch"] == format!("refs/heads/{}", w["branch"].as_str().unwrap_or(""))
        }) || fs::symlink_metadata(path).is_ok()
        {
            return Err(conflict("Promoted worktree removal is incomplete."));
        }
        let branch = format!("refs/heads/{}", text(w, "branch")?);
        let head = self
            .inner
            .git
            .run(
                checkout,
                &["for-each-ref", "--format=%(objectname)", &branch],
            )
            .await?;
        if head.is_empty() {
            return Ok(());
        }
        if head != text(w, "toHead")? {
            return Err(conflict("Promoted temporary branch moved."));
        }
        let intent = self
            .inner
            .git
            .describe_mutation(
                id,
                checkout,
                &["update-ref", "-d", &branch, text(w, "toHead")?],
            )
            .await?;
        self.clean_promoted_source(id, &work, &r).await?;
        let outcome = self
            .inner
            .git
            .execute_mutation(id, intent, |launch| {
                self.store(|s| {
                    self.cleanup_guard(s, &work, &r)?;
                    launch()
                })
            })
            .await?;
        if outcome.code != 0 {
            return Err(conflict("Temporary branch cleanup was not confirmed."));
        }
        Ok(())
    }
}
