//! Per-project Jira refresh (FOR-53). A project's confirmed Jira site and its
//! own filters drive its refresh; a complete observation commits atomically
//! and imports never start work. Any failure saves nothing partial and records
//! why: not connected (no connection, another site, or no credentials),
//! rejected credentials, unreachable, an HTTP error, or an invalid response.
use crate::{
    delivery_flight::DeliveryFlight,
    messaging::StoreAccess,
    providers::{Error, jira::JiraClient},
    repository::array,
};
use crewroom_core::{Result, jira_projects};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct JiraSync<S: StoreAccess> {
    store: S,
    /// The Jira connection configured on this machine, if any. It serves one
    /// site; projects confirming another site are reported as not connected.
    client: Option<JiraClient>,
    /// One driven refresh per project: concurrent requests for a project
    /// share one complete observation, a cancelled waiter does not stop it,
    /// and close drains every project's refresh. `true` once closing.
    flights: Arc<Mutex<(bool, HashMap<String, DeliveryFlight>)>>,
}

/// Why a project's refresh failed, as recorded with it.
fn classify(error: &Error, site: &str) -> (&'static str, String) {
    match error {
        Error::Unavailable(u) => match (u.status, u.request_sent) {
            (Some(status @ (401 | 403)), _) => (
                "rejected",
                format!(
                    "Jira rejected the credentials for {site} (HTTP {status}). Save a new Jira token under Settings → Integrations, then refresh; nothing was imported."
                ),
            ),
            (Some(_), _) => ("unavailable", u.message.clone()),
            (None, true) => (
                "unreachable",
                format!(
                    "Jira at {site} could not be reached. Check the network, then refresh; nothing was imported."
                ),
            ),
            (None, false)
                if u.message.contains("credentials") || u.message.starts_with("Jira:") =>
            {
                ("not_connected", u.message.clone())
            }
            (None, false) => ("unavailable", u.message.clone()),
        },
        Error::Invalid(_) => (
            "invalid",
            "Jira returned an invalid source response. No partial import was saved.".into(),
        ),
    }
}

