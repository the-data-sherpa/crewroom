use super::*;
use std::sync::Arc;
use tokio::sync::Mutex;
enum AttemptError {
    Store(Error),
    Provider(providers::Error),
}
impl From<Error> for AttemptError {
    fn from(e: Error) -> Self {
        Self::Store(e)
    }
}
impl From<providers::Error> for AttemptError {
    fn from(e: providers::Error) -> Self {
        Self::Provider(e)
    }
}
fn presend(message: &str) -> providers::Error {
    providers::Error::Unavailable(providers::Unavailable {
        message: message.into(),
        status: None,
        request_sent: false,
    })
}
impl<S: StoreAccess> JiraDelivery<S> {
    /// Holds the short flight/store guards through the final durable check.
    fn active(&self, update: &mut Value, proof: Option<&Proof>) -> Result<bool> {
        self.flight.with_status(|closed| {
            if closed { update["reason"]=json!("Waiting for the server to resume.");return Ok(false); }
            self.store.with_store(|s| {
                let run=s.get_run(text(update,"runId")?)?;
                if update["kind"]=="research_outcome" {
                    if run["researchCompletion"]["revision"]==run["taskRevision"] && !present(&run["sourceConflict"])
                        && matches!(run["status"].as_str(),Some("running"|"pausing"|"paused")) {
                        update["reason"]=json!("Waiting for confirmed research completion.");return Ok(false);
                    }
                    if run["status"]!="completed" || run["researchOutcome"]["revision"]!=run["taskRevision"] || present(&run["sourceConflict"]) {
                        block(update,"The research outcome is no longer complete for current requirements.");return Ok(false);
                    }
                } else {
                    if present(&run["sourceConflict"]) || run["desiredState"]=="cancelled" {
                        block(update,"Run requirements changed or the run was cancelled. This update will not be sent.");return Ok(false);
                    }
                    if !controller(update) && (run["status"]!="running" || run["desiredState"]!="running") {
                        update["reason"]=json!("Waiting for the run to resume.");return Ok(false);
                    }
                    if transition(update) {
                        let record=s.publication(text(update,"runId")?)?.unwrap_or(Value::Null);
                        if controller(update) {
                            let due=if update["kind"]=="done" {present(&record["merged"])} else {record["pr"]["state"]=="confirmed" && record["pr"]["providerState"]=="OPEN" && !present(&record["merged"])};
                            if !due {block(update,"The publication no longer supports this Jira transition.");return Ok(false);}
                        } else if present(&record["merged"]) {block(update,"Superseded by the merge.");return Ok(false);}
                    }
                }
                if let Some(proof)=proof && proof.check(s).is_err() {
                    block(update,if update["kind"]=="research_outcome" {"The research outcome is no longer complete for current requirements."} else {"The recorded evidence is no longer current."});
                    return Ok(false);
                }
                Ok(true)
            })
        })
    }
    async fn may_send(&self, update: &mut Value) -> Result<bool> {
        if !self.active(update, None)? {
            return Ok(false);
        }
        let verified = match update["kind"].as_str().unwrap_or("") {
            "research_outcome" => Some(self.research_text(text(update, "runId")?)),
            "evidence" | "pr_link" => Some(
                self.evidence_text(
                    text(update, "runId")?,
                    text(update, "candidateId")?,
                    update["kind"] == "pr_link",
                )
                .await,
            ),
            _ => None,
        };
        if let Some(verified) = verified {
            match verified {
                Ok(verified) if update["body"] == verified.body => {
                    return self.active(update, Some(&verified.proof));
                }
                _ => {
                    block(
                        update,
                        if update["kind"] == "research_outcome" {
                            "The research outcome is no longer complete for current requirements."
                        } else {
                            "The recorded evidence is no longer current."
                        },
                    );
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    fn derive_transitions(&self) -> Result<()> {
        self.store.with_store(|s| {
            let source=&self.client.source;
            for record in s.publications()? {
                let run=s.get_run(text(&record,"runId")?)?;
                let task=s.get_task(text(&run,"taskId")?)?;
                if task["source"]!="jira" || task["sourceSite"]!=source["siteUrl"] {continue;}
                let updates=s.jira_updates(Some(text(&run,"id")?))?;
                let intent=|kind:&str,status:&str,transition:&str|->Result<()> {
                    let key=format!("{}-{}",if kind=="done" {"merged"} else {"review"},text(&record,"id")?);
                    if source[status].is_null() || source[transition].is_null() || updates.iter().any(|u|u["eventKey"]==key) {return Ok(());}
                    s.save_jira_update(&json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run["id"],"attemptId":record["attemptId"],"siteUrl":task["sourceSite"],"issueId":task["sourceId"],"eventKey":key,"kind":kind,"statusId":source[status],"transitionId":source[transition],"candidateId":record["candidateId"],"state":"pending","createdAt":time()}))?;
                    Ok(())
                };
                if present(&record["merged"]) {
                    for mut stale in updates.iter().filter(|u| matches!(u["kind"].as_str(),Some("in_review"|"in_progress")) && u["state"]!="confirmed" && !u["reason"].as_str().is_some_and(|r|r.starts_with("Superseded by the merge"))).cloned() {
                        let unknown=matches!(stale["state"].as_str(),Some("uncertain"|"sending"));
                        block(&mut stale,if unknown {"Superseded by the merge; the earlier transition's outcome is unknown."} else {"Superseded by the merge."});
                        s.save_jira_update(&stale)?;
                    }
                    intent("done","doneStatusId","doneTransitionId")?;
                } else if record["pr"]["state"]=="confirmed" && record["pr"]["providerState"]=="OPEN" {intent("in_review","reviewStatusId","reviewTransitionId")?;}
            }
            Ok(())
        })
    }
    pub(super) async fn perform_flush(&self) -> Result<()> {
        self.derive_transitions()?;
        let mut held = HashSet::new();
        for mut update in self.store.with_store(|s| s.jira_updates(None))? {
            let lane = format!(
                "{}:{}",
                text(&update, "runId")?,
                if update["kind"] == "research_outcome" {
                    format!("outcome:{}", text(&update, "eventKey")?)
                } else if controller(&update) {
                    "controller".into()
                } else {
                    "agent".into()
                }
            );
            if update["state"] == "confirmed" || held.contains(&lane) {
                continue;
            }
            let releases = |u: &Value| {
                u["kind"] == "in_review"
                    || u["reason"]
                        .as_str()
                        .is_some_and(|r| r.starts_with("Superseded by the merge"))
            };
            if update["state"] == "blocked" {
                if !releases(&update) {
                    held.insert(lane);
                }
                continue;
            }
            if update["state"] == "sending" {
                update["state"] = json!("uncertain");
            }
            if update["siteUrl"] != self.client.source["siteUrl"] {
                block(&mut update, "The configured Jira site changed.");
                self.store.with_store(|s| s.save_jira_update(&update))?;
                held.insert(lane);
                continue;
            }
            if update["state"] == "pending" && !self.may_send(&mut update).await? {
                self.store.with_store(|s| s.save_jira_update(&update))?;
                if !(update["state"] == "blocked" && releases(&update)) {
                    held.insert(lane);
                }
                continue;
            }
            update["lastAttemptAt"] = json!(time());
            let mut dispatched = false;
            match self.deliver(&mut update, &mut dispatched).await {
                Ok(()) => {
                    if update["state"] == "confirmed" {
                        clear(&mut update, "reason");
                    } else if update["state"] == "uncertain" {
                        update["reason"] = json!(
                            "The previous write outcome is unknown. Waiting for a matching Jira receipt; no duplicate will be sent."
                        );
                    }
                }
                Err(AttemptError::Store(error)) => return Err(error),
                Err(AttemptError::Provider(error)) => {
                    let (rejected,not_sent,rate_limited,reason)=match &error {
                        providers::Error::Unavailable(e)=>(e.status.is_some_and(|s|(400..500).contains(&s)),dispatched && !e.request_sent,e.status==Some(429),e.message.clone()),
                        providers::Error::Invalid(_)=>(false,false,false,"Jira returned an invalid response. The update is retained for reconciliation.".into()),
                    };
                    if not_sent {
                        if update["state"] != "blocked" {
                            update["state"] = json!("pending");
                        }
                    } else if rejected && update["state"] != "uncertain" {
                        update["state"] = json!(if rate_limited { "pending" } else { "blocked" });
                    } else if dispatched {
                        update["state"] = json!("uncertain");
                    }
                    update["reason"] = json!(reason);
                }
            }
            self.store.with_store(|s| s.save_jira_update(&update))?;
            if update["state"] != "confirmed" {
                held.insert(lane);
            }
        }
        Ok(())
    }
    async fn deliver(
        &self,
        update: &mut Value,
        dispatched: &mut bool,
    ) -> std::result::Result<(), AttemptError> {
        let issue = text(update, "issueId")?.to_owned();
        if transition(update) {
            let observed = self.client.issue(&issue).await?;
            if observed["fields"]["status"]["id"] == update["statusId"] {
                update["state"] = json!("confirmed");
                update["externalId"] = update["statusId"].clone();
            } else if update["state"] != "uncertain" {
                let transitions = self.client.transitions(&issue).await?;
                let matching = transitions.iter().find(|t| {
                    t["id"] == update["transitionId"] && t["to"]["id"] == update["statusId"]
                });
                if matching.is_none_or(|t| {
                    t["fields"]
                        .as_object()
                        .is_some_and(|f| f.values().any(|v| v["required"] == true))
                }) {
                    block(
                        update,
                        "The configured Jira transition is unavailable or requires additional fields.",
                    );
                } else if self.may_send(update).await? {
                    self.send(update, dispatched).await?;
                }
            }
        } else {
            let comments = self.client.comments(&issue).await?;
            let existing = comments
                .iter()
                .filter(|c| {
                    crewroom_core::jira::description_text(&c["body"]).contains(&marker(update))
                })
                .collect::<Vec<_>>();
            if !existing.is_empty() {
                if existing.len() == 1
                    && crewroom_core::jira::description_text(&existing[0]["body"])
                        == crewroom_core::jira::description_text(&document(update))
                {
                    update["state"] = json!("confirmed");
                    update["externalId"] = existing[0]["id"].clone();
                } else {
                    block(
                        update,
                        "The remote update marker has conflicting content. Inspect Jira before resolving it.",
                    );
                }
            } else if update["state"] != "uncertain" && self.may_send(update).await? {
                self.send(update, dispatched).await?;
            }
        }
        Ok(())
    }
    async fn send(
        &self,
        update: &mut Value,
        dispatched: &mut bool,
    ) -> std::result::Result<(), AttemptError> {
        update["state"] = json!("sending");
        self.store.with_store(|s| s.save_jira_update(update))?;
        *dispatched = true;
        let shared = Arc::new(Mutex::new(update.clone()));
        let this = self.clone();
        let checked = shared.clone();
        let before: providers::BeforeDispatch = Arc::new(move || {
            let this = this.clone();
            let checked = checked.clone();
            Box::pin(async move {
                let mut update = checked.lock().await;
                if this.may_send(&mut update).await.map_err(|_| {
                    presend("Jira authorization is unavailable. Saved work is retained.")
                })? {
                    Ok(())
                } else {
                    Err(presend("The run stopped before Jira dispatch."))
                }
            })
        });
        let issue = text(update, "issueId")?;
        let result = if transition(update) {
            self.client
                .transition(issue, text(update, "transitionId")?, Some(&before))
                .await
                .map(|_| update["statusId"].clone())
        } else {
            self.client
                .add_comment(issue, document(update), Some(&before))
                .await
                .map(|v| v["id"].clone())
        };
        *update = shared.lock().await.clone();
        let external = result?;
        update["state"] = json!("confirmed");
        update["externalId"] = external;
        Ok(())
    }
}
