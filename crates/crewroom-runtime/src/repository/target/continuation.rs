//! A live scoped writer may advance only the retained isolated rebase. The
//! durable continuation remains unresolved if either checked child is uncertain.
use super::*;
use resolution::ConflictSource;

fn marker_block(content: &str) -> bool {
    let mut stage = 0;
    for line in content.split_inclusive('\n') {
        let ended = line.ends_with('\n');
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if stage == 0 && ended && (line == "<<<<<<<" || line.starts_with("<<<<<<< ")) {
            stage = 1;
        } else if stage == 1 && ended && line == "=======" {
            stage = 2;
        } else if stage == 2 && (line == ">>>>>>>" || line.starts_with(">>>>>>> ")) {
            return true;
        }
    }
    false
}
impl<S: StoreAccess> Repository<S> {
    fn continuation_guard(
        &self,
        s: &Store,
        source: &ConflictSource,
        identity: &Value,
    ) -> Result<()> {
        Self::writer(s, text(identity, "attemptId")?)?;
        Self::conflict_unchanged(s, source)?;
        let w = &source.refresh["resolutionWorkspace"];
        if w["state"] != "continuing" || w["continuation"] != *identity {
            return Err(conflict(
                "Git continuation identity changed before dispatch.",
            ));
        }
        self.metadata(&source.original)?;
        self.metadata(&source.work)?;
        Ok(())
    }
    async fn continuation_git(
        &self,
        id: &str,
        identity: &Value,
        args: &[&str],
    ) -> Result<git::MutationOutcome> {
        let key = text(identity, "requestId")?;
        let intent = self
            .inner
            .git
            .describe_mutation(id, Path::new(text(identity, "path")?), args)
            .await?;
        let before = self.conflict_work(id, Some(key), true).await?;
        self.no_merge_drivers(&before.work).await?;
        let source = self.conflict_work(id, Some(key), true).await?;
        self.inner
            .git
            .execute_mutation(id, intent, |launch| {
                self.store(|s| {
                    self.continuation_guard(s, &source, identity)?;
                    launch()
                })
            })
            .await
    }
    pub async fn continue_target_conflict(
        &self,
        attempt_id: &str,
        tool_key: &str,
        input: &Value,
    ) -> Result<Value> {
        request(input, &["requestId"])?;
        let key = text(input, "requestId")?;
        if !crewroom_core::store::valid_uuid(key) {
            return Err(Error::Invalid("Invalid target refresh identity.".into()));
        }
        Self::tool_key(tool_key)?;
        let attempt = self.store(|s| s.get_attempt(attempt_id))?;
        let id = text(&attempt, "runId")?;
        let lock = self.lock(id);
        let _guard = lock.lock().await;
        let owner = self.store(|s| Self::writer(s, attempt_id))?;
        let held = Self::unresolved_scope(&owner)
            .filter(|r| r["requestId"] == key)
            .ok_or_else(|| conflict("No active target conflict is assigned to this writer."))?;
        let workspace = &held["resolutionWorkspace"];
        let receipts = array(&workspace["continuationReceipts"]);
        if let Some(receipt) = receipts
            .iter()
            .find(|r| r["attemptId"] == attempt_id && r["toolCallId"] == tool_key)
        {
            if receipt["requestId"] != key
                || receipts.last() != Some(receipt)
                || workspace["state"] != receipt["result"]["state"]
                || workspace.get("continuation").is_some()
            {
                return Err(conflict(
                    "This historical Git continuation cannot advance the current workspace. Inspect its current state.",
                ));
            }
            let source = self.conflict_work(id, Some(key), false).await?;
            if workspace["state"] == "ready"
                && self.resolved_conflict_head(&source).await?.as_deref()
                    != receipt["result"]["head"].as_str()
            {
                return Err(conflict(
                    "Conflict continuation result changed since its receipt.",
                ));
            }
            self.store(|s| {
                Self::writer(s, attempt_id)?;
                Self::conflict_unchanged(s, &source)
            })?;
            return Ok(receipt["result"].clone());
        }
        if workspace["state"] == "continuing" {
            return Err(conflict(
                "Git continuation ownership is unverified; retain the workspace for operator inspection.",
            ));
        }
        let source = self.conflict_work(id, Some(key), false).await?;
        if workspace["state"] == "ready" {
            self.validate_conflict_unlocked(id).await?;
            self.store(|s| {
                Self::writer(s, attempt_id)?;
                Self::conflict_unchanged(s, &source)
            })?;
            return Ok(json!({"state":"ready","head":workspace["toHead"],"conflicts":[]}));
        }
        if workspace["state"] != "conflicted" {
            return Err(conflict(
                "The target conflict requires an operator decision.",
            ));
        }
        // A vanished rebase without our durable continuation requires explicit
        // stopped/Resume recovery; a running writer cannot silently adopt it.
        if !self.rebase_in_progress(&source.work).await? {
            return Err(conflict(
                "Explicit Resume and stopped ownership are required to recover the isolated rebase.",
            ));
        }
        let paths = self.conflict_paths(&source.work).await?;
        if !self
            .conflict_paths_within_scope(&source.work, &source.refresh, &paths)
            .await?
        {
            let mut w = workspace.clone();
            w["state"] = json!("operator_required");
            self.store(|s| {
                Self::writer(s, attempt_id)?;
                Self::conflict_unchanged(s, &source)?;
                s.record_target_refresh_workspace(id, key, &w, Some(&json!(paths)))
            })?;
            return Ok(json!({"state":"operator_required","conflicts":paths}));
        }
        for path in &paths {
            let file = self.read_file(id, path).await?;
            if marker_block(text(&file, "content")?) {
                return Err(conflict(
                    "Resolve every conflict marker before continuing the target rebase.",
                ));
            }
        }
        let identity = json!({"attemptId":attempt_id,"toolCallId":tool_key,"requestId":key,"path":workspace["path"],"branch":workspace["branch"],"sourceHead":source.refresh["fromHead"],"paths":paths});
        self.store(|s| {
            Self::writer(s, attempt_id)?;
            Self::conflict_unchanged(s, &source)?;
            s.begin_target_conflict_continuation(id, key, &identity)
        })?;
        let result:Result<Value> = async {
            let mut args=vec!["add","--"];args.extend(paths.iter().map(String::as_str));
            if self.continuation_git(id,&identity,&args).await?.code != 0 {return Err(conflict("Git could not stage the retained conflict files."));}
            self.continuation_git(id,&identity,&["-c","user.name=Crewroom","-c","user.email=crewroom@localhost","-c","commit.gpgSign=false","-c","core.editor=true","rebase","--continue"]).await?;
            let source=self.conflict_work(id,Some(key),true).await?;
            let result=if self.rebase_in_progress(&source.work).await? {
                let conflicts=self.conflict_paths(&source.work).await?;
                let valid=self.conflict_paths_within_scope(&source.work,&source.refresh,&conflicts).await?;
                json!({"state":if valid {"conflicted"} else {"operator_required"},"conflicts":conflicts})
            } else {
                match self.resolved_conflict_head(&source).await? {
                    Some(head)=>json!({"state":"ready","head":head,"conflicts":[]}),
                    None=>json!({"state":"operator_required","conflicts":[]})
                }
            };
            self.store(|s| {self.continuation_guard(s,&source,&identity)?;s.finish_target_conflict_continuation(id,key,&identity,&result)})?;
            Ok(result)
        }.await;
        if result.is_err() {
            self.store(|s| {
                let run=s.get_run(id)?;
                if !["cancelled","completed","closed"].contains(&run["status"].as_str().unwrap_or("")) && run["desiredState"] != "cancelled" {
                    s.require_attention(id,"Isolated Git continuation ownership is unverified; retain the workspace and inspect before resuming.")?;
                }
                Ok(())
            })?;
        }
        result
    }
}
