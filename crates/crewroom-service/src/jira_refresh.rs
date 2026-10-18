//! A project's confirmed Jira site drives that project's Jira refresh
//! (FOR-53). Each project whose confirmed ticket source (FOR-47) is a Jira
//! site saves its own filters and refreshes from that site; the status of
//! every project is shown beside the Jira connection, so not connected,
//! rejected and unreachable are visible per project. The Jira connection's
//! credentials never enter a projection.
//!
//! Routes: `POST /api/repositories/{id}/jira/{filters|sync}` and
//! `POST /api/jira/sync`, which refreshes every project whose site is the
//! connected one and reports when no project uses it.
use crate::{ApiError, array, execution::Execution};
use crewroom_core::{Error, jira_projects};
use crewroom_runtime::messaging::StoreAccess;
use serde_json::{Value, json};
use std::sync::Arc;

/// `POST /api/repositories/{id}/jira/{filters|sync}`.
pub(crate) async fn mutate(
    e: &Arc<Execution>,
    repository_id: &str,
    action: &str,
    body: &Value,
) -> Result<Value, ApiError> {
    match action {
        "filters" => {
            e.store
                .with_store(|s| s.save_jira_filters(repository_id, body))?;
        }
        "sync" => {
            if body != &json!({}) {
                return Err(Error::Invalid("Refresh accepts no fields.".into()).into());
            }
            e.sync.refresh_project(repository_id).await?;
        }
        _ => return Err(Error::NotFound("Unknown Jira refresh operation.".into()).into()),
    }
    e.changed();
    Ok(json!({"jiraRefresh": workspace_view(e)?}))
}

/// `POST /api/jira/sync`: refreshes every project that uses the connected
/// site. With no such project nothing is refreshed and the summary says so.
pub(crate) async fn refresh_all(e: &Arc<Execution>, body: &Value) -> Result<Value, ApiError> {
    if body != &json!({}) {
        return Err(Error::Invalid("Refresh accepts no fields.".into()).into());
    }
    if e.sync.site().is_none() {
        return Err(Error::Conflict("Jira connection is not configured.".into()).into());
    }
    let refreshed = e.sync.refresh_all().await?;
    e.changed();
    Ok(json!({"refreshed": refreshed, "jiraRefresh": workspace_view(e)?}))
}

pub(crate) fn workspace_view(e: &Execution) -> crewroom_core::Result<Value> {
    let (sources, legacy, repositories) = e.store.with_store(|s| {
        Ok((
            s.jira_project_sources()?,
            s.legacy_jira_setting()?,
            s.snapshot()?["repositories"].clone(),
        ))
    })?;
    Ok(summary(
        &sources,
        legacy.as_ref(),
        &repositories,
        e.sync.connection(),
    ))
}

/// The connection as projects see it: configured for one site, or not.
fn connection_view(connection: Option<&Value>) -> Value {
    match connection.and_then(|c| c["siteUrl"].as_str()) {
        Some(site) => json!({"state":"configured","site":site}),
        None => json!({"state":"not_configured","site":null}),
    }
}

/// Every active project's Jira refresh status beside the Jira connection. A
/// workspace-level setting no project uses is reported, never refreshed.
pub(crate) fn summary(
    sources: &[Value],
    legacy: Option<&Value>,
    repositories: &Value,
    connection: Option<&Value>,
) -> Value {
    let connection = connection_view(connection);
    let mut projects = vec![];
    for saved in sources {
        let Some(project) = array(repositories)
            .iter()
            .find(|r| r["id"] == saved["repositoryId"] && r.get("retiredAt").is_none())
        else {
            continue;
        };
        let mut status = view(Some(saved), &connection);
        status["repositoryId"] = saved["repositoryId"].clone();
        status["project"] = project["name"].clone();
        projects.push(status);
    }
    let label = match projects.len() {
        0 => "No project has a Jira site as its ticket source, so nothing refreshes from Jira. Confirm a Jira site for a project under Settings → Repositories.".to_owned(),
        n => format!(
            "{n} project{} refresh{} from Jira; each sets its own filters under Settings → Repositories.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "es" } else { "" }
        ),
    };
    let connection_label = match connection["site"].as_str() {
        Some(site) => format!("Jira connection: {site}."),
        None => "Jira is not connected on this machine; no project can refresh from Jira.".into(),
    };
    let mut summary = json!({
        "label": label,
        "connection": connection,
        "connectionLabel": connection_label,
        "projects": projects,
        "legacy": null,
    });
    if let Some(legacy) = legacy
        && let Some(source) = jira_projects::workspace_source(&legacy["source"])
    {
        let site = source["siteUrl"].as_str().unwrap_or("");
        let filters = jira_projects::filters_text(&source);
        summary["legacy"] = json!({
            "source": source,
            "filters": filters,
            "label": format!(
                "The workspace-level Jira refresh setting ({site}; {filters}) is not used by any project, so it does not refresh. Confirm Jira {site} as a project's ticket source to adopt its filters."
            ),
        });
    }
    summary
}

