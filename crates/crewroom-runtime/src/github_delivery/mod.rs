//! GitHub issue delivery (FOR-48): the GitHub counterpart of Jira delivery for
//! tickets imported from GitHub Issues.
//!
//! The delivery reporter's milestone, blocker, evidence, PR-link and research
//! outcome updates become issue comments. Each comment carries a stable
//! `Crewroom update <id>` marker, so a retry or restart re-reads the issue's
//! comments and confirms an earlier write instead of posting it twice; a write
//! whose outcome is unknown stays uncertain until a matching comment is found
//! or the operator explicitly retries it.
//!
//! GitHub has only open/closed. The issue closes through the `Closes #N`
//! reference FOR-46 puts in the PR description when that PR merges; Crewroom
//! never closes it. After a merge Crewroom reads the issue and records the
//! state GitHub reports as the ticket's source status, which stays separate
//! from Crewroom workflow state and agent activity. Optional workflow labels
//! (in progress, in review) are applied only when the operator names them.
//!
//! Durable intent, provider receipt and the issue's own state are distinct
//! facts. Pending, sending, uncertain and blocked updates are undelivered and
//! keep their reason; the token is read per request and never stored here.
mod dispatch;
mod prepare;
use crate::{
    conflict,
    delivery_evidence::{Proof, VerifiedText, present},
    delivery_flight::DeliveryFlight,
    evidence::{Evidence, Research},
    messaging::StoreAccess,
    providers::{self, Credentials},
    repository::text,
};
use crewroom_core::{Result, store::Store, ticket_source};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Provider name used in refusals and evidence binding.
const PROVIDER: &str = "GitHub";

