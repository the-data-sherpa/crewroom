use super::*;
use crate::{
    providers,
    publication_git::{PublicationGit, Push},
    publication_remote::{Connection, unmatched},
};
use std::{path::PathBuf, sync::Mutex as SyncMutex};
enum Failure {
    Store(Error),
    Provider(providers::Error),
    Held(String),
}
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Store(e)
    }
}
impl From<providers::Error> for Failure {
    fn from(e: providers::Error) -> Self {
        Self::Provider(e)
    }
}
fn operational(e: Error) -> Failure {
    match e {
        Error::Sql(_) => Failure::Store(e),
        _ => Failure::Held(e.to_string()),
    }
}
fn held(message: &str) -> Failure {
    Failure::Held(message.into())
}
fn before_error() -> providers::Error {
    providers::Error::Unavailable(providers::Unavailable {
        message: "Publication authorization changed or became unavailable before dispatch.".into(),
        status: None,
        request_sent: false,
    })
}
impl<S: StoreAccess> Publication<S> {
    pub(super) async fn perform_flush(&self) -> Result<()> {
        let records = self.remote.store(|s| s.publications())?;
        let mut pending = vec![];
        let mut failure = None;
        // Register the complete snapshot before waiting for any provider. In
        // particular, Stop on a queued second run fences its original ticket.
        for record in records {
            let run = text(&record, "runId")?.to_owned();
            let this = self.clone();
            match self.remote.register(
                &run.clone(),
                "publication-flush",
                move |ticket| async move {
                    let _lock = this.mutation.lock().await;
                    ticket.check()?;
                    this.flush_record(&run, &ticket).await
                },
            ) {
                Ok(receiver) => pending.push(receiver),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        for result in futures_util::future::join_all(pending).await {
            let result = result
                .map_err(|_| {
                    conflict("Publication worker was interrupted. Reconcile saved intents.")
                })
                .and_then(|v| v);
            if let Err(e) = result {
                failure.get_or_insert(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    async fn flush_record(&self, run: &str, ticket: &Ticket) -> Result<()> {
        let Some(mut record) = self.remote.store(|s| s.publication(run))? else {
            return Ok(());
        };
        if present(&record["merged"])
            || self
                .remote
                .store(|s| s.publication_superseded(run, text(&record, "id")?))?
        {
            return Ok(());
        }
        let Some(connection) = self.remote.matched(&record) else {
            record["error"] = json!(unmatched(&record));
            record["ready"] = json!(false);
            return self.save(&record);
        };
        record["lastAttemptAt"] = json!(time());
        if record["push"]["state"] != "blocked" && record["pr"]["state"] != "blocked" {
            remove(&mut record, "error");
        }
        for key in ["push", "pr"] {
            if record[key]["state"] == "sending" {
                record[key]["state"] = json!("uncertain");
            }
        }
        let target = text(&record, "target")?.to_owned();
        let current = &mut record;
        let connected = &connection;
        let result = self
            .remote
            .git(ticket, run, "", &target, &connection, |git| async move {
                Ok(self.deliver(current, ticket, connected, &git).await)
            })
            .await;
        let mut cleanup = None;
        let failure = match result {
            Ok(Ok(())) => None,
            Ok(Err(Failure::Store(e))) => return Err(e),
            Ok(Err(e)) => Some(e),
            Err(e) => {
                cleanup = Some(e);
                Some(held(
                    "Publication transport cleanup is unverified. Retain intents for reconciliation.",
                ))
            }
        };
        if let Some(error) = failure {
            if record["push"]["state"] == "sending" {
                record["push"]["state"] = json!("uncertain");
            }
            if record["pr"]["state"] == "sending" {
                let state = match &error {
                    Failure::Provider(providers::Error::Unavailable(e))
                        if !e.request_sent || e.status == Some(429) =>
                    {
                        "pending"
                    }
                    Failure::Provider(providers::Error::Unavailable(e))
                        if e.status.is_some_and(|s| (400..500).contains(&s)) =>
                    {
                        "blocked"
                    }
                    _ => "uncertain",
                };
                record["pr"]["state"] = json!(state);
            }
            record["error"] = json!(match error {
                Failure::Held(s) => s,
                Failure::Provider(providers::Error::Unavailable(e)) => e.message,
                _ => "Publication is unavailable. Reconcile retained intents before another write."
                    .into(),
            });
        }
        let local = self.local(run).await;
        let source = self
            .remote
            .store(|s| Ok(present(&s.get_run(run)?["sourceConflict"])))?;
        let assessed = assess(&record, local.as_ref(), source);
        record
            .as_object_mut()
            .unwrap()
            .extend(assessed.as_object().unwrap().clone());
        self.save(&record)?;
        if record["pr"]["state"] != "pending"
            || matches!(
                record["push"]["state"].as_str(),
                Some("blocked" | "uncertain")
            )
        {
            let mut notice = json!({"kind":"publication","subjectId":record["id"],"candidateId":record["candidateId"],"pushState":record["push"]["state"],"prState":record["pr"]["state"],"ready":record["ready"],"blockers":record["blockers"]});
            if let Some(url) = record["pr"].get("url") {
                notice["prUrl"] = url.clone();
            }
            self.remote.store(|s| s.notice_delivery(run, &notice))?;
        }
        cleanup.map_or(Ok(()), Err)
    }
    fn confirm_pr(&self, record: &mut Value, pr: &Value) -> Result<()> {
        if !identity(record, pr) {
            return Err(conflict(
                "The PR does not match the durable publication identity.",
            ));
        }
        record["pr"] = json!({"state":"confirmed","id":pr["id"],"url":pr["links"]["html"]["href"],"describedSha256":digest(text(pr,"description")?),"providerState":pr["state"]});
        description_receipt(record, pr);
        self.save(record)
    }
    async fn deliver(
        &self,
        record: &mut Value,
        ticket: &Ticket,
        connection: &Connection,
        git: &PublicationGit,
    ) -> std::result::Result<(), Failure> {
        let client = &connection.host;
        let run = text(record, "runId")?.to_owned();
        ticket.check().map_err(operational)?;
        client.verify_repository().await?;
        ticket.check().map_err(operational)?;
        set_optional(
            record,
            "remoteTargetHead",
            git.reference(text(record, "target")?)
                .await
                .map_err(operational)?,
        );
        ticket.check().map_err(operational)?;
        set_optional(
            record,
            "remoteSourceHead",
            git.reference(text(record, "branch")?)
                .await
                .map_err(operational)?,
        );
        if !matches!(
            record["push"]["state"].as_str(),
            Some("confirmed" | "blocked")
        ) {
            if record["remoteSourceHead"] == record["revision"]["head"] {
                record["push"]["state"] = json!("confirmed");
                self.save(record)?;
            } else if record["remoteSourceHead"] != record["push"]["expectedHead"] {
                record["push"]["state"] = json!("blocked");
                return Err(held(
                    "Remote branch changed outside this publication. No overwrite was attempted.",
                ));
            } else {
                if record["pr"]["state"] == "confirmed" {
                    ticket.check().map_err(operational)?;
                    let current = client.pull_request(pr_id(record)?).await?;
                    record["pr"]["providerState"] = current["state"].clone();
                    self.save(record)?;
                    if current["state"] != "OPEN"
                        || !identity(record, &current)
                        || current["source"]["commit"]["hash"] != record["push"]["expectedHead"]
                        || current["destination"]["commit"]["hash"] != record["revision"]["base"]
                    {
                        record["push"]["state"] = json!("blocked");
                        return Err(held(
                            "The existing PR closed, changed head/base, or lost its publication identity. No push was attempted.",
                        ));
                    }
                }
                self.work.status(&run).await.map_err(operational)?;
                let proof = self.may_write(record, ticket).await.map_err(operational)?;
                if record["remoteTargetHead"] != record["revision"]["base"] {
                    return Err(held(
                        "Remote target changed. Refresh and validate the base before publication.",
                    ));
                }
                record["push"]["state"] = json!("sending");
                self.save(record)?;
                let push = Push {
                    directory: PathBuf::from(text(&proof.facts["work"], "path")?),
                    head: text(&record["revision"], "head")?.into(),
                    branch: text(record, "branch")?.into(),
                    expected_head: record["push"]["expectedHead"].as_str().map(String::from),
                };
                let this = self.clone();
                let guarded = record.clone();
                let dispatch_ticket = ticket.clone();
                git.push(push, move |launch| {
                    dispatch_ticket.with_current(|| {
                        this.flight.with_open(|| {
                            this.remote.store(|s| {
                                Self::writable(s, &guarded)?;
                                proof.check(s)?;
                                launch()
                            })
                        })
                    })
                })
                .await
                .map_err(operational)?;
                ticket.check().map_err(operational)?;
                set_optional(
                    record,
                    "remoteSourceHead",
                    git.reference(text(record, "branch")?)
                        .await
                        .map_err(operational)?,
                );
                if record["remoteSourceHead"] != record["revision"]["head"] {
                    return Err(held(
                        "Remote branch did not confirm the publication revision.",
                    ));
                }
                record["push"]["state"] = json!("confirmed");
                self.save(record)?;
            }
        }
        if record["push"]["state"] != "confirmed" {
            return Err(held(
                record["error"]
                    .as_str()
                    .unwrap_or("Git publication remains blocked."),
            ));
        }
        if !matches!(
            record["pr"]["state"].as_str(),
            Some("confirmed" | "blocked")
        ) {
            ticket.check().map_err(operational)?;
            let existing = client
                .pull_requests(text(record, "branch")?, text(record, "target")?)
                .await?;
            let matches = existing
                .iter()
                .filter(|p| {
                    p["description"]
                        .as_str()
                        .is_some_and(|s| s.contains(&marker(record)))
                })
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(held(
                    "Multiple PRs claim this publication identity. Inspect the provider before continuing.",
                ));
            }
            if let Some(pr) = matches.first() {
                self.confirm_pr(record, pr).map_err(operational)?;
            } else if record["pr"]["state"] == "uncertain" {
                return Err(held(
                    "PR creation outcome is unknown. No matching receipt was found; no duplicate will be created.",
                ));
            } else if existing.iter().any(|p| p["state"] == "OPEN") {
                record["pr"]["state"] = json!("blocked");
                return Err(held(
                    "An existing PR uses this branch without the recorded publication identity.",
                ));
            } else {
                self.may_write(record, ticket).await.map_err(operational)?;
                if record["remoteSourceHead"] != record["revision"]["head"] {
                    return Err(held(
                        "Remote source changed after publication. Reconcile the branch before PR creation.",
                    ));
                }
                if record["remoteTargetHead"] != record["revision"]["base"] {
                    return Err(held(
                        "Remote target changed. Refresh and validate before PR creation.",
                    ));
                }
                record["pr"]["state"] = json!("sending");
                self.save(record)?;
                let pr = self.write_pr(record, ticket, connection, false).await?;
                self.confirm_pr(record, &pr).map_err(operational)?;
            }
        }
        if record["pr"]["state"] == "confirmed"
            && record["pr"]["describedSha256"] != digest(text(record, "description")?)
        {
            ticket.check().map_err(operational)?;
            let current = client.pull_request(pr_id(record)?).await?;
            record["pr"]["providerState"] = current["state"].clone();
            self.save(record)?;
            if !identity(record, &current) {
                return Err(held(
                    "The PR does not match the durable publication identity.",
                ));
            }
            if current["description"] == record["description"]
                && current["title"] == record["title"]
            {
                record["pr"]["describedSha256"] = json!(digest(text(record, "description")?));
                self.save(record)?;
            } else {
                if current["state"] != "OPEN"
                    || current["source"]["commit"]["hash"] != record["revision"]["head"]
                    || current["destination"]["commit"]["hash"] != record["revision"]["base"]
                {
                    return Err(held(
                        "The existing PR closed or changed identity before its description update. No PR write was attempted.",
                    ));
                }
                self.may_write(record, ticket).await.map_err(operational)?;
                let pr = self.write_pr(record, ticket, connection, true).await?;
                if !identity(record, &pr)
                    || pr["state"] != "OPEN"
                    || pr["description"] != record["description"]
                    || pr["title"] != record["title"]
                    || pr["source"]["commit"]["hash"] != record["revision"]["head"]
                    || pr["destination"]["commit"]["hash"] != record["revision"]["base"]
                {
                    return Err(held(
                        "The PR description update was not confirmed for this revision.",
                    ));
                }
                record["pr"]["describedSha256"] = json!(digest(text(record, "description")?));
                record["pr"]["providerState"] = pr["state"].clone();
                self.save(record)?;
            }
        }
        if record["pr"]["state"] == "confirmed" {
            ticket.check().map_err(operational)?;
            let current = client.pull_request(pr_id(record)?).await?;
            if !identity(record, &current) {
                return Err(held(
                    "The PR does not match the durable publication identity.",
                ));
            }
            record["pr"]["providerState"] = current["state"].clone();
            description_receipt(record, &current);
            if current["state"] == "MERGED"
                && present(&current["merge_commit"])
                && current["source"]["commit"]["hash"] == record["revision"]["head"]
            {
                record["merged"] =
                    json!({"commit":current["merge_commit"]["hash"],"observedAt":time()});
            }
            self.save(record)?;
            if !present(&record["merged"]) {
                ticket.check().map_err(operational)?;
                let observation = client.observe(pr_id(record)?).await?;
                if present(&record["observation"])
                    && policy::regressed(&observation, &record["observation"])
                {
                    return Err(held(
                        "Bitbucket returned older observations. Retained evidence was not replaced.",
                    ));
                }
                if !identity(record, &observation["pr"]) {
                    return Err(held(
                        "The PR lost its durable publication identity during hosted observation.",
                    ));
                }
                description_receipt(record, &observation["pr"]);
                record["pr"]["providerState"] = observation["pr"]["state"].clone();
                record["observation"] = observation;
            }
        }
        Ok(())
    }
    async fn write_pr(
        &self,
        record: &Value,
        ticket: &Ticket,
        connection: &Connection,
        update: bool,
    ) -> std::result::Result<Value, Failure> {
        let this = self.clone();
        let check = record.clone();
        let ticket = ticket.clone();
        let failure = Arc::new(SyncMutex::new(None));
        let callback_failure = failure.clone();
        let before: providers::BeforeDispatch = Arc::new(move || {
            let this = this.clone();
            let record = check.clone();
            let ticket = ticket.clone();
            let failure = callback_failure.clone();
            Box::pin(async move {
                if let Err(e) = this.may_write(&record, &ticket).await {
                    // Preserve persistence failure rather than reclassifying it
                    // as a retryable provider rejection.
                    if matches!(e, Error::Sql(_)) {
                        *failure.lock().unwrap() = Some(e);
                    }
                    return Err(before_error());
                }
                Ok(())
            })
        });
        let result = if update {
            connection
                .host
                .update_pull_request(pr_id(record)?, record, Some(&before))
                .await
        } else {
            connection.host.create_pull_request(&json!({"title":record["title"],"description":record["description"],"branch":record["branch"],"target":record["target"],"head":record["revision"]["head"]}),Some(&before)).await
        };
        if let Some(e) = failure.lock().unwrap().take() {
            return Err(Failure::Store(e));
        }
        Ok(result?)
    }
}
fn pr_id(record: &Value) -> Result<i64> {
    record["pr"]["id"]
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or_else(|| conflict("The confirmed PR identity is missing."))
}
fn set_optional(record: &mut Value, key: &str, value: Option<String>) {
    match value {
        Some(value) => record[key] = json!(value),
        None => remove(record, key),
    }
}

fn description_receipt(record: &mut Value, pr: &Value) {
    if pr["title"] == record["title"] {
        record["pr"]["describedSha256"] = json!(digest(pr["description"].as_str().unwrap_or("")));
    } else {
        remove(&mut record["pr"], "describedSha256");
    }
}
