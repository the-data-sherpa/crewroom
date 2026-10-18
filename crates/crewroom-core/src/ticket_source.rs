//! Provider-neutral ticket source (FOR-40).
//!
//! A ticket's source is an external fact: the provider kind (`source`), its
//! identity (`sourceSite`, `sourceId`, `key`, `sourceUrl`) and the provider's own
//! status (`sourceStatus`, `sourceStatusId`). It is separate from Crewroom's
//! workflow state and from agent activity; nothing here reads either of those.
//! Local tickets have no provider and therefore no source status.
use crate::{
    Result,
    lifecycle::{Transition, transition},
    source_time::parse as source_timestamp,
    store::{Store, conflict, id, parse},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

/// An external ticket provider Crewroom recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// Stored `source` kind.
    pub kind: &'static str,
    /// Operator-facing provider name used to label its status.
    pub name: &'static str,
}

pub const JIRA: Provider = Provider {
    kind: "jira",
    name: "Jira",
};
pub const GITHUB: Provider = Provider {
    kind: "github",
    name: "GitHub",
};
const PROVIDERS: [Provider; 2] = [JIRA, GITHUB];

/// Shown in place of a source status for a ticket with no external provider.
pub const NOT_LINKED: &str = "Not linked";

/// The stored source kind (`jira`, `github`, `local`, `evaluation`). Records
/// without one were created locally.
pub fn kind(task: &Value) -> &str {
    task["source"].as_str().unwrap_or("local")
}

/// The external provider a ticket was imported from, if any.
pub fn provider(task: &Value) -> Option<Provider> {
    let kind = kind(task);
    PROVIDERS.into_iter().find(|p| p.kind == kind)
}

/// The provider's own status name, exactly as last imported.
pub fn status(task: &Value) -> Option<&str> {
    provider(task).and(task["sourceStatus"].as_str())
}

/// "Jira In Progress" for an imported ticket; "Not linked" for a local one.
pub fn status_label(task: &Value) -> String {
    match provider(task) {
        Some(p) => format!("{} {}", p.name, status(task).unwrap_or("")),
        None => NOT_LINKED.into(),
    }
}

/// The status value alone, without the provider name ("Not linked" when local).
pub fn status_value(task: &Value) -> String {
    match provider(task) {
        Some(_) => status(task).unwrap_or("").into(),
        None => NOT_LINKED.into(),
    }
}

/// "Jira ticket" / "Local ticket".
pub fn ticket_label(task: &Value) -> String {
    match provider(task) {
        Some(p) => format!("{} ticket", p.name),
        None => "Local ticket".into(),
    }
}

/// Short kind tag shown beside the key in ticket lists ("jira", "local").
pub fn kind_tag(task: &Value) -> &'static str {
    provider(task).map_or("local", |p| p.kind)
}

/// Whether a ticket was imported from exactly this provider site. Delivery and
/// synchronization use this as their gate so a ticket is never written back to
/// a provider or site it did not come from.
pub fn belongs_to(task: &Value, provider: Provider, site: &Value) -> bool {
    task["source"] == provider.kind && !site.is_null() && task["sourceSite"] == *site
}

/// Whether newly observed source requirements differ from the stored ones.
/// Only title and description are requirements; status, priority and rank are
/// not, so they never force re-acceptance.
pub fn requirements_changed(previous: &Value, title: &str, description: &str) -> bool {
    previous["title"] != title || previous["description"] != description
}

/// One provider issue normalized to the neutral source model. Provider adapters
/// validate their own input before building it.
pub(crate) struct SourceObservation {
    pub id: Value,
    pub key: Value,
    pub title: String,
    pub description: String,
    pub status: Value,
    pub status_id: Option<Value>,
    pub priority: String,
    pub project: String,
    pub updated_at: Option<Value>,
    pub url: String,
}

