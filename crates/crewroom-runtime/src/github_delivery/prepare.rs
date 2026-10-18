use super::*;
use crate::{providers::github_pulls, repository::array};

impl<S: StoreAccess> GitHubDelivery<S> {
    /// Only the active delivery reporter (or a research coordinator without
    /// one, for its outcome) may queue updates, and only for a ticket imported
    /// from the saved GitHub repository.
    fn authorize(
        &self,
        s: &Store,
        attempt_id: &str,
        value: &Value,
    ) -> Result<(Value, Value, Source, Option<Value>)> {
        let attempt = s.get_attempt(attempt_id)?;
        let run = s.get_run(text(&attempt, "runId")?)?;
        let runtime = s.runtime(text(&run, "id")?)?;
        let assignment = array(&runtime["assignments"])
            .iter()
            .find(|a| a["currentAttemptId"] == attempt_id);
        let reporter = assignment
            .is_some_and(|a| a["coordinator"] != true && a["role"]["id"] == "delivery-reporter");
        let fallback = assignment.is_some_and(|a| a["coordinator"] == true)
            && value["kind"] == "research_outcome"
            && run["purpose"] == "research"
            && !array(&runtime["assignments"])
                .iter()
                .any(|a| !present(&a["retiredAt"]) && a["role"]["id"] == "delivery-reporter");
        if assignment.is_none_or(|a| present(&a["retiredAt"]))
            || (!reporter && !fallback)
            || attempt["ownership"] != "held"
            || !matches!(attempt["state"].as_str(), Some("idle" | "executing"))
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || present(&run["sourceConflict"])
        {
            return Err(conflict(
                "Only the active delivery reporter, or a research coordinator without one, may request GitHub updates.",
            ));
        }
        let task = s.get_task(text(&run, "taskId")?)?;
        let source = source(s)?
            .filter(|source| ticket_source::belongs_to(&task, ticket_source::GITHUB, &source.site));
        let Some(source) = source else {
            return Err(conflict(
                "This run does not belong to the configured GitHub repository.",
            ));
        };
        if issue_number(&task).is_none() {
            return Err(conflict("This ticket has no GitHub issue number."));
        }
        let prior = s
            .github_updates(Some(text(&run, "id")?))?
            .into_iter()
            .find(|u| u["eventKey"] == value["eventKey"]);
        if prior.as_ref().is_some_and(|p| {
            p["kind"] != value["kind"]
                || value
                    .get("candidateId")
                    .is_some_and(|v| p["candidateId"] != *v)
                || value.get("body").is_some_and(|v| p["body"] != *v)
        }) {
            return Err(conflict("GitHub event key has different content."));
        }
        if run["purpose"] == "research"
            && !matches!(
                value["kind"].as_str(),
                Some("milestone" | "blocker" | "research_outcome")
            )
        {
            return Err(conflict(
                "Research does not report coding evidence or GitHub workflow labels.",
            ));
        }
        if value["kind"] == "research_outcome"
            && (run["purpose"] != "research"
                || value["eventKey"] != format!("research-outcome-{}", run["taskRevision"]))
        {
            return Err(conflict(
                "Research outcome event key must identify the accepted revision.",
            ));
        }
        Ok((run, task, source, prior))
    }

    /// Queues a reporter update. The same event key returns the earlier
    /// intent; different content under it is refused.
    pub async fn prepare(&self, attempt_id: &str, input: &Value) -> Result<Value> {
        let value = crate::delivery_evidence::report(input, PROVIDER)?;
        let (first, _, _, prior) = self.flight.with_open(|| {
            self.store
                .with_store(|s| self.authorize(s, attempt_id, &value))
        })?;
        if let Some(prior) = prior {
            return Ok(prior);
        }
        let verified = match value["kind"].as_str().unwrap_or("") {
            "evidence" | "pr_link" => Some(
                self.evidence_text(
                    text(&first, "id")?,
                    text(&value, "candidateId")?,
                    value["kind"] == "pr_link",
                )
                .await?,
            ),
            "research_outcome" => Some(self.research_text(text(&first, "id")?)?),
            _ => None,
        };
        self.flight.with_open(|| {
            self.store.with_store(|s| {
                let (run, task, source, prior) = self.authorize(s, attempt_id, &value)?;
                if let Some(prior) = prior {
                    return Ok(prior);
                }
                if let Some(v) = &verified {
                    v.proof.check(s)?;
                }
                let mut update = value.clone();
                for (key, value) in json!({
                    "id": uuid::Uuid::new_v4().to_string(),
                    "runId": run["id"],
                    "attemptId": attempt_id,
                    "taskId": task["id"],
                    "siteUrl": task["sourceSite"],
                    "repository": source.repository,
                    "issueKey": task["key"],
                    "issueNumber": issue_number(&task),
                    "state": "pending",
                    "createdAt": time(),
                })
                .as_object()
                .unwrap()
                {
                    update[key] = value.clone();
                }
                if value["kind"] == "in_progress" {
                    let Some(label) = source.labels["inProgress"].as_str() else {
                        return Err(conflict(
                            "GitHub workflow labels are off. Name an in-progress label under Settings → Integrations → GitHub Issues to use it.",
                        ));
                    };
                    update["labels"] = json!({"add":[label],"remove":[]});
                }
                if let Some(verified) = verified {
                    update["body"] = json!(verified.body);
                }
                s.save_github_update(&update)
            })
        })
    }