/// One project's refresh status. The connection is checked beside the last
/// result, so a missing or other-site connection is never hidden behind an
/// older successful refresh.
pub(crate) fn view(saved: Option<&Value>, connection: &Value) -> Value {
    let Some(saved) = saved else {
        return json!({
            "state": "not_configured",
            "label": "This project's ticket source is not a Jira site.",
        });
    };
    let source = &saved["source"];
    let site = source["siteUrl"].as_str().unwrap_or("");
    let filters = jira_projects::has_filters(source);
    let connected_site = connection["site"].as_str();
    let connection_state = match connected_site {
        None => "not_connected",
        Some(c) if !c.eq_ignore_ascii_case(site) => "other_site",
        Some(_) => "configured",
    };
    let state = saved["state"].as_str().unwrap_or("not_synced");
    let label = match state {
        "connected" => format!(
            "{site}: {} ticket{} matched the last complete refresh.",
            saved["issueCount"]
                .as_u64()
                .unwrap_or(array(&saved["taskIds"]).len() as u64),
            if saved["issueCount"] == 1 { "" } else { "s" }
        ),
        "unavailable" => format!(
            "{site}: last refresh failed. {}",
            saved["error"].as_str().unwrap_or("")
        ),
        _ if !filters => format!(
            "{site}: no filters saved yet. Save a project key, saved filter or JQL, then refresh."
        ),
        _ => format!("{site}: filters saved, not refreshed yet."),
    };
    let connection_note = match connection_state {
        "not_connected" => Some(format!(
            "Not connected: Jira is not connected on this machine, so {site} does not refresh."
        )),
        "other_site" => Some(format!(
            "Not connected: Crewroom's Jira connection is for {}, not {site}, so this project does not refresh.",
            connected_site.unwrap_or("")
        )),
        _ => None,
    };
    // The truthful availability shown per project: the connection first,
    // then the reason the last refresh failed.
    let availability = match (connection_state, state, saved["reason"].as_str()) {
        ("not_connected" | "other_site", _, _) => "not_connected",
        (_, "unavailable", Some(reason)) => reason,
        (_, "connected", _) => "connected",
        _ if !filters => "needs_filters",
        _ => "not_synced",
    };
    let mut view = json!({
        "state": state,
        "availability": availability,
        "label": label,
        "source": source,
        "site": site,
        "filters": jira_projects::filters_text(source),
        "hasFilters": filters,
        "jql": jira_projects::jql(source),
        "lastSyncedAt": saved["lastSyncedAt"],
        "lastAttemptAt": saved["lastAttemptAt"],
        "adoptedFrom": saved["adoptedFrom"],
        "issueCount": saved["issueCount"],
        "connection": connection_state,
        "canRefresh": connection_state == "configured" && filters,
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

    fn source(project: Option<&str>) -> Value {
        json!({"siteUrl":"https://a.example","projectKey":project,"filterId":null,"assignee":null,"jql":null})
    }

    #[test]
    fn each_project_shows_not_connected_rejected_and_unreachable() {
        let a = json!({"state":"configured","site":"https://a.example"});
        let fresh = view(
            Some(&json!({"source":source(None),"state":"not_synced"})),
            &a,
        );
        assert_eq!(fresh["availability"], "needs_filters");
        assert_eq!(fresh["canRefresh"], false);
        assert!(
            fresh["label"]
                .as_str()
                .unwrap()
                .contains("no filters saved")
        );
        let saved = json!({"source":source(Some("CR")),"state":"not_synced"});
        assert_eq!(view(Some(&saved), &a)["canRefresh"], true);
        let none = view(Some(&saved), &connection_view(None));
        assert_eq!(none["availability"], "not_connected");
        assert!(
            none["connectionNote"]
                .as_str()
                .unwrap()
                .starts_with("Not connected")
        );
        let other = view(
            Some(&saved),
            &json!({"state":"configured","site":"https://b.example"}),
        );
        assert_eq!(other["availability"], "not_connected");
        assert!(
            other["connectionNote"]
                .as_str()
                .unwrap()
                .contains("https://b.example")
        );
        for reason in ["rejected", "unreachable"] {
            let failed = json!({"source":source(Some("CR")),"state":"unavailable","reason":reason,"error":"why","lastSyncedAt":"t"});
            let failed = view(Some(&failed), &a);
            assert_eq!(failed["availability"], reason);
            assert_eq!(failed["lastSyncedAt"], "t");
        }
        let synced = json!({"source":source(Some("CR")),"state":"connected","issueCount":2});
        assert_eq!(
            view(Some(&synced), &a)["label"],
            "https://a.example: 2 tickets matched the last complete refresh."
        );
    }

    #[test]
    fn summary_names_projects_and_reports_an_unused_workspace_setting() {
        let repositories =
            json!([{"id":"a","name":"Alpha"},{"id":"b","name":"Old","retiredAt":"t"}]);
        let sources = [
            json!({"repositoryId":"a","source":source(Some("CR")),"state":"not_synced"}),
            json!({"repositoryId":"b","source":source(Some("CR")),"state":"not_synced"}),
        ];
        let connection = json!({"siteUrl":"https://a.example"});
        let s = summary(&sources, None, &repositories, Some(&connection));
        assert_eq!(s["projects"].as_array().unwrap().len(), 1);
        assert_eq!(s["projects"][0]["project"], "Alpha");
        assert!(s["legacy"].is_null());
        let legacy = json!({"source":{"siteUrl":"https://c.example","filterId":"12345","assigneeAccountId":"operator-1","name":"Assigned work"}});
        let s = summary(&[], Some(&legacy), &json!([]), None);
        assert!(
            s["label"]
                .as_str()
                .unwrap()
                .starts_with("No project has a Jira site")
        );
        let label = s["legacy"]["label"].as_str().unwrap();
        assert!(label.contains("is not used by any project, so it does not refresh"));
        assert!(label.contains("saved filter 12345"));
        assert_eq!(s["connection"]["state"], "not_configured");
        assert!(!s.to_string().contains("cloudId"));
    }
}
