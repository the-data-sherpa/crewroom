//! GitHub Issues as a project's ticket source (FOR-45): each project whose
//! confirmed ticket source (FOR-47) is a GitHub repository has its own
//! filters, sync and truthful status. The repository is the confirmed one;
//! only the filters are saved here.
//!
//! A sync reads with the active GitHub token (saved or `CREWROOM_GITHUB_TOKEN`)
//! and commits one complete observation atomically; any failure saves nothing
//! partial and records why: not connected, credentials unreadable, token
//! rejected, no access to the repository (missing or not installed/granted),
//! issues disabled, rate limited, unreachable, or an unusable filter. Imported
//! tickets carry GitHub's open/closed state as their source status only.
//!
//! Imported tickets that leave the filter (for example an issue closed under
//! the default `state=open`) are re-read once so their last GitHub status is
//! recorded; tickets with unfinished runs are re-read on every sync. An issue
//! GitHub no longer serves (deleted, transferred or hidden) is listed with the
//! sync instead of failing it.
use super::{Active, GitHub};
use crate::{ApiError, AppState, array, operation};
use crewroom_core::{
    Error,
    github_issues::{parse_repository, site},
    ticket_source,
};
use crewroom_runtime::providers::{
    github::issues::{Failure, Filters, Reread},
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

/// `POST /api/repositories/{id}/github-issues/{filters|labels|sync}`.
pub(crate) async fn mutate(
    state: &AppState,
    repository_id: &str,
    action: &str,
    body: Value,
) -> Result<Value, ApiError> {
    let id = repository_id.to_owned();
    match action {
        "filters" => {
            operation(state, move |s| s.store.save_github_filters(&id, &body)).await?;
        }
        // Optional workflow labels for issue delivery (FOR-48); off by default.
        "labels" => {
            operation(state, move |s| {
                s.store.save_github_status_labels(&id, &body)
            })
            .await?;
        }
        "sync" => {
            if body != json!({}) {
                return Err(Error::Invalid("Sync accepts no fields.".into()).into());
            }
            sync(state, &id).await?;
        }
        _ => return Err(Error::NotFound("Unknown GitHub Issues operation.".into()).into()),
    }
    state.changes.send_modify(|v| *v += 1);
    Ok(json!({"githubIssues": workspace_view(state).await?}))
}

/// The status of every project's GitHub Issues source, and of a former
/// workspace-level source no project has adopted yet. Never carries a token.
pub(crate) async fn workspace_view(state: &AppState) -> Result<Value, ApiError> {
    let (sources, legacy, repositories) = operation(state, |s| {
        Ok((
            s.store.github_sources()?,
            s.store.legacy_github_source()?,
            s.store.snapshot()?["repositories"].clone(),
        ))
    })
    .await?;
    Ok(summary(
        &sources,
        legacy.as_ref(),
        &repositories,
        &state.github.view(),
    ))
}

async fn sync(state: &AppState, repository_id: &str) -> Result<(), ApiError> {
    let github = state.github.clone();
    // One sync at a time; a second request waits and then observes again.
    let _flight = github.issue_sync.lock().await;
    let id = repository_id.to_owned();
    let saved = operation(state, move |s| {
        let repository = s.store.configured_repository(&id)?;
        if repository.get("retiredAt").is_some() {
            return Err(Error::Conflict(
                "Retired repositories cannot sync GitHub issues.".into(),
            ));
        }
        s.store.github_source(&id)
    })
    .await?
    .ok_or_else(|| {
        Error::Conflict(
            "Confirm GitHub Issues as this project's ticket source under Settings → Repositories before syncing.".into(),
        )
    })?;
    let source = saved["source"].clone();
    let repository = source["repository"].as_str().unwrap_or("").to_owned();
    let Some((owner, name)) = parse_repository(&repository) else {
        return Err(Error::Conflict(
            "The project's GitHub source is invalid; confirm it again.".into(),
        )
        .into());
    };
    let outcome = match github.issue_token() {
        Err((reason, message)) => Err((reason.to_owned(), message)),
        Ok(token) => observe(state, &github, &token, &owner, &name, &saved)
            .await
            .map_err(|f| (f.kind().to_owned(), f.message(&repository))),
    };
    let id = repository_id.to_owned();
    match outcome {
        Ok(observed) => {
            let committed = source.clone();
            let project = id.clone();
            let result = operation(state, move |s| {
                s.store.commit_github_sync(
                    &project,
                    &committed,
                    &json!(observed.issues),
                    &json!(observed.retained),
                    &json!(observed.unreadable),
                )
            })
            .await;
            if let Err(error) = result {
                let message = format!(
                    "GitHub returned issues Crewroom could not import ({}); nothing was imported.",
                    error.0
                );
                operation(state, move |s| {
                    s.store.fail_github_sync(&id, &source, "invalid", &message)
                })
                .await?;
            }
        }
        Err((reason, message)) => {
            operation(state, move |s| {
                s.store.fail_github_sync(&id, &source, &reason, &message)
            })
            .await?;
        }
    }
    Ok(())
}

/// One complete observation of the saved source.
struct Observed {
    /// Issues matching the filters, in GitHub's order.
    issues: Vec<Value>,
    /// Imported issues re-read because they no longer match.
    retained: Vec<Value>,
    /// `{taskId, key, status}` for imported issues GitHub no longer serves.
    unreadable: Vec<Value>,
}

/// A run that can still act on its ticket's requirements.
fn unfinished(run: &Value) -> bool {
    !["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""))
        && run["desiredState"] != "cancelled"
}

/// Lists the matching issues, then re-reads imported issues that are missing
/// from the listing: those that matched at the last sync (so a close, relabel
/// or reassignment is recorded), those GitHub could not serve last time, and
/// those with unfinished runs (so a requirement change is still noticed). A
/// deleted, transferred or hidden issue is recorded as unreadable; token,
/// rate-limit and network failures still fail the sync.
async fn observe(
    state: &AppState,
    github: &GitHub,
    token: &str,
    owner: &str,
    name: &str,
    saved: &Value,
) -> Result<Observed, Failure> {
    let listing = github
        .client
        .list_issues(token, owner, name, &Filters::from_source(&saved["source"]))
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
    let mut recheck: HashSet<&str> = array(&saved["taskIds"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    recheck.extend(
        array(&saved["unreadable"])
            .iter()
            .filter_map(|u| u["taskId"].as_str()),
    );
    let runs = array(&workspace["runs"]);
    let mut observed = Observed {
        issues: vec![],
        retained: vec![],
        unreadable: vec![],
    };
    for task in array(&workspace["tasks"]) {
        if !ticket_source::belongs_to(task, ticket_source::GITHUB, &site) {
            continue;
        }
        let Some(task_id) = task["id"].as_str() else {
            continue;
        };
        if !recheck.contains(task_id)
            && !runs
                .iter()
                .any(|r| r["taskId"] == task["id"] && unfinished(r))
        {
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
        match github.client.issue(token, owner, name, number).await? {
            Reread::Issue(issue)
                if issue["id"].as_u64().map(|v| v.to_string()).as_deref() == Some(id) =>
            {
                observed.retained.push(issue);
                ids.insert(id.to_owned());
            }
            Reread::Gone { status } => observed.unreadable.push(json!({
                "taskId": task_id,
                "key": task["key"],
                "status": status,
            })),
            // A different issue or a pull request under this number: GitHub
            // numbers are not reused, so there is nothing to record.
            Reread::Issue(_) | Reread::PullRequest => {}
        }
    }
    observed.issues = listing.issues;
    Ok(observed)
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

fn connection_note(connection: &Value) -> Option<&'static str> {
    match connection["state"].as_str().unwrap_or("") {
        "disconnected" => Some("GitHub is not connected, so sync is unavailable."),
        "unreadable" => Some("GitHub credentials are unreadable, so sync is unavailable."),
        "rejected" => Some(
            "GitHub rejected the token at the last check, so sync will fail until you reconnect.",
        ),
        _ => None,
    }
}

/// Every project's source status, keyed by `repositoryId` and named after the
/// project, beside the GitHub connection. A former workspace-level source
/// that no project uses yet is reported, never synced.
pub(crate) fn summary(
    sources: &[Value],
    legacy: Option<&Value>,
    repositories: &Value,
    connection: &Value,
) -> Value {
    let mut projects = vec![];
    for saved in sources {
        let Some(project) = array(repositories)
            .iter()
            .find(|r| r["id"] == saved["repositoryId"] && r.get("retiredAt").is_none())
        else {
            continue;
        };
        let mut status = view(Some(saved), connection);
        status["repositoryId"] = saved["repositoryId"].clone();
        status["project"] = project["name"].clone();
        projects.push(status);
    }
    let label = match projects.len() {
        0 => "No project has GitHub Issues as its ticket source. Confirm it for a project under Settings → Repositories.".to_owned(),
        n => format!(
            "{n} project{} import{} GitHub issues; each sets its own filters under Settings → Repositories.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "s" } else { "" }
        ),
    };
    let mut summary = json!({
        "label": label,
        "projects": projects,
        "connection": connection["state"],
        "legacy": null,
    });
    if let Some(note) = connection_note(connection) {
        summary["connectionNote"] = json!(note);
    }
    if let Some(legacy) = legacy {
        let repository = legacy["source"]["repository"].as_str().unwrap_or("");
        summary["legacy"] = json!({
            "source": legacy["source"],
            "filters": filters_text(&legacy["source"]),
            "label": format!(
                "A GitHub Issues source saved before sources were per project ({repository}; {}) is not used by any project and does not sync. Confirm GitHub Issues {repository} as a project's ticket source to keep its filters.",
                filters_text(&legacy["source"])
            ),
        });
    }
    summary
}

/// Status for one project's source. GitHub's connection state is shown beside
/// the last sync result, so a disconnected token is never hidden behind an
/// older successful sync.
pub(crate) fn view(saved: Option<&Value>, connection: &Value) -> Value {
    let Some(saved) = saved else {
        return json!({
            "state": "not_configured",
            "label": "This project's ticket source is not GitHub Issues.",
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
    // Recorded by the last complete sync; kept beside a later failure.
    let unreadable: Vec<&str> = array(&saved["unreadable"])
        .iter()
        .filter_map(|u| u["key"].as_str())
        .collect();
    let label = if unreadable.is_empty() {
        label
    } else {
        format!(
            "{label} GitHub no longer serves {} (deleted, transferred, or hidden from this token); {} the last status GitHub reported.",
            unreadable.join(", "),
            if unreadable.len() == 1 {
                "its ticket shows"
            } else {
                "their tickets show"
            }
        )
    };
    let connection_state = connection["state"].as_str().unwrap_or("");
    let mut view = json!({
        "state": state,
        "label": label,
        "source": source,
        "filters": filters_text(source),
        "site": site(repository),
        "lastSyncedAt": saved["lastSyncedAt"],
        "lastAttemptAt": saved["lastAttemptAt"],
        "adoptedFrom": saved["adoptedFrom"],
        "issueCount": saved["issueCount"],
        "unreadable": saved["unreadable"],
        "connection": connection_state,
    });
    if state == "unavailable" {
        view["reason"] = saved["reason"].clone();
        view["error"] = saved["error"].clone();
    }
    // Issue delivery (FOR-48): labels are applied only when named here.
    let labels = &saved["statusLabels"];
    view["statusLabels"] =
        json!({"inProgress": labels["inProgress"], "inReview": labels["inReview"]});
    view["labelsNote"] = json!(
        match (labels["inProgress"].as_str(), labels["inReview"].as_str()) {
            (None, None) =>
                "Workflow labels are off. Progress is reported as issue comments only.".to_owned(),
            (progress, review) => format!(
                "Workflow labels: in progress {}; in review {}. Labels never change the issue's open/closed state.",
                progress.map_or("off".to_owned(), |l| format!("\"{l}\"")),
                review.map_or("off".to_owned(), |l| format!("\"{l}\""))
            ),
        }
    );
    if let Some(note) = connection_note(connection) {
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

    #[test]
    fn summary_names_each_project_and_reports_an_unadopted_workspace_source() {
        let source =
            json!({"repository":"o/r","labels":[],"assignee":null,"milestone":null,"state":"open"});
        let repositories = json!([
            {"id":"a","name":"Alpha"},
            {"id":"b","name":"Retired","retiredAt":"t"},
        ]);
        let sources = [
            json!({"repositoryId":"a","source":source,"state":"not_synced"}),
            json!({"repositoryId":"b","source":source,"state":"not_synced"}),
        ];
        let connected = json!({"state":"connected"});
        let summary = summary(&sources, None, &repositories, &connected);
        let projects = summary["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1, "retired projects are not listed");
        assert_eq!(projects[0]["repositoryId"], "a");
        assert_eq!(projects[0]["project"], "Alpha");
        assert_eq!(
            summary["label"],
            "1 project imports GitHub issues; each sets its own filters under Settings → Repositories."
        );
        assert!(summary["legacy"].is_null());
        let legacy = json!({"source":{"repository":"o/old","labels":["bug"],"assignee":null,"milestone":null,"state":"all"}});
        let summary = summary_with_legacy(&legacy, &json!({"state":"disconnected"}));
        assert!(
            summary["legacy"]["label"]
                .as_str()
                .unwrap()
                .contains("is not used by any project and does not sync")
        );
        assert_eq!(summary["legacy"]["filters"], "state all · labels bug");
        assert_eq!(
            summary["connectionNote"],
            "GitHub is not connected, so sync is unavailable."
        );
        assert!(
            summary["label"]
                .as_str()
                .unwrap()
                .starts_with("No project has GitHub Issues")
        );
    }

    fn summary_with_legacy(legacy: &Value, connection: &Value) -> Value {
        summary(&[], Some(legacy), &json!([]), connection)
    }
}
