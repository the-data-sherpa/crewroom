//! A project's ticket source suggested from its repository remote (FOR-47).
//!
//! The workspace projects, for every active repository, the remote it
//! publishes to (classified, never the raw address), the suggested source, the
//! confirmed source, a "source no longer matches remote" warning and whether
//! a change is blocked by queued or open work. Reading the remote runs Git on
//! a blocking worker. `POST /api/repositories/{id}/ticket-source` is the one
//! explicit confirmation; the remote is read again at that moment and saved
//! with the choice. Nothing here switches a source on its own.
use crate::{ApiError, AppState, operation};
use crewroom_core::{Error, project_source};
use serde_json::{Value, json};

async fn observe(repositories: Vec<Value>) -> Result<Vec<Option<Value>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        repositories
            .iter()
            .map(|r| {
                r.get("retiredAt")
                    .is_none()
                    .then(|| project_source::observe(project_source::read_remote(r).as_deref()))
            })
            .collect()
    })
    .await
    .map_err(|_| {
        ApiError::from(Error::Conflict(
            "Reading repository remotes was interrupted.".into(),
        ))
    })
}

/// Adds `projectSource` to each active repository. `runs` are all of the
/// workspace's runs, so work that blocks a change is never hidden by a view
/// filter.
pub(crate) async fn project(value: &mut Value, runs: &[Value]) -> Result<(), ApiError> {
    let repositories = value["repositories"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let observed = observe(repositories).await?;
    let sites = project_source::jira_sites(value);
    if let Some(repositories) = value["repositories"].as_array_mut() {
        for (repository, observed) in repositories.iter_mut().zip(observed) {
            let Some(observed) = observed else { continue };
            let blockers =
                project_source::blocking_keys(runs, repository["id"].as_str().unwrap_or(""));
            repository["projectSource"] =
                project_source::view(repository, &observed, &blockers, &sites);
        }
    }
    Ok(())
}

/// Saves the operator's confirmed source against the remote read now.
pub(crate) async fn confirm(state: &AppState, id: &str, body: Value) -> Result<Value, ApiError> {
    let id = id.to_owned();
    let lookup = id.clone();
    let repository = operation(state, move |s| s.store.configured_repository(&lookup)).await?;
    let observed = observe(vec![repository])
        .await?
        .pop()
        .flatten()
        .ok_or_else(|| {
            ApiError::from(Error::Conflict(
                "Retired repositories cannot change their ticket source.".into(),
            ))
        })?;
    let saved = operation(state, move |s| {
        s.store.confirm_ticket_source(&id, &body, &observed)
    })
    .await?;
    state.changes.send_modify(|v| *v += 1);
    Ok(json!({"repository": saved}))
}
