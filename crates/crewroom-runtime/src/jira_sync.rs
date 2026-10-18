//! A complete Jira observation commits atomically; imports never start work.
use crate::{
    delivery_flight::DeliveryFlight,
    messaging::StoreAccess,
    providers::{Error, jira::JiraClient},
    repository::array,
};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};
use std::collections::HashSet;
#[derive(Clone)]
pub struct JiraSync<S: StoreAccess> {
    store: S,
    client: JiraClient,
    flight: DeliveryFlight,
}
impl<S: StoreAccess> JiraSync<S> {
    pub fn new(store: S, client: JiraClient) -> Self {
        Self {
            store,
            client,
            flight: DeliveryFlight::default(),
        }
    }
    pub fn snapshot(&self) -> Result<Value> {
        let saved = self.store.with_store(|s| s.jira_state())?;
        Ok(saved
            .filter(|v| stringify(&v["source"]) == stringify(&self.client.source))
            .unwrap_or_else(
                || json!({"source":self.client.source,"state":"not_synced","taskIds":[]}),
            ))
    }
    pub async fn refresh(&self) -> Result<Value> {
        let this = self.clone();
        self.flight.run(async move { this.perform().await }).await?;
        self.snapshot()
    }
    pub async fn close(&self) -> Result<()> {
        self.flight.close().await
    }
    async fn perform(&self) -> Result<()> {
        let invalid = || Error::Invalid("Invalid source response.".into());
        let result: crate::providers::Result<()> = async {
            let issues = self.client.search().await?;
            let mut ids: HashSet<String> = issues
                .iter()
                .map(|i| i["id"].as_str().unwrap().into())
                .collect();
            let workspace = self
                .store
                .with_store(|s| s.snapshot())
                .map_err(|_| invalid())?;
            let mut retained = Vec::new();
            for run in array(&workspace["runs"])
                .iter()
                .filter(|r| r["desiredState"] != "cancelled")
            {
                let task = array(&workspace["tasks"])
                    .iter()
                    .find(|t| t["id"] == run["taskId"])
                    .ok_or_else(invalid)?;
                if task["source"] == "jira"
                    && task["sourceSite"] == self.client.source["siteUrl"]
                    && let Some(id) = task["sourceId"].as_str().filter(|s| !s.is_empty())
                    && !ids.contains(id)
                {
                    retained.push(self.client.issue(id).await?);
                    ids.insert(id.into());
                }
            }
            self.store
                .with_store(|s| {
                    s.commit_jira_sync(&self.client.source, &json!(issues), &json!(retained))
                })
                .map_err(|_| invalid())?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let message = match error {
                Error::Unavailable(v) => v.message,
                Error::Invalid(_) => {
                    "Jira returned an invalid source response. No partial import was saved.".into()
                }
            };
            self.store
                .with_store(|s| s.fail_jira_sync(&self.client.source, &message))?;
        }
        Ok(())
    }
}