impl Store {
    /// Save observed source tickets. Imported fields are replaced; refinements
    /// and steering are kept. A title/description change after the operator
    /// accepted requirements marks the ticket for re-acceptance and holds any
    /// open run, independent of which provider reported it.
    pub(crate) fn import_source_rows(
        &self,
        provider: Provider,
        site: &str,
        observations: &[SourceObservation],
    ) -> Result<Vec<Value>> {
        let mut saved = vec![];
        for (rank, issue) in observations.iter().enumerate() {
            let prior_id: Option<String> = self
                .db
                .query_row(
                    "SELECT id FROM tickets WHERE source_site=? AND source_id=?",
                    params![site, issue.id.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            let previous = prior_id
                .as_deref()
                .map(|id| self.get_task(id))
                .transpose()?;
            if let (Some(old), Some(new)) = (
                previous
                    .as_ref()
                    .and_then(|p| p["sourceUpdatedAt"].as_str()),
                issue.updated_at.as_ref().and_then(Value::as_str),
            ) && source_timestamp(new)
                .zip(source_timestamp(old))
                .is_some_and(|(new, old)| new < old)
            {
                return Err(conflict(&format!(
                    "{} returned an older issue observation.",
                    provider.name
                )));
            }
            let (title, description) = (issue.title.as_str(), issue.description.as_str());
            let changed = previous
                .as_ref()
                .is_some_and(|p| requirements_changed(p, title, description));
            let task_id = prior_id.unwrap_or_else(id);
            if changed && let Some(run) = self.open_run(&task_id)? {
                let runtime = self.runtime(run["id"].as_str().unwrap())?;
                let active = runtime["attempts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped");
                self.save_run(&transition(&run,&Transition::SourceChanged{reason:format!("{} requirements changed. Compare and accept the new requirements before continuing this run.",provider.name),active},self.clock_ms())?)?;
            }
            // Preserve the legacy field order too: adopted requirement messages
            // embed a serialized task snapshot in their immutable body.
            let mut source = json!({"key":issue.key,"title":title,"description":description,"sourceStatus":issue.status,"priority":issue.priority,"project":issue.project,"rank":rank});
            if let Some(value) = &issue.status_id {
                source["sourceStatusId"] = value.clone();
            }
            if let Some(value) = &issue.updated_at {
                source["sourceUpdatedAt"] = value.clone();
            }
            source["id"] = json!(task_id);
            source["source"] = json!(provider.kind);
            source["sourceSite"] = json!(site);
            source["sourceId"] = issue.id.clone();
            source["sourceUrl"] = json!(issue.url);
            source["sourceRevision"] = json!(
                previous
                    .as_ref()
                    .and_then(|p| p["sourceRevision"].as_u64())
                    .unwrap_or(0)
                    + u64::from(changed || previous.is_none())
            );
            source["updatedAt"] = json!(self.time());
            let mut refinement = if previous.is_some() {
                parse(self.db.query_row(
                    "SELECT refinement FROM tickets WHERE id=?",
                    [&task_id],
                    |r| r.get(0),
                )?)?
            } else {
                json!({"revision":1,"brief":"","criteria":[],"needsReacceptance":false})
            };
            if changed && !refinement["brief"].as_str().unwrap_or("").is_empty() {
                // Keep the text the operator last accepted so re-acceptance can show what
                // the source changed; later changes before re-acceptance keep the first baseline.
                if refinement["needsReacceptance"] != true
                    && let Some(p) = previous.as_ref()
                {
                    refinement["acceptedSource"] =
                        json!({"title":p["title"],"description":p["description"]});
                }
                refinement["needsReacceptance"] = json!(true);
            }
            self.db.execute("INSERT INTO tickets VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET imported=excluded.imported,refinement=excluded.refinement",params![task_id,site,issue.id.as_str(),source.to_string(),refinement.to_string()])?;
            saved.push(self.get_task(&task_id)?);
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_use_the_provider_name_and_local_tickets_stay_not_linked() {
        let jira = json!({"source":"jira","sourceStatus":"In Progress"});
        assert_eq!(status_label(&jira), "Jira In Progress");
        assert_eq!(status_value(&jira), "In Progress");
        assert_eq!(ticket_label(&jira), "Jira ticket");
        assert_eq!(kind_tag(&jira), "jira");
        let github = json!({"source":"github","sourceStatus":"open"});
        assert_eq!(status_label(&github), "GitHub open");
        assert_eq!(ticket_label(&github), "GitHub ticket");
        for local in [
            json!({"source":"local"}),
            json!({}),
            json!({"source":"evaluation"}),
        ] {
            assert_eq!(status_label(&local), NOT_LINKED);
            assert_eq!(status_value(&local), NOT_LINKED);
            assert_eq!(ticket_label(&local), "Local ticket");
            assert_eq!(kind_tag(&local), "local");
            assert_eq!(status(&local), None);
        }
        // A stray status on a local record is not presented as a provider fact.
        assert_eq!(
            status(&json!({"source":"local","sourceStatus":"Done"})),
            None
        );
    }

    #[test]
    fn delivery_gate_requires_the_same_provider_and_site() {
        let site = json!("https://fixture.atlassian.net");
        let task = json!({"source":"jira","sourceSite":"https://fixture.atlassian.net"});
        assert!(belongs_to(&task, JIRA, &site));
        assert!(!belongs_to(&task, GITHUB, &site));
        assert!(!belongs_to(
            &task,
            JIRA,
            &json!("https://other.atlassian.net")
        ));
        assert!(!belongs_to(&json!({"source":"local"}), JIRA, &Value::Null));
        assert!(!belongs_to(&json!({"source":"jira"}), JIRA, &Value::Null));
    }

    #[test]
    fn only_title_and_description_are_requirements() {
        let previous = json!({"title":"T","description":"D","sourceStatus":"Backlog"});
        assert!(!requirements_changed(&previous, "T", "D"));
        assert!(requirements_changed(&previous, "T2", "D"));
        assert!(requirements_changed(&previous, "T", "D2"));
    }
}