impl<S: StoreAccess> JiraSync<S> {
    pub fn new(store: S, client: JiraClient) -> Self {
        Self::with_connection(store, Some(client))
    }
    /// Refresh without a configured Jira connection: every project's refresh
    /// records that Jira is not connected.
    pub fn unconnected(store: S) -> Self {
        Self::with_connection(store, None)
    }
    fn with_connection(store: S, client: Option<JiraClient>) -> Self {
        Self {
            store,
            client,
            flights: Arc::default(),
        }
    }
    /// The connected site, if a connection is configured.
    pub fn site(&self) -> Option<&str> {
        self.client
            .as_ref()
            .and_then(|c| c.source["siteUrl"].as_str())
    }
    /// The connection's public metadata (site and name); never credentials.
    pub fn connection(&self) -> Option<&Value> {
        self.client.as_ref().map(|c| &c.source)
    }
    /// Moves a workspace-level setting still in the connection file into the
    /// projects that confirmed its site. See `offer_workspace_jira_setting`.
    pub fn offer_workspace_setting(&self) -> Result<Value> {
        match &self.client {
            Some(c) => self
                .store
                .with_store(|s| s.offer_workspace_jira_setting(&c.source)),
            None => Ok(json!({"state":"none"})),
        }
    }
    /// Refreshes one project from its confirmed Jira site with its own
    /// filters, then returns its refresh record.
    pub async fn refresh_project(&self, repository_id: &str) -> Result<Value> {
        self.refreshable(repository_id)?;
        // After close a refresh starts nothing and reports the saved record,
        // as provider delivery does while stopping.
        let flight = {
            let mut flights = self.flights.lock().unwrap();
            (!flights.0).then(|| {
                flights
                    .1
                    .entry(repository_id.to_owned())
                    .or_default()
                    .clone()
            })
        };
        if let Some(flight) = flight {
            let this = self.clone();
            let id = repository_id.to_owned();
            flight.run(async move { this.perform(&id).await }).await?;
        }
        self.store
            .with_store(|s| s.jira_project_source(repository_id))
            .map(|v| v.unwrap_or(Value::Null))
    }
    /// Refreshes every active project whose confirmed site is the connected
    /// one and that has filters. Others are left as they are; their status
    /// shows why they do not refresh. Returns the projects refreshed.
    pub async fn refresh_all(&self) -> Result<Vec<String>> {
        let Some(site) = self.site().map(str::to_owned) else {
            return Ok(vec![]);
        };
        let candidates = self.store.with_store(|s| {
            let repositories = s.snapshot()?["repositories"].clone();
            Ok(s.jira_project_sources()?
                .into_iter()
                .filter(|p| {
                    p["source"]["siteUrl"]
                        .as_str()
                        .is_some_and(|v| v.eq_ignore_ascii_case(&site))
                        && jira_projects::has_filters(&p["source"])
                        && array(&repositories)
                            .iter()
                            .any(|r| r["id"] == p["repositoryId"] && r.get("retiredAt").is_none())
                })
                .filter_map(|p| p["repositoryId"].as_str().map(str::to_owned))
                .collect::<Vec<_>>())
        })?;
        let mut refreshed = vec![];
        for id in candidates {
            // A provider failure is recorded with that project; one project's
            // refusal (for example a source changed meanwhile) never stops
            // the others.
            if self.refresh_project(&id).await.is_ok() {
                refreshed.push(id);
            }
        }
        Ok(refreshed)
    }
    pub async fn close(&self) -> Result<()> {
        let flights: Vec<DeliveryFlight> = {
            let mut flights = self.flights.lock().unwrap();
            flights.0 = true;
            flights.1.values().cloned().collect()
        };
        for flight in flights {
            flight.close().await?;
        }
        Ok(())
    }
    /// The project's refresh record, or why it cannot refresh at all: not a
    /// Jira project, retired, or without filters. Nothing is recorded then.
    fn refreshable(&self, repository_id: &str) -> Result<Value> {
        let saved = self.store.with_store(|s| {
            let repository = s.configured_repository(repository_id)?;
            if repository.get("retiredAt").is_some() {
                return Err(crewroom_core::Error::Conflict(
                    "Retired repositories cannot refresh Jira.".into(),
                ));
            }
            s.jira_project_source(repository_id)
        })?;
        let saved = saved.ok_or_else(|| {
            crewroom_core::Error::Conflict(
                "Confirm a Jira site as this project's ticket source under Settings → Projects before refreshing.".into(),
            )
        })?;
        if jira_projects::jql(&saved["source"]).is_none() {
            return Err(crewroom_core::Error::Conflict(
                "Save this project's Jira filters (a project key, saved filter or JQL) before refreshing.".into(),
            ));
        }
        Ok(saved)
    }
    async fn perform(&self, repository_id: &str) -> Result<()> {
        let saved = self.refreshable(repository_id)?;
        let source = saved["source"].clone();
        let site = source["siteUrl"].as_str().unwrap_or("").to_owned();
        let jql = jira_projects::jql(&source).unwrap_or_default();
        let fail = |reason: &str, message: String| {
            self.store
                .with_store(|s| s.fail_jira_project_sync(repository_id, &source, reason, &message))
        };
        let client = match &self.client {
            None => {
                return fail(
                    "not_connected",
                    format!(
                        "Jira is not connected on this machine, so {site} cannot refresh. Configure the Jira connection, then refresh; nothing was imported."
                    ),
                );
            }
            Some(c)
                if !c.source["siteUrl"]
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case(&site)) =>
            {
                return fail(
                    "not_connected",
                    format!(
                        "Crewroom's Jira connection is for {}, not {site}, so this project cannot refresh. Nothing was imported.",
                        c.source["siteUrl"].as_str().unwrap_or("another site")
                    ),
                );
            }
            Some(c) => c,
        };
        let observed: crate::providers::Result<(Vec<Value>, Vec<Value>)> = async {
            let issues = client.search(&jql).await?;
            let mut ids: HashSet<String> = issues
                .iter()
                .filter_map(|i| i["id"].as_str().map(str::to_owned))
                .collect();
            let workspace = self
                .store
                .with_store(|s| s.snapshot())
                .map_err(|_| Error::Invalid("Invalid source response.".into()))?;
            let previous: HashSet<&str> = array(&saved["taskIds"])
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let mut retained = Vec::new();
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
                // Only this project's tickets: those its refresh matched last
                // time, or that target it.
                let ours = task["id"].as_str().is_some_and(|id| previous.contains(id))
                    || task["repositoryId"] == repository_id;
                if ours
                    && crewroom_core::ticket_source::belongs_to(
                        task,
                        crewroom_core::ticket_source::JIRA,
                        &json!(site),
                    )
                    && let Some(id) = task["sourceId"].as_str().filter(|s| !s.is_empty())
                    && !ids.contains(id)
                {
                    retained.push(client.issue(id).await?);
                    ids.insert(id.into());
                }
            }
            Ok((issues, retained))
        }
        .await;
        match observed {
            Ok((issues, retained)) => {
                let committed = self.store.with_store(|s| {
                    s.commit_jira_project_sync(
                        repository_id,
                        &source,
                        &json!(issues),
                        &json!(retained),
                    )
                });
                if committed.is_err() {
                    fail(
                        "invalid",
                        "Jira returned issues Crewroom could not import. No partial import was saved."
                            .into(),
                    )?;
                }
                Ok(())
            }
            Err(error) => {
                let (reason, message) = classify(&error, &site);
                fail(reason, message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Unavailable;

    fn unavailable(status: Option<u16>, sent: bool, message: &str) -> Error {
        Error::Unavailable(Unavailable {
            message: message.into(),
            status,
            request_sent: sent,
        })
    }

    #[test]
    fn failures_are_classified_without_provider_detail() {
        let site = "https://fixture.atlassian.net";
        for status in [401, 403] {
            let (reason, message) = classify(&unavailable(Some(status), true, "x"), site);
            assert_eq!(reason, "rejected");
            assert!(message.contains(&format!("HTTP {status}")) && message.contains(site));
        }
        assert_eq!(
            classify(
                &unavailable(
                    Some(503),
                    true,
                    "Jira returned HTTP 503. Saved work is retained."
                ),
                site
            ),
            (
                "unavailable",
                "Jira returned HTTP 503. Saved work is retained.".into()
            )
        );
        assert_eq!(
            classify(&unavailable(None, true, "x"), site).0,
            "unreachable"
        );
        assert_eq!(
            classify(
                &unavailable(None, false, "Jira credentials are unavailable."),
                site
            )
            .0,
            "not_connected"
        );
        assert_eq!(
            classify(
                &unavailable(None, false, "Jira pagination did not complete."),
                site
            )
            .0,
            "unavailable"
        );
        assert_eq!(classify(&Error::Invalid("x".into()), site).0, "invalid");
    }
}
