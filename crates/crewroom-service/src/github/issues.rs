//! GitHub Issues as a ticket source (FOR-45): save the repository and filters,
//! sync on request, and project a truthful status for the workbench.
//!
//! A sync reads with the active GitHub token (saved or `CREWROOM_GITHUB_TOKEN`)
//! and commits one complete observation atomically; any failure saves nothing
//! partial and records why: not connected, credentials unreadable, token
//! rejected, no access to the repository (missing or not installed/granted),
//! issues disabled, rate limited, unreachable, or an unusable filter. Imported
//! tickets carry GitHub's open/closed state as their source status only.
use super::{Active, GitHub};
use crate::{ApiError, AppState, array, operation};
use crewroom_core::{
    Error,
    github_issues::{parse_repository, site},
    ticket_source,
};
use crewroom_runtime::providers::{
    github::issues::{Failure, Filters},
    secrets,
};
use serde_json::{Value, json};
use std::collections::HashSet;

impl GitHub {
    /// The active token for a sync, or the recorded reason there is none.
    fn issue_token(&self) -> Result<zeroize::Zeroizing<String>, (&'static str, String)> {
        match self.active() {
            Active::Token(_, token) => Ok(token),
            Active::Unreadable => Err((
                "unreadable",
                format!(
                    "GitHub: {}. Connect GitHub again under Settings → Integrations; nothing was imported.",
                    secrets::UNREADABLE
                ),
            )),
            Active::None => Err((
                "not_connected",
                "GitHub is not connected. Connect GitHub under Settings → Integrations, then sync; nothing was imported.".into(),
            )),
        }
    }
}

/// `POST /api/github/issues/{source|sync}`.
pub(crate) async fn mutate(state: &AppState, action: &str, body: Value) -> Result<Value, ApiError> {
    match action {
        "source" => {
            operation(state, move |s| s.store.save_github_source(&body)).await?;
        }
        "sync" => {
            if body != json!({}) {
                return Err(Error::Invalid("Sync accepts no fields.".into()).into());
            }
            sync(state).await?;
        }
        _ => return Err(Error::NotFound("Unknown GitHub Issues operation.".into()).into()),
    }
    state.changes.send_modify(|v| *v += 1);
    Ok(json!({"githubIssues": workspace_view(state).await?}))
}

/// The status projection for the workspace. Never carries a token.
pub(crate) async fn workspace_view(state: &AppState) -> Result<Value, ApiError> {
    let saved = operation(state, |s| s.store.github_sync_state()).await?;
    Ok(view(saved.as_ref(), &state.github.view()))
}

async fn sync(state: &AppState) -> Result<(), ApiError> {
    let github = state.github.clone();
    // One sync at a time; a second request waits and then observes again.
    let _flight = github.issue_sync.lock().await;
    let saved = operation(state, |s| s.store.github_sync_state())
        .await?
        .ok_or_else(|| {
            Error::Conflict("Save a GitHub repository as the ticket source before syncing.".into())
        })?;
    let source = saved["source"].clone();
    let repository = source["repository"].as_str().unwrap_or("").to_owned();
    let Some((owner, name)) = parse_repository(&repository) else {
        return Err(
            Error::Conflict("The saved GitHub source is invalid; save it again.".into()).into(),
        );
    };
    let outcome = match github.issue_token() {
        Err((reason, message)) => Err((reason.to_owned(), message)),
        Ok(token) => observe(state, &github, &token, &owner, &name, &source)
            .await
            .map_err(|f| (f.kind().to_owned(), f.message(&repository))),
    };
    match outcome {
        Ok((issues, retained)) => {
            let committed = source.clone();
            let result = operation(state, move |s| {
                s.store
                    .commit_github_sync(&committed, &json!(issues), &json!(retained))
            })
            .await;
            if let Err(error) = result {
                let message = format!(
                    "GitHub returned issues Crewroom could not import ({}); nothing was imported.",
                    error.0
                );
                operation(state, move |s| {
                    s.store.fail_github_sync(&source, "invalid", &message)
                })
                .await?;
            }
        }
        Err((reason, message)) => {
            operation(state, move |s| {
                s.store.fail_github_sync(&source, &reason, &message)
            })
            .await?;
        }
    }
    Ok(())
}

