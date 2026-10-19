use super::*;
use crate::providers::github_issue_client::GitHubIssueClient;
use std::collections::{HashMap, HashSet};
use tokio::sync::Mutex;

enum AttemptError {
    Store(crewroom_core::Error),
    Provider(providers::Error),
}
impl From<crewroom_core::Error> for AttemptError {
    fn from(e: crewroom_core::Error) -> Self {
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
/// Prefix of the reason a merged issue keeps while GitHub has not closed it.
const WAITING_CLOSE: &str = "Waiting for GitHub to close";
/// Milliseconds between unforced re-reads of an issue that is waiting to close.
const CLOSE_RECHECK_MS: i64 = 60_000;

fn labels(update: &Value, key: &str) -> Vec<String> {
    update["labels"][key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
fn has(labels: &[String], label: &str) -> bool {
    labels.iter().any(|l| l.eq_ignore_ascii_case(label))
}

impl<S: StoreAccess> GitHubDelivery<S> {
    /// Holds the short flight/store guards through the final durable check.
    fn active(&self, update: &mut Value, proof: Option<&Proof>) -> Result<bool> {
        self.flight.with_status(|closed| {
            if closed {
                update["reason"] = json!("Waiting for the server to resume.");
                return Ok(false);
            }
            self.store.with_store(|s| {
                let run = s.get_run(text(update, "runId")?)?;
                if update["kind"] == "research_outcome" {
                    if run["researchCompletion"]["revision"] == run["taskRevision"]
                        && !present(&run["sourceConflict"])
                        && matches!(run["status"].as_str(), Some("running" | "pausing" | "paused"))
                    {
                        update["reason"] = json!("Waiting for confirmed research completion.");
                        return Ok(false);
                    }
                    if run["status"] != "completed"
                        || run["researchOutcome"]["revision"] != run["taskRevision"]
                        || present(&run["sourceConflict"])
                    {
                        block(update, "The research outcome is no longer complete for current requirements.");
                        return Ok(false);
                    }
                } else {
                    if present(&run["sourceConflict"]) || run["desiredState"] == "cancelled" {
                        block(update, "Run requirements changed or the run was cancelled. This update will not be sent.");
                        return Ok(false);
                    }
                    if !controller(update)
                        && (run["status"] != "running" || run["desiredState"] != "running")
                    {
                        update["reason"] = json!("Waiting for the run to resume.");
                        return Ok(false);
                    }
                    if labelled(update) {
                        let record = s.publication(text(update, "runId")?)?.unwrap_or(Value::Null);
                        if controller(update) {
                            let due = if update["kind"] == "done" {
                                present(&record["merged"])
                            } else {
                                record["pr"]["state"] == "confirmed"
                                    && record["pr"]["providerState"] == "OPEN"
                                    && !present(&record["merged"])
                            };
                            if !due {
                                block(update, "The publication no longer supports this GitHub label change.");
                                return Ok(false);
                            }
                        } else if present(&record["merged"]) {
                            block(update, "Superseded by the merge.");
                            return Ok(false);
                        }
                    }
                }
                if let Some(proof) = proof
                    && proof.check(s).is_err()
                {
                    block(update, if update["kind"] == "research_outcome" {
                        "The research outcome is no longer complete for current requirements."
                    } else {
                        "The recorded evidence is no longer current."
                    });
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

    pub(super) async fn perform_flush(&self) -> Result<()> {
        let force = self.force.swap(false, Ordering::SeqCst);
        self.derive()?;
        let current = self.store.with_store(sites)?;
        let mut clients: HashMap<String, GitHubIssueClient> = HashMap::new();
        let mut held = HashSet::new();
        for mut update in self.store.with_store(|s| s.github_updates(None))? {
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
            if current.is_empty() {
                update["reason"] = json!(
                    "No project has GitHub Issues as its ticket source. Confirm it for the project under Settings → Projects; the update is kept."
                );
                self.store.with_store(|s| s.save_github_update(&update))?;
                held.insert(lane);
                continue;
            }
            if !current.contains(&update["siteUrl"]) {
                block(&mut update, "The configured GitHub repository changed.");
                self.store.with_store(|s| s.save_github_update(&update))?;
                held.insert(lane);
                continue;
            }
            let waiting = update["kind"] == "done"
                && update["reason"]
                    .as_str()
                    .is_some_and(|r| r.starts_with(WAITING_CLOSE));
            let recent = update["lastAttemptAt"]
                .as_str()
                .and_then(crewroom_core::source_time::parse)
                .is_some_and(|at| at.saturating_add(CLOSE_RECHECK_MS) > crewroom_core::now_ms());
            if waiting && recent && !force {
                held.insert(lane);
                continue;
            }
            if update["state"] == "pending" && !self.may_send(&mut update).await? {
                self.store.with_store(|s| s.save_github_update(&update))?;
                if !(update["state"] == "blocked" && releases(&update)) {
                    held.insert(lane);
                }
                continue;
            }
            update["lastAttemptAt"] = json!(time());
            let repository = text(&update, "repository")?.to_owned();
            let client = match clients.get(&repository) {
                Some(client) => client.clone(),
                None => match GitHubIssueClient::new(
                    &repository,
                    self.credentials.clone(),
                    self.fixture.as_deref(),
                ) {
                    Ok(client) => {
                        clients.insert(repository.clone(), client.clone());
                        client
                    }
                    Err(error) => {
                        block(&mut update, &error.to_string());
                        self.store.with_store(|s| s.save_github_update(&update))?;
                        held.insert(lane);
                        continue;
                    }
                },
            };
            let mut dispatched = false;
            match self.deliver(&client, &mut update, &mut dispatched).await {
                Ok(()) => {
                    if update["state"] == "confirmed" {
                        clear(&mut update, "reason");
                    } else if update["state"] == "uncertain" {
                        update["reason"] = json!(
                            "The previous write outcome is unknown. Waiting for a matching GitHub comment; no duplicate will be sent. Retry only after checking the issue."
                        );
                    }
                }
                Err(AttemptError::Store(error)) => return Err(error),
                Err(AttemptError::Provider(error)) => {
                    let (rejected, not_sent, rate_limited, reason) = match &error {
                        providers::Error::Unavailable(e) => (
                            e.status.is_some_and(|s| (400..500).contains(&s)),
                            dispatched && !e.request_sent,
                            e.status == Some(429),
                            e.message.clone(),
                        ),
                        providers::Error::Invalid(_) => (
                            false,
                            false,
                            false,
                            "GitHub returned an invalid response. The update is retained for reconciliation.".into(),
                        ),
                    };
                    if not_sent {
                        if update["state"] != "blocked" {
                            update["state"] = json!("pending");
                        }
                    } else if rejected && (update["state"] != "uncertain" || labelled(&update)) {
                        update["state"] = json!(if rate_limited { "pending" } else { "blocked" });
                    } else if dispatched {
                        // A label write is idempotent and retried; a comment
                        // write with an unknown outcome is not repeated.
                        update["state"] = json!(if labelled(&update) {
                            "pending"
                        } else {
                            "uncertain"
                        });
                    }
                    update["reason"] = json!(reason);
                }
            }
            self.store.with_store(|s| s.save_github_update(&update))?;
            if update["state"] != "confirmed" {
                held.insert(lane);
            }
        }
        Ok(())
    }

    async fn deliver(
        &self,
        client: &GitHubIssueClient,
        update: &mut Value,
        dispatched: &mut bool,
    ) -> std::result::Result<(), AttemptError> {
        let number = update["issueNumber"].as_u64().ok_or_else(|| {
            crewroom_core::Error::Invalid("GitHub update has no issue number.".into())
        })?;
        if labelled(update) {
            let issue = client.issue(number).await?;
            let add: Vec<_> = labels(update, "add")
                .into_iter()
                .filter(|l| !has(&issue.labels, l))
                .collect();
            let remove: Vec<_> = labels(update, "remove")
                .into_iter()
                .filter(|l| has(&issue.labels, l))
                .collect();
            if !add.is_empty() || !remove.is_empty() {
                // Label writes are idempotent, so an uncertain one is re-sent.
                if !self.may_send(update).await? {
                    return Ok(());
                }
                self.send(
                    update,
                    dispatched,
                    |client, before| {
                        let (add, remove) = (add.clone(), remove.clone());
                        Box::pin(async move {
                            for label in &add {
                                client.add_label(number, label, Some(&before)).await?;
                            }
                            for label in &remove {
                                client.remove_label(number, label, Some(&before)).await?;
                            }
                            Ok(json!(null))
                        })
                    },
                    client,
                )
                .await?;
            }
            if update["kind"] == "done" {
                return self.observe_closure(client, update, number).await;
            }
            let after = if add.is_empty() && remove.is_empty() {
                issue.labels
            } else {
                client.issue(number).await?.labels
            };
            update["state"] = json!("confirmed");
            update["externalId"] = json!(after);
            return Ok(());
        }
        let comments = client.comments(number).await?;
        let mark = marker(update);
        let existing: Vec<_> = comments
            .iter()
            .filter(|c| c["body"].as_str().is_some_and(|b| b.contains(&mark)))
            .collect();
        if !existing.is_empty() {
            if existing.len() == 1
                && normalized(existing[0]["body"].as_str().unwrap_or(""))
                    == normalized(&document(update))
            {
                update["state"] = json!("confirmed");
                update["externalId"] = existing[0]["id"].clone();
            } else {
                block(
                    update,
                    "The remote update marker has conflicting content. Inspect the GitHub issue before resolving it.",
                );
            }
        } else if update["state"] != "uncertain" && self.may_send(update).await? {
            let body = document(update);
            self.send(
                update,
                dispatched,
                |client, before| {
                    let body = body.clone();
                    Box::pin(async move { client.add_comment(number, &body, Some(&before)).await })
                },
                client,
            )
            .await?;
        }
        Ok(())
    }

    /// After a merge: GitHub closes the issue through the PR's closing
    /// reference. Crewroom only reads and records the state GitHub reports.
    async fn observe_closure(
        &self,
        client: &GitHubIssueClient,
        update: &mut Value,
        number: u64,
    ) -> std::result::Result<(), AttemptError> {
        let issue = client.issue(number).await?;
        let task = text(update, "taskId")?.to_owned();
        self.store.with_store(|s| {
            s.record_github_issue_state(&task, &issue.state, issue.updated_at.as_deref())
        })?;
        update["issueState"] = json!(issue.state);
        update["observedAt"] = json!(time());
        if issue.state == "closed" {
            update["state"] = json!("confirmed");
            update["externalId"] = json!("closed");
        } else {
            update["state"] = json!("pending");
            update["reason"] = json!(if update["closesOnMerge"] == true {
                format!(
                    "{WAITING_CLOSE} #{number}. GitHub closes it when the merged PR's closing reference reaches the default branch; Crewroom does not close it."
                )
            } else {
                format!(
                    "{WAITING_CLOSE} #{number}. The PR is not on GitHub, so its merge does not close the issue; close it on GitHub when appropriate."
                )
            });
        }
        Ok(())
    }

    /// Persists `sending` before dispatch and re-checks authorization just
    /// before the request leaves; the provider receipt is saved afterwards.
    async fn send<F>(
        &self,
        update: &mut Value,
        dispatched: &mut bool,
        write: F,
        client: &GitHubIssueClient,
    ) -> std::result::Result<(), AttemptError>
    where
        F: FnOnce(
            GitHubIssueClient,
            providers::BeforeDispatch,
        ) -> futures_util::future::BoxFuture<'static, providers::Result<Value>>,
    {
        update["state"] = json!("sending");
        self.store.with_store(|s| s.save_github_update(update))?;
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
                    presend("GitHub authorization is unavailable. Saved work is retained.")
                })? {
                    Ok(())
                } else {
                    Err(presend("The run stopped before GitHub dispatch."))
                }
            })
        });
        let result = write(client.clone(), before).await;
        *update = shared.lock().await.clone();
        let external = result?;
        if labelled(update) {
            // Label receipts are confirmed by the re-read that follows.
            update["state"] = json!("pending");
        } else {
            update["state"] = json!("confirmed");
            update["externalId"] = external;
        }
        Ok(())
    }
}