fn time() -> String {
    crewroom_core::timestamp(crewroom_core::now_ms())
}
/// Updates Crewroom derives from the publication, not from the reporter.
fn controller(update: &Value) -> bool {
    matches!(update["kind"].as_str(), Some("in_review" | "done"))
}
/// Updates that change labels or observe the issue instead of commenting.
fn labelled(update: &Value) -> bool {
    update["kind"] == "in_progress" || controller(update)
}
fn clear(value: &mut Value, key: &str) {
    if let Some(fields) = value.as_object_mut() {
        fields.remove(key);
    }
}
fn block(update: &mut Value, reason: &str) {
    update["state"] = json!("blocked");
    update["reason"] = json!(reason);
}
fn marker(update: &Value) -> String {
    format!("Crewroom update {}", update["id"].as_str().unwrap_or(""))
}
/// The exact Markdown comment for an update. Verified evidence is labelled as
/// such; reporter text is labelled as an agent report.
fn document(update: &Value) -> String {
    let kind = update["kind"].as_str().unwrap_or("");
    let prefix = if matches!(kind, "evidence" | "pr_link" | "research_outcome") {
        "**Crewroom verified evidence**".to_owned()
    } else {
        format!("**Crewroom {kind} — agent report**")
    };
    format!(
        "{prefix}\n\n{}\n\nRun {}\n{}",
        update["body"].as_str().unwrap_or(""),
        update["runId"].as_str().unwrap_or(""),
        marker(update)
    )
}
/// GitHub may store comment bodies with CRLF line endings.
fn normalized(body: &str) -> String {
    body.replace("\r\n", "\n").trim_end().to_owned()
}
/// The issue number from a GitHub ticket key (`#N`).
fn issue_number(task: &Value) -> Option<u64> {
    task["key"]
        .as_str()?
        .strip_prefix('#')?
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

/// The saved GitHub Issues source: repository, site and workflow labels.
struct Source {
    repository: String,
    site: Value,
    labels: Value,
}
fn source(s: &Store) -> Result<Option<Source>> {
    Ok(s.github_sync_state()?.and_then(|saved| {
        let repository = saved["source"]["repository"].as_str()?.to_owned();
        Some(Source {
            site: json!(crewroom_core::github_issues::site(&repository)),
            repository,
            labels: saved
                .get("statusLabels")
                .cloned()
                .unwrap_or(json!({"inProgress":null,"inReview":null})),
        })
    }))
}

#[derive(Clone)]
pub struct GitHubDelivery<S: StoreAccess> {
    store: S,
    credentials: Credentials,
    fixture: Option<String>,
    evidence: Option<Evidence<S>>,
    research: Option<Research<S>>,
    flight: DeliveryFlight,
    force: Arc<AtomicBool>,
}
impl<S: StoreAccess> GitHubDelivery<S> {
    /// `credentials` reads the active GitHub token per request; `fixture`
    /// replaces api.github.com with a loopback test origin only.
    pub fn new(
        store: S,
        credentials: Credentials,
        fixture: Option<String>,
        evidence: Option<Evidence<S>>,
        research: Option<Research<S>>,
    ) -> Self {
        Self {
            store,
            credentials,
            fixture,
            evidence,
            research,
            flight: DeliveryFlight::default(),
            force: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn read(&self, run_id: &str) -> Result<Vec<Value>> {
        self.store.with_store(|s| {
            s.get_run(run_id)?;
            s.github_updates(Some(run_id))
        })
    }
    /// Whether a research run on a GitHub-sourced ticket still needs its
    /// outcome comment requested before it may complete.
    pub fn needs_research_summary(&self, run_id: &str, revision: u64) -> Result<bool> {
        self.store.with_store(|s| {
            let task = s.get_task(text(&s.get_run(run_id)?, "taskId")?)?;
            let Some(source) = source(s)? else {
                return Ok(false);
            };
            Ok(
                ticket_source::belongs_to(&task, ticket_source::GITHUB, &source.site)
                    && !s.github_updates(Some(run_id))?.iter().any(|u| {
                        u["kind"] == "research_outcome"
                            && u["eventKey"] == format!("research-outcome-{revision}")
                            && u["state"] != "blocked"
                    }),
            )
        })
    }
    /// Reconciles every undelivered update once. Concurrent callers share one
    /// pass. Waiting merge observations are re-read at most once a minute.
    pub async fn flush(&self) -> Result<()> {
        let this = self.clone();
        self.flight
            .run(async move { this.perform_flush().await })
            .await
    }
    /// An operator-requested reconcile: also re-reads waiting merge observations.
    pub async fn reconcile(&self) -> Result<()> {
        self.force.store(true, Ordering::SeqCst);
        self.flush().await
    }
    /// Operator retry of an undelivered update. A blocked or uncertain update
    /// returns to pending; the next pass re-reads the issue first, so a write
    /// GitHub already applied is confirmed rather than repeated.
    pub fn retry(&self, update_id: &str) -> Result<Value> {
        self.flight.with_open(|| {
            self.store.with_store(|s| {
                let mut update = s
                    .github_updates(None)?
                    .into_iter()
                    .find(|u| u["id"] == update_id)
                    .ok_or_else(|| {
                        crewroom_core::Error::NotFound("GitHub update not found.".into())
                    })?;
                if !matches!(update["state"].as_str(), Some("blocked" | "uncertain")) {
                    return Err(conflict(
                        "Only a blocked or uncertain GitHub update can be retried.",
                    ));
                }
                update["state"] = json!("pending");
                update["retriedAt"] = json!(time());
                update["reason"] = json!("Retry requested by the operator.");
                s.save_github_update(&update)
            })
        })
    }
    pub async fn close(&self) -> Result<()> {
        self.flight.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_comment_kind_carries_its_label_run_and_stable_marker() {
        for (kind, prefix) in [
            ("milestone", "**Crewroom milestone — agent report**"),
            ("blocker", "**Crewroom blocker — agent report**"),
            ("evidence", "**Crewroom verified evidence**"),
            ("pr_link", "**Crewroom verified evidence**"),
            ("research_outcome", "**Crewroom verified evidence**"),
        ] {
            let update = json!({"id":"00000000-0000-4000-8000-000000000048","kind":kind,"runId":"run-1","body":"Synthetic body"});
            let body = document(&update);
            assert!(body.starts_with(prefix), "{kind}: {body}");
            assert!(body.ends_with(
                "Synthetic body\n\nRun run-1\nCrewroom update 00000000-0000-4000-8000-000000000048"
            ));
            assert_eq!(normalized(&body.replace('\n', "\r\n")), body);
        }
        assert!(labelled(&json!({"kind":"in_progress"})) && labelled(&json!({"kind":"done"})));
        assert!(!labelled(&json!({"kind":"pr_link"})));
    }

    #[test]
    fn only_numbered_issue_keys_name_an_issue() {
        assert_eq!(issue_number(&json!({"key":"#12"})), Some(12));
        for key in [json!("#0"), json!("SIM-1"), json!("12"), Value::Null] {
            assert_eq!(issue_number(&json!({"key":key})), None);
        }
    }
}