/// Lists the matching issues, then re-reads issues that have open runs but no
/// longer match, so a requirement change on running work is still noticed.
async fn observe(
    state: &AppState,
    github: &GitHub,
    token: &str,
    owner: &str,
    name: &str,
    source: &Value,
) -> Result<(Vec<Value>, Vec<Value>), Failure> {
    let listing = github
        .client
        .list_issues(token, owner, name, &Filters::from_source(source))
        .await?;
    let mut ids: HashSet<String> = listing
        .issues
        .iter()
        .filter_map(|i| i["id"].as_u64().map(|id| id.to_string()))
        .collect();
    let site = json!(site(&format!("{owner}/{name}")));
    let workspace = operation(state, |s| s.store.snapshot())
        .await
        .map_err(|e| Failure::Unavailable(e.0.to_string()))?;
    let mut retained = vec![];
    for run in array(&workspace["runs"])
        .iter()
        .filter(|r| r["desiredState"] != "cancelled")
    {
        let Some(task) = array(&workspace["tasks"])
            .iter()
            .find(|t| t["id"] == run["taskId"])
        else {
            continue;
        };
        if !ticket_source::belongs_to(task, ticket_source::GITHUB, &site) {
            continue;
        }
        let Some(id) = task["sourceId"].as_str() else {
            continue;
        };
        let Some(number) = task["key"]
            .as_str()
            .and_then(|k| k.strip_prefix('#'))
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        if ids.contains(id) {
            continue;
        }
        if let Some(issue) = github.client.issue(token, owner, name, number).await?
            && issue["id"].as_u64().map(|v| v.to_string()).as_deref() == Some(id)
        {
            retained.push(issue);
            ids.insert(id.to_owned());
        }
    }
    Ok((listing.issues, retained))
}

fn filters_text(source: &Value) -> String {
    let mut parts = vec![format!(
        "state {}",
        source["state"].as_str().unwrap_or("open")
    )];
    if let Some(labels) = source["labels"].as_array().filter(|a| !a.is_empty()) {
        let labels: Vec<_> = labels.iter().filter_map(Value::as_str).collect();
        parts.push(format!("labels {}", labels.join(", ")));
    }
    if let Some(assignee) = source["assignee"].as_str() {
        parts.push(format!("assignee {assignee}"));
    }
    if let Some(milestone) = source["milestone"].as_str() {
        parts.push(format!("milestone {milestone}"));
    }
    parts.join(" · ")
}

/// Status for Settings and the ticket list. GitHub's connection state is shown
/// beside the last sync result, so a disconnected token is never hidden behind
/// an older successful sync.
pub(crate) fn view(saved: Option<&Value>, connection: &Value) -> Value {
    let Some(saved) = saved else {
        return json!({
            "state": "not_configured",
            "label": "No GitHub repository is saved as a ticket source.",
        });
    };
    let source = &saved["source"];
    let repository = source["repository"].as_str().unwrap_or("");
    let state = saved["state"].as_str().unwrap_or("not_synced");
    let label = match state {
        "connected" => format!(
            "{repository}: {} issue{} imported in the last complete sync.",
            saved["issueCount"].as_u64().unwrap_or(0),
            if saved["issueCount"] == 1 { "" } else { "s" }
        ),
        "unavailable" => format!(
            "{repository}: last sync failed. {}",
            saved["error"].as_str().unwrap_or("")
        ),
        _ => format!("{repository}: saved, not synced yet."),
    };
    let connection_state = connection["state"].as_str().unwrap_or("");
    let connection_note = match connection_state {
        "disconnected" => Some("GitHub is not connected, so sync is unavailable."),
        "unreadable" => Some("GitHub credentials are unreadable, so sync is unavailable."),
        "rejected" => Some(
            "GitHub rejected the token at the last check, so sync will fail until you reconnect.",
        ),
        _ => None,
    };
    let mut view = json!({
        "state": state,
        "label": label,
        "source": source,
        "filters": filters_text(source),
        "site": site(repository),
        "lastSyncedAt": saved["lastSyncedAt"],
        "lastAttemptAt": saved["lastAttemptAt"],
        "issueCount": saved["issueCount"],
        "connection": connection_state,
    });
    if state == "unavailable" {
        view["reason"] = saved["reason"].clone();
        view["error"] = saved["error"].clone();
    }
    if let Some(note) = connection_note {
        view["connectionNote"] = json!(note);
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_reports_each_state_and_keeps_the_connection_visible() {
        assert_eq!(
            view(None, &json!({"state":"connected"}))["state"],
            "not_configured"
        );
        let source = json!({"repository":"o/r","labels":["bug"],"assignee":null,"milestone":null,"state":"open"});
        let saved = json!({"source":source,"state":"not_synced","taskIds":[]});
        let fresh = view(Some(&saved), &json!({"state":"disconnected"}));
        assert_eq!(fresh["label"], "o/r: saved, not synced yet.");
        assert_eq!(fresh["filters"], "state open · labels bug");
        assert_eq!(
            fresh["connectionNote"],
            "GitHub is not connected, so sync is unavailable."
        );
        let synced = json!({"source":source,"state":"connected","issueCount":1,"lastSyncedAt":"t"});
        assert_eq!(
            view(Some(&synced), &json!({"state":"connected"}))["label"],
            "o/r: 1 issue imported in the last complete sync."
        );
        let failed = json!({"source":source,"state":"unavailable","reason":"rate_limited","error":"GitHub rate-limited this request (HTTP 403).","lastSyncedAt":"t"});
        let failed = view(Some(&failed), &json!({"state":"connected"}));
        assert_eq!(failed["reason"], "rate_limited");
        assert_eq!(failed["lastSyncedAt"], "t");
        assert!(failed["label"].as_str().unwrap().contains("rate-limited"));
    }
}