    pub(super) async fn evidence_text(
        &self,
        run_id: &str,
        candidate_id: &str,
        publication: bool,
    ) -> Result<VerifiedText> {
        let mut verified = crate::delivery_evidence::evidence_text(
            &self.store,
            self.evidence.as_ref(),
            PROVIDER,
            run_id,
            candidate_id,
            publication,
        )
        .await?;
        if publication {
            // The PR description carries the closing reference only when the
            // PR is on GitHub (FOR-46); say which applies.
            let on_github = self.store.with_store(|s| {
                Ok(s.publication(run_id)?
                    .is_some_and(|p| github_pulls::is_github(&p["repository"])))
            })?;
            verified.body.push_str(if on_github {
                "\nThe pull request description closes this issue when the PR merges into the default branch."
            } else {
                "\nThis pull request is not on GitHub, so merging it does not close this issue."
            });
        }
        Ok(verified)
    }
    pub(super) fn research_text(&self, run_id: &str) -> Result<VerifiedText> {
        crate::delivery_evidence::research_text(
            &self.store,
            self.research.as_ref(),
            PROVIDER,
            run_id,
        )
    }

    /// Controller-derived updates: the configured in-review label once the PR
    /// is confirmed open, and after a merge the label cleanup plus the issue's
    /// closed state. Nothing is derived for labels the operator left off.
    pub(super) fn derive(&self) -> Result<()> {
        self.store.with_store(|s| {
            let Some(source) = source(s)? else {
                return Ok(());
            };
            let configured = |key: &str| source.labels[key].as_str().map(str::to_owned);
            let (progress, review) = (configured("inProgress"), configured("inReview"));
            for record in s.publications()? {
                let run = s.get_run(text(&record, "runId")?)?;
                let task = s.get_task(text(&run, "taskId")?)?;
                if !ticket_source::belongs_to(&task, ticket_source::GITHUB, &source.site) {
                    continue;
                }
                let Some(number) = issue_number(&task) else {
                    continue;
                };
                let updates = s.github_updates(Some(text(&run, "id")?))?;
                let intent = |kind: &str, labels: Value| -> Result<()> {
                    let key = format!(
                        "{}-{}",
                        if kind == "done" { "merged" } else { "review" },
                        text(&record, "id")?
                    );
                    if updates.iter().any(|u| u["eventKey"] == key) {
                        return Ok(());
                    }
                    let mut update = json!({
                        "id": uuid::Uuid::new_v4().to_string(),
                        "runId": run["id"],
                        "attemptId": record["attemptId"],
                        "taskId": task["id"],
                        "siteUrl": task["sourceSite"],
                        "repository": source.repository,
                        "issueKey": task["key"],
                        "issueNumber": number,
                        "eventKey": key,
                        "kind": kind,
                        "labels": labels,
                        "candidateId": record["candidateId"],
                        "state": "pending",
                        "createdAt": time(),
                    });
                    if kind == "done" {
                        update["closesOnMerge"] =
                            json!(github_pulls::is_github(&record["repository"]));
                    }
                    s.save_github_update(&update)?;
                    Ok(())
                };
                if present(&record["merged"]) {
                    for mut stale in updates
                        .iter()
                        .filter(|u| {
                            matches!(u["kind"].as_str(), Some("in_review" | "in_progress"))
                                && u["state"] != "confirmed"
                                && !u["reason"]
                                    .as_str()
                                    .is_some_and(|r| r.starts_with("Superseded by the merge"))
                        })
                        .cloned()
                    {
                        block(&mut stale, "Superseded by the merge.");
                        s.save_github_update(&stale)?;
                    }
                    let remove: Vec<_> = [&progress, &review].into_iter().flatten().collect();
                    intent("done", json!({"add":[],"remove":remove}))?;
                } else if record["pr"]["state"] == "confirmed"
                    && record["pr"]["providerState"] == "OPEN"
                    && let Some(review) = &review
                {
                    let remove: Vec<_> = progress.iter().collect();
                    intent("in_review", json!({"add":[review],"remove":remove}))?;
                }
            }
            Ok(())
        })
    }
}
