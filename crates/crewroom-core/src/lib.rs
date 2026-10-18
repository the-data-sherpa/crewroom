//! Crewroom's durable workflow authority. GUI code must use the service API.
mod active_time;
mod attempts;
mod runner_identity;
pub use runner_identity::{runner_kind, verify_runner_identity};
pub mod authority;
mod briefs;
pub use briefs::snapshot_briefs;
pub mod choreography;
pub mod comparisons;
mod continuations;
mod dependencies;
mod feedback_resume;
pub use feedback_resume::feedback_awaits_resume;
pub mod github_issues;
pub mod intake;
pub mod jira;
mod launch;
pub use launch::parse_team;
pub mod learning;
pub mod lifecycle;
mod messages;
mod migrations;
pub mod model_jobs;
mod observations;
pub use observations::compact_observation;
mod outcomes;
pub mod playbooks;
pub mod principles;
pub mod project_source;
mod provider_records;
pub mod repositories;
mod repository_work;
mod requirements;
pub use requirements::requirements_hash;
pub mod research;
pub mod role_drafts;
pub mod role_history;
pub mod roles;
mod scheduling;
pub mod serialized;
pub mod source_time;
pub mod store;
mod target_recovery;
mod teams;
pub mod ticket_source;
pub use teams::insertion_request as parse_insertion_request;
mod workflow;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    RepositoryConfigurationChanged(String),
    #[error("{message}")]
    FeedbackResume { message: String, terminal: bool },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Untagged historical runs are coding; explicit unknown or malformed purposes
/// never inherit coding authority.
pub fn run_purpose(value: &serde_json::Value) -> Result<&str> {
    match value.get("purpose") {
        None => Ok("coding"),
        Some(serde_json::Value::String(purpose))
            if matches!(
                purpose.as_str(),
                "coding" | "research" | "explore" | "grill" | "evaluation"
            ) =>
        {
            Ok(purpose)
        }
        _ => Err(Error::Invalid("Unknown or malformed run purpose.".into())),
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub fn timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .expect("supported timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
