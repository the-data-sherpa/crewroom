//! Promotion is a separately owned checked reset, admitted only after the
//! resolver has stopped. Recovery proves the actual effect; it never relaunches.
use super::*;
use resolution::ConflictSource;
impl<S: StoreAccess> Repository<S> {
    pub(crate) async fn ready_conflict(
        &self,
        id: &str,
        key: &str,
    ) -> Result<(ConflictSource, String)> {
        let source = self.conflict_work(id, Some(key), false).await?;
        let expected = source.refresh["resolutionWorkspace"]["toHead"].as_str();
        let head = self.resolved_conflict_head(&source).await?;
        if head.is_none() || head.as_deref() != expected {
            return Err(conflict(
                "Conflict resolution changed after ready state; stop for the operator.",
            ));
        }
        let digest = json_hash(&json!(self.files(&source.work).await?));
        self.store(|s| Self::conflict_unchanged(s, &source))?;
        Ok((source, digest))
    }
    fn promotion_guard(
        &self,
        s: &Store,
        source: &ConflictSource,
        completion: &Value,
    ) -> Result<()> {
        Self::completion_stopped(
            s,
            text(&source.original, "runId")?,
            completion,
            source.refresh["resolutionWorkspace"]["state"] == "promoting",
        )?;
        Self::conflict_unchanged(s, source)?;
        let w = &source.refresh["resolutionWorkspace"];
        if completion["conflictRefreshId"] != source.refresh["requestId"]
            || completion["head"] != source.refresh["fromHead"]
            || w["state"] == "promoting" && w["operationId"] != completion["id"]
        {
            return Err(conflict("Conflict promotion completion identity changed."));
        }
        self.metadata(&source.original)?;
        self.metadata(&source.work)?;
        Ok(())
    }
    async fn promotion_source(&self, id: &str, completion: &Value) -> Result<ConflictSource> {
        let (source, digest) = self
            .ready_conflict(id, text(completion, "conflictRefreshId")?)
            .await?;
        if completion["worktreeHash"] != digest {
            return Err(conflict(
                "Conflict resolution changed after completion was requested.",
            ));
        }
        let current = self.status(id).await?;
        if !self
            .validated_conflict_base_drift(
                id,
                &current,
                Some(text(completion, "conflictRefreshId")?),
            )
            .await?
        {
            return Err(conflict(
                "Original branch or target changed during promotion; no files were discarded.",
            ));
        }
        self.store(|s| self.promotion_guard(s, &source, completion))?;
        Ok(source)
    }
    fn promotion_intent(&self, source: &ConflictSource, completion: &Value) -> Result<git::Intent> {
        let w = &source.refresh["resolutionWorkspace"];
        if w["state"] != "promoting" || w["operationId"] != completion["id"] {
            return Err(conflict(
                "Persist the exact promotion intent before inspecting its checked process.",
            ));
        }
        self.inner.git.saved_promotion(
            text(&source.original, "runId")?,
            text(completion, "id")?,
            &w["promotionProcess"],
            self.path(&source.original)?,
            text(w, "toHead")?,
        )
    }
    async fn terminal_promotion(
        &self,
        id: &str,
        completion: &Value,
        changed: Option<&str>,
    ) -> Result<Value> {
        let source = self.promotion_source(id, completion).await?;
        let r = &source.refresh;
        let w = &r["resolutionWorkspace"];
        if w["state"] == "promoting"
            && crate::seal_checked(&self.promotion_intent(&source, completion)?.descriptor)
                .await
                .state
                != "stopped"
        {
            return Err(conflict(
                "Checked Git reset is live or uncertain; retain the promoting completion.",
            ));
        }
        // Sealing can wait for a launch lock. Re-prove source after that await.
        let source = self.promotion_source(id, completion).await?;
        let r = &source.refresh;
        let w = &r["resolutionWorkspace"];
        let original = &source.original;
        let current = self.status(id).await?;
        if current["revision"]["head"] != r["fromHead"] || current["revision"]["dirty"] != false {
            return Err(conflict(
                "Original branch changed or partially reset; retain the promoting completion.",
            ));
        }
        let mut proof = json!({"originalHead":current["revision"]["head"],"originalBase":original["base"],"originalPath":original["path"],"originalBranch":original["branch"],"originalTargetRef":original["targetRef"],"originalGitFile":original["gitFile"],"workspaceGitFile":w["gitFile"],"targetRef":r["targetRef"],"workspacePath":w["path"],"workspaceBranch":w["branch"],"toHead":w["toHead"],"worktreeHash":completion["worktreeHash"],"requirementsHash":original["requirementsHash"],"instructionsHash":json_hash(&original["instructions"]),"expectedTarget":r["toBase"]});
        self.store(|s| {
            self.promotion_guard(s, &source, completion)?;
            if let Some(head) = changed {
                proof["observedTarget"] = json!(head);
                s.reject_unpromoted_conflict(
                    id,
                    text(r, "requestId")?,
                    text(completion, "id")?,
                    &proof,
                )
            } else {
                s.interrupt_unpromoted_conflict(
                    id,
                    text(r, "requestId")?,
                    text(completion, "id")?,
                    &proof,
                )
            }
        })?;
        Err(conflict(if changed.is_some() {
            "The live target changed before promotion; the resolved workspace is retained for an operator retry."
        } else {
            "The checked reset stopped without applying the resolved conflict; an explicit operator retry is required."
        }))
    }
    pub(crate) async fn commit_conflict(&self, id: &str, completion: &Value) -> Result<Value> {
        let work = self.owned(id)?;
        let r = refresh(&work, text(completion, "conflictRefreshId")?)?;
        self.store(|s| {
            Self::completion_stopped(
                s,
                id,
                completion,
                r["state"] == "completed" || r["resolutionWorkspace"]["state"] == "promoting",
            )
        })?;
        if r["state"] == "completed" && r["resolutionWorkspace"]["operationId"] == completion["id"]
        {
            let current = self.status(id).await?;
            if current["revision"]["head"] != r["toHead"]
                || current["revision"]["base"] != r["toBase"]
                || current["revision"]["dirty"] != false
                || current["revision"]["worktreeHash"] != completion["worktreeHash"]
            {
                return Err(conflict(
                    "Promoted source changed before its candidate was recorded.",
                ));
            }
            return Ok(current["revision"].clone());
        }
        let source = self.promotion_source(id, completion).await?;
        if source.refresh["resolutionWorkspace"]["state"] == "promoting" {
            let intent = self.promotion_intent(&source, completion)?;
            let observed = crate::seal_checked(&intent.descriptor).await;
            if observed.state != "stopped" {
                return Err(conflict(
                    "Checked Git reset ownership is live or uncertain; retain the promoting completion.",
                ));
            }
            let current = self.status(id).await?;
            if current["revision"]["head"] == source.refresh["resolutionWorkspace"]["toHead"]
                && observed.owner.is_some()
            {
                return self.finish_promotion(id, completion).await;
            }
            let changed = self
                .verify_target(&source.original, &source.refresh)
                .await?;
            return self
                .terminal_promotion(id, completion, changed.as_deref())
                .await;
        }
        if source.refresh["resolutionWorkspace"]["state"] != "ready" {
            return Err(conflict(
                "Conflict rebase must be ready for its recorded completion.",
            ));
        }
        if let Some(changed) = self
            .verify_target(&source.original, &source.refresh)
            .await?
        {
            return self
                .terminal_promotion(id, completion, Some(&changed))
                .await;
        }
        let to_head = text(&source.refresh["resolutionWorkspace"], "toHead")?;
        if source.refresh["fromHead"] == to_head {
            return Err(conflict(
                "Conflict promotion has no source-changing reset to admit.",
            ));
        }
        let intent = self
            .inner
            .git
            .describe_promotion(
                id,
                self.path(&source.original)?,
                text(completion, "id")?,
                to_head,
            )
            .await?;
        let process = git::Git::promotion_record(&intent)?;
        let source = self.promotion_source(id, completion).await?;
        if source.refresh["resolutionWorkspace"]["toHead"] != to_head {
            return Err(conflict(
                "Ready conflict changed before promotion admission.",
            ));
        }
        self.store(|s| {
            self.promotion_guard(s, &source, completion)?;
            s.begin_target_promotion(
                id,
                text(&source.refresh, "requestId")?,
                text(completion, "id")?,
                to_head,
                &process,
            )
        })?;
        let admitted = self.owned(id)?;
        let r = refresh(&admitted, text(completion, "conflictRefreshId")?)?;
        if let Some(changed) = self.verify_target(&admitted, &r).await? {
            return self
                .terminal_promotion(id, completion, Some(&changed))
                .await;
        }
        // The target observer can await. Only this fresh source proof can
        // authorize the immediately guarded durable manifest and OS launch.
        let source = self.promotion_source(id, completion).await?;
        let current = self.status(id).await?;
        if current["revision"]["head"] != source.refresh["fromHead"] {
            return Err(conflict(
                "Original branch changed before checked reset dispatch.",
            ));
        }
        if source.refresh["resolutionWorkspace"]["promotionProcess"] != process {
            return Err(conflict(
                "Recorded promotion process changed before dispatch.",
            ));
        }
        let code = self
            .inner
            .git
            .execute_mutation(id, intent, |launch| {
                self.store(|s| {
                    Self::committable(s, id, completion)?;
                    self.promotion_guard(s, &source, completion)?;
                    launch()
                })
            })
            .await?
            .code;
        if code != 0 {
            return Err(conflict(
                "Checked Git reset failed; retain the promotion intent and inspect its bounded logs.",
            ));
        }
        self.finish_promotion(id, completion).await
    }
    async fn finish_promotion(&self, id: &str, completion: &Value) -> Result<Value> {
        let source = self.promotion_source(id, completion).await?;
        let r = &source.refresh;
        let w = &r["resolutionWorkspace"];
        let intent = self.promotion_intent(&source, completion)?;
        let observed = crate::inspect_checked(&intent.descriptor).await;
        if observed.state != "stopped" || observed.owner.is_none() {
            return Err(conflict("Stopped checked reset ownership is not proved."));
        }
        let current = self.status(id).await?;
        if current["revision"]["head"] != w["toHead"]
            || current["revision"]["dirty"] != false
            || current["revision"]["worktreeHash"] != completion["worktreeHash"]
        {
            return Err(conflict(
                "Promoted branch does not match the stopped checked reset.",
            ));
        }
        let mut moved = source.original.clone();
        moved["base"] = r["toBase"].clone();
        moved["targetRef"] = r["targetRef"].clone();
        let checkpoint = self
            .checkpoint(&moved, "refreshed", Some(text(completion, "attemptId")?))
            .await?;
        if checkpoint["revision"]["head"] != w["toHead"]
            || checkpoint["revision"]["base"] != r["toBase"]
            || checkpoint["revision"]["dirty"] != false
            || checkpoint["revision"]["worktreeHash"] != completion["worktreeHash"]
        {
            return Err(conflict(
                "Promoted branch did not match the isolated resolved revision.",
            ));
        }
        let mut promoted = w.clone();
        promoted["state"] = json!("promoted");
        self.store(|s| {
            self.promotion_guard(s, &source, completion)?;
            s.finish_target_refresh(
                id,
                text(r, "requestId")?,
                &json!({"state":"completed","toHead":w["toHead"],"resolutionWorkspace":promoted}),
                Some(&checkpoint),
            )
        })?;
        let observation = self.verify_target(&source.original, r).await;
        if !matches!(observation, Ok(None)) {
            self.store(|s| {s.require_attention(id,"The live target moved or could not be verified after the checked reset. Local promotion is retained; refresh and revalidate before publication.")})?;
        }
        Ok(checkpoint["revision"].clone())
    }
}
