//! Cancel stays terminal. A successor receives the existing worktree only after
//! fresh stopped-process, source and (when published) provider verification.
use super::*;
use futures_util::future::BoxFuture;

pub(super) type ContinuationObserver =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<()>> + Send + Sync>;

impl<S: StoreAccess> Repository<S> {
    pub fn set_continuation_observer<F, Fut>(&self, observer: F)
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        *self.inner.continuation_observer.write().unwrap() =
            Some(Arc::new(move |id| Box::pin(observer(id))));
    }

    pub async fn verify_cancelled_work(&self, id: &str, successor: Option<&str>) -> Result<Value> {
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        self.verify_cancelled_unlocked(id, successor).await
    }

    async fn verify_cancelled_unlocked(&self, id: &str, successor: Option<&str>) -> Result<Value> {
        let source = self.store(|s| s.cancelled_source(id, successor))?;
        if let Some(publication) = source.get("publication")
            && (publication["branch"] != source["work"]["branch"]
                || publication["repositoryPath"] != source["work"]["repository"]["path"]
                || publication["target"] != source["work"]["repository"]["targetBranch"])
        {
            return Err(conflict(
                "Continuation held: the recorded PR does not match the retained branch and repository.",
            ));
        }
        let observer = self.inner.continuation_observer.read().unwrap().clone();
        if source.get("publication").is_some() && observer.is_none() {
            return Err(conflict(
                "Continuation held: PR verification is unavailable.",
            ));
        }
        if let Some(observer) = observer {
            observer(id.to_owned()).await?;
        }
        // The historical run owns these descriptors, even after a transfer.
        self.inner.git.reconcile(id).await?;
        let observed = self.status(id).await?;
        let expected = &source["checkpoint"]["revision"];
        if observed["revision"]["head"] != expected["head"]
            || observed["revision"]["worktreeHash"] != expected["worktreeHash"]
        {
            return Err(conflict(
                "Continuation held: the recorded branch head or saved edits changed. Inspect the retained work.",
            ));
        }
        if observed["drift"]["instructions"] == true {
            return Err(conflict(
                "Continuation held: project instructions changed. Inspect the retained work before a new run.",
            ));
        }
        self.metadata(&source["work"])?;
        self.store(|s| {
            let mut current = s.cancelled_source(id, successor)?;
            if current["work"] != source["work"]
                || current["configured"] != source["configured"]
                || current.get("publication") != source.get("publication")
            {
                return Err(conflict("Continuation held: ownership, configuration or publication changed during verification."));
            }
            current["revision"] = observed["revision"].clone();
            current["targetMoved"] = observed["drift"]["base"].clone();
            Ok(current)
        })
    }

    /// Receipt replay precedes any physical or provider access. Queueing stores
    /// intent only; admission verifies the retained source again before transfer.
    pub async fn queue_cancelled_continuation(
        &self,
        id: &str,
        input: &Value,
        roles: &[Value],
    ) -> Result<Value> {
        if let Some(saved) = self.store(|s| s.continuation_replay(id, input))? {
            return Ok(saved);
        }
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        if let Some(saved) = self.store(|s| s.continuation_replay(id, input))? {
            return Ok(saved);
        }
        let result = async {
            let source = self.verify_cancelled_unlocked(id, None).await?;
            self.store(|s| {
                let current = s.cancelled_source(id, None)?;
                if current["configured"] != source["configured"]
                    || current.get("publication") != source.get("publication")
                {
                    return Err(conflict(
                        "Continuation held: configuration or publication changed before queueing.",
                    ));
                }
                s.queue_cancelled_continuation(id, input, roles, &source["work"])
            })
        }
        .await;
        match result {
            Ok(value) => Ok(value),
            Err(error) => match self.store(|s| s.continuation_replay(id, input))? {
                Some(saved) => Ok(saved),
                None => Err(error),
            },
        }
    }

    fn retained_link(source: &Value, link: &Value) -> Result<()> {
        if source["work"]["branch"] != link["branch"]
            || source["revision"]["head"] != link["expectedHead"]
            || source["revision"]["worktreeHash"] != link["worktreeHash"]
            || source["publication"]["id"] != link["publicationId"]
            || source["publication"]["pr"]["id"] != link["prId"]
        {
            return Err(conflict(
                "Continuation held: the captured branch or PR identity changed while queued.",
            ));
        }
        Ok(())
    }

    pub(super) async fn prepare_cancelled_unlocked(
        &self,
        id: &str,
        input: &Value,
        run: &Value,
    ) -> Result<Value> {
        let link = &run["continuedFrom"];
        let source_id = text(link, "runId")?;
        if input["mode"] != "continue"
            || input["branch"] != link["branch"]
            || input["expectedHead"] != link["expectedHead"]
        {
            return Err(conflict(
                "Continuation held: launch must retain the captured branch and head.",
            ));
        }
        if let Some(work) = self.store(|s| s.repository_work(id))? {
            if work["mode"] != "continue"
                || work["branch"] != link["branch"]
                || work["startHead"] != link["expectedHead"]
                || work["state"] != "ready"
            {
                return Err(conflict(
                    "Continuation held: the existing owner does not match its captured launch.",
                ));
            }
            let unexecuted = self.store(|s| {
                Ok(s.get_run(id)?.get("launchCompletedAt").is_none()
                    && array(&s.runtime(id)?["attempts"])
                        .iter()
                        .all(|a| a["state"] == "admitted"))
            })?;
            self.validate(&work).await?;
            // Original-source proof is last: successor metadata probes cannot
            // leave a stale saved-content observation at the admission boundary.
            let verified = if unexecuted {
                let source = self.verify_cancelled_unlocked(source_id, Some(id)).await?;
                Self::retained_link(&source, link)?;
                Some(source)
            } else {
                None
            };
            self.metadata(&work)?;
            self.store(|s| {
                let current = s.get_run(id)?;
                if let Some(source) = &verified {
                    let cancelled = s.cancelled_source(source_id, Some(id))?;
                    if current.get("launchCompletedAt").is_some()
                        || array(&s.runtime(id)?["attempts"])
                            .iter()
                            .any(|a| a["state"] != "admitted")
                        || cancelled["work"] != source["work"]
                        || cancelled["configured"] != source["configured"]
                        || cancelled.get("publication") != source.get("publication")
                    {
                        return Err(conflict(
                            "Continuation held: retained source changed before admission replay.",
                        ));
                    }
                }
                if current["continuedFrom"] != *link
                    || current["controlRevision"] != run["controlRevision"]
                    || s.repository_work(id)?.as_ref() != Some(&work)
                {
                    return Err(conflict(
                        "Continuation held: ownership changed during admission.",
                    ));
                }
                Ok(work)
            })
        } else {
            let source = self.verify_cancelled_unlocked(source_id, Some(id)).await?;
            Self::retained_link(&source, link)?;
            let dependencies = self.store(|s| s.dependency_merges(&s.get_run(id)?))?;
            if let Some(hold) = self
                .dependency_base_hold(id, Some(text(&source["work"], "base")?))
                .await?
            {
                return Err(conflict(&format!("Continuation held: {hold}")));
            }
            // Dependency probes await Git. Re-prove source and provider identity
            // afterwards, then keep final authorization and transfer synchronous.
            let source = self.verify_cancelled_unlocked(source_id, Some(id)).await?;
            Self::retained_link(&source, link)?;
            self.metadata(&source["work"])?;
            self.store(|s| {
                let current = s.get_run(id)?;
                let cancelled = s.cancelled_source(source_id, Some(id))?;
                if current["continuedFrom"] != *link
                    || current["controlRevision"] != run["controlRevision"]
                    || s.dependency_merges(&current)? != dependencies
                    || cancelled["work"] != source["work"]
                    || cancelled.get("publication") != source.get("publication")
                {
                    return Err(conflict("Continuation held: source, dependencies or queued identity changed before transfer."));
                }
                let mut revision = source["revision"].clone();
                revision["base"] = source["work"]["base"].clone();
                revision["requirementsHash"] = json!(requirements_hash(&current["taskSnapshot"],
                    (!array(&current["adoptions"]).is_empty()).then(||current["taskRevision"].as_u64()).flatten()));
                let checkpoint = json!({"id":uuid::Uuid::new_v4().to_string(),"at":crewroom_core::timestamp(crewroom_core::now_ms()),"reason":"prepared","revision":revision});
                let target = source["work"]["continuationTargetRef"].as_str().or_else(|| {
                    (source["targetMoved"] == true).then(|| source["work"]["targetRef"].as_str()).flatten()
                });
                s.transfer_cancelled_work(id, &source["work"], &source["configured"], &checkpoint, target)
            })
        }
    }
}
