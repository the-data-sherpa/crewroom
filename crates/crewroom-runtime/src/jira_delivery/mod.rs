//! Durable Jira intent, provider receipt and imported issue status are distinct.
mod dispatch;
mod prepare;
use crate::{
    conflict,
    delivery_evidence::{Proof, VerifiedText},
    delivery_flight::DeliveryFlight,
    evidence::{Evidence, Research},
    messaging::StoreAccess,
    providers::{self, jira::JiraClient},
    repository::{array, text},
};
use crewroom_core::{Error, Result, store::Store};
use serde_json::{Value, json};
use std::collections::HashSet;
fn time() -> String {
    crewroom_core::timestamp(crewroom_core::now_ms())
}
fn controller(update: &Value) -> bool {
    matches!(update["kind"].as_str(), Some("in_review" | "done"))
}
fn transition(update: &Value) -> bool {
    update["kind"] == "in_progress" || controller(update)
}
fn clear(value: &mut Value, key: &str) {
    value.as_object_mut().unwrap().remove(key);
}
fn block(update: &mut Value, reason: &str) {
    update["state"] = json!("blocked");
    update["reason"] = json!(reason);
}
fn present(value: &Value) -> bool {
    !value.is_null() && value != false
}
fn marker(update: &Value) -> String {
    format!("Crewroom update {}", update["id"].as_str().unwrap())
}
fn document(update: &Value) -> Value {
    let prefix = if matches!(
        update["kind"].as_str(),
        Some("evidence" | "pr_link" | "research_outcome")
    ) {
        "Crewroom verified evidence".into()
    } else {
        format!(
            "Crewroom {} — agent report",
            update["kind"].as_str().unwrap()
        )
    };
    let body = format!(
        "{prefix}\n\n{}\n\nRun {}\n{}",
        update["body"].as_str().unwrap(),
        update["runId"].as_str().unwrap(),
        marker(update)
    );
    json!({"type":"doc","version":1,"content":body.split('\n').map(|text| json!({"type":"paragraph","content":if text.is_empty(){vec![]}else{vec![json!({"type":"text","text":text})]}})).collect::<Vec<_>>()})
}
#[derive(Clone)]
pub struct JiraDelivery<S: StoreAccess> {
    store: S,
    client: JiraClient,
    evidence: Option<Evidence<S>>,
    research: Option<Research<S>>,
    flight: DeliveryFlight,
}
impl<S: StoreAccess> JiraDelivery<S> {
    pub fn new(
        store: S,
        client: JiraClient,
        evidence: Option<Evidence<S>>,
        research: Option<Research<S>>,
    ) -> Self {
        Self {
            store,
            client,
            evidence,
            research,
            flight: DeliveryFlight::default(),
        }
    }
    pub fn read(&self, run_id: &str) -> Result<Vec<Value>> {
        self.store.with_store(|s| {
            s.get_run(run_id)?;
            s.jira_updates(Some(run_id))
        })
    }
    pub fn needs_research_summary(&self, run_id: &str, revision: u64) -> Result<bool> {
        self.store.with_store(|s| {
            let task = s.get_task(text(&s.get_run(run_id)?, "taskId")?)?;
            Ok(crewroom_core::ticket_source::belongs_to(
                &task,
                crewroom_core::ticket_source::JIRA,
                &self.client.source["siteUrl"],
            ) && !s.jira_updates(Some(run_id))?.iter().any(|u| {
                u["kind"] == "research_outcome"
                    && u["eventKey"] == format!("research-outcome-{revision}")
                    && u["state"] != "blocked"
            }))
        })
    }
    pub async fn flush(&self) -> Result<()> {
        let this = self.clone();
        self.flight
            .run(async move { this.perform_flush().await })
            .await
    }
    pub async fn close(&self) -> Result<()> {
        self.flight.close().await
    }
}
