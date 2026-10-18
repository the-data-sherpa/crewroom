//! GitHub Issues as a ticket source (FOR-45).
//!
//! The operator saves one GitHub repository (`owner/name`) with optional
//! label, assignee, milestone and state filters. A sync imports the plain
//! repository issues that match (never pull requests) through the
//! provider-neutral `import_source_rows`, so identity matching, re-acceptance
//! and preserved refinements behave exactly as they do for Jira. GitHub's
//! open/closed state is stored as the ticket's source status and nothing here
//! reads or writes Crewroom workflow state or agent activity. Transport and
//! credentials stay in the service; this module only validates and records.
use crate::{
    Result,
    source_time::parse as source_timestamp,
    store::{Store, conflict, invalid, strict, trim_text},
    ticket_source::{GITHUB, SourceObservation},
};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Most issues one sync may import. Larger sources must be narrowed by filter.
pub const MAX_ISSUES: usize = 1000;
const STATES: [&str; 3] = ["open", "closed", "all"];

/// Validates `owner/name` with GitHub's permitted characters.
pub fn parse_repository(value: &str) -> Option<(String, String)> {
    let (owner, name) = value.trim().split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (ok(owner) && ok(name)).then(|| (owner.to_owned(), name.to_owned()))
}

/// The ticket-source site for a repository. GitHub names are case-insensitive,
/// so the identity is lower-cased; the operator's spelling is kept for display.
pub fn site(repository: &str) -> String {
    format!("https://github.com/{}", repository.to_ascii_lowercase())
}

fn optional_text(body: &Value, key: &str, max: usize, label: &str) -> Result<Option<String>> {
    match &body[key] {
        Value::Null => Ok(None),
        Value::String(s) if trim_text(s).is_empty() => Ok(None),
        Value::String(s) if trim_text(s).chars().count() <= max => {
            Ok(Some(trim_text(s).to_owned()))
        }
        _ => Err(invalid(&format!(
            "{label} must be text of up to {max} characters."
        ))),
    }
}

/// Validates the saved source: repository and filters. Labels may be a list or
/// comma-separated text; every listed label must be present on an issue.
pub fn normalize_source(body: &Value) -> Result<Value> {
    strict(
        body,
        &["repository", "labels", "assignee", "milestone", "state"],
    )?;
    let repository = body["repository"]
        .as_str()
        .and_then(parse_repository)
        .map(|(owner, name)| format!("{owner}/{name}"))
        .ok_or_else(|| invalid("Enter the GitHub repository as owner/name."))?;
    let raw_labels: Vec<String> = match &body["labels"] {
        Value::Null => vec![],
        Value::String(s) => s.split(',').map(|l| trim_text(l).to_owned()).collect(),
        Value::Array(values) => values
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| trim_text(s).to_owned())
                    .ok_or_else(|| invalid("Labels must be text."))
            })
            .collect::<Result<_>>()?,
        _ => return Err(invalid("Labels must be text or a list of text.")),
    };
    let mut labels = vec![];
    let mut seen = HashSet::new();
    for label in raw_labels.into_iter().filter(|l| !l.is_empty()) {
        if label.chars().count() > 50 || label.contains(',') {
            return Err(invalid(
                "Each label must be up to 50 characters, without commas.",
            ));
        }
        if seen.insert(label.to_lowercase()) {
            labels.push(label);
        }
    }
    if labels.len() > 20 {
        return Err(invalid("Filter by at most 20 labels."));
    }
    let assignee = optional_text(body, "assignee", 39, "Assignee")?;
    if let Some(a) = &assignee
        && a != "*"
        && a != "none"
        && !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(invalid(
            "Assignee must be a GitHub login, * (anyone) or none (unassigned).",
        ));
    }
    let milestone = match &body["milestone"] {
        Value::Number(n) => Some(
            n.as_u64()
                .filter(|n| *n > 0)
                .map(|n| n.to_string())
                .ok_or_else(|| invalid("Milestone number must be positive."))?,
        ),
        _ => optional_text(body, "milestone", 255, "Milestone")?,
    };
    let state = match &body["state"] {
        Value::Null => "open".to_owned(),
        Value::String(s) if STATES.contains(&trim_text(s)) => trim_text(s).to_owned(),
        _ => return Err(invalid("State must be open, closed or all.")),
    };
    Ok(json!({
        "repository": repository,
        "labels": labels,
        "assignee": assignee,
        "milestone": milestone,
        "state": state,
    }))
}

fn text(value: &Value, min: usize, max: usize, label: &str) -> Result<String> {
    let value = value
        .as_str()
        .ok_or_else(|| invalid(&format!("GitHub issue {label} must be text.")))?;
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid(&format!(
            "GitHub issue {label} is outside its permitted size."
        )));
    }
    Ok(value.to_owned())
}

/// Validates one issue from `GET /repos/{owner}/{name}/issues` and normalizes it
/// to the neutral source observation. Pull requests are refused here as well as
/// filtered by the service, so one can never become a ticket.
fn observation(repository: &str, issue: &Value) -> Result<SourceObservation> {
    if !issue.is_object() {
        return Err(invalid("GitHub issue must be an object."));
    }
    if issue.get("pull_request").is_some_and(|v| !v.is_null()) {
        return Err(invalid("A pull request is not a GitHub issue ticket."));
    }
    let id = issue["id"]
        .as_u64()
        .ok_or_else(|| invalid("GitHub issue id must be a number."))?;
    let number = issue["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("GitHub issue number must be positive."))?;
    let title = trim_text(&text(&issue["title"], 1, 1000, "title")?).to_owned();
    if title.is_empty() {
        return Err(invalid("GitHub issue title is empty."));
    }
    let body = match &issue["body"] {
        Value::Null => String::new(),
        value => text(value, 0, 262_144, "body")?,
    };
    let state = text(&issue["state"], 1, 20, "state")?;
    if state != "open" && state != "closed" {
        return Err(invalid("GitHub issue state must be open or closed."));
    }
    let url = text(&issue["html_url"], 1, 2000, "URL")?;
    if !url.starts_with("https://github.com/") {
        return Err(invalid("GitHub issue URL must be on github.com."));
    }
    let updated_at = match &issue["updated_at"] {
        Value::Null => None,
        value => {
            let updated = text(value, 1, 64, "update time")?;
            if source_timestamp(&updated).is_none() {
                return Err(invalid("Invalid source timestamp."));
            }
            Some(json!(updated))
        }
    };
    Ok(SourceObservation {
        id: json!(id.to_string()),
        key: json!(format!("#{number}")),
        title,
        description: trim_text(&body).to_owned(),
        status: json!(state),
        status_id: None,
        priority: "Unspecified".into(),
        project: repository.to_owned(),
        updated_at,
        url,
    })
}

fn observations(repository: &str, issues: &Value, unique: bool) -> Result<Vec<SourceObservation>> {
    let issues = issues
        .as_array()
        .filter(|a| a.len() <= MAX_ISSUES)
        .ok_or_else(|| invalid("Invalid GitHub batch size."))?;
    let mut seen = HashSet::new();
    let mut normalized = vec![];
    for issue in issues {
        let observed = observation(repository, issue)?;
        if unique && !seen.insert(observed.id.to_string()) {
            return Err(invalid("The import contains duplicate issue IDs."));
        }
        normalized.push(observed);
    }
    Ok(normalized)
}

const UPSERT: &str =
    "INSERT INTO github_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data";

impl Store {
    /// The saved GitHub Issues source and its last sync result, if configured.
    pub fn github_sync_state(&self) -> Result<Option<Value>> {
        self.record("github_sync", "id", "1")
    }
    /// Saves the repository and filters. A changed source starts as not synced;
    /// tickets imported earlier keep their source, refinements and history.
    pub fn save_github_source(&self, body: &Value) -> Result<Value> {
        let source = normalize_source(body)?;
        self.transaction(|s| {
            if let Some(saved) = s.github_sync_state()?
                && saved["source"] == source
            {
                return Ok(saved);
            }
            let state =
                json!({"source":source,"state":"not_synced","taskIds":[],"savedAt":s.time()});
            s.db.execute(UPSERT, [state.to_string()])?;
            Ok(state)
        })
    }
    /// Commits one complete observation of the saved source atomically. `issues`
    /// matched the filters, in GitHub's order; `retained` are issues with open
    /// runs that no longer match but are re-read so their requirement changes
    /// are still noticed. Nothing is saved when the source changed meanwhile.
    pub fn commit_github_sync(
        &self,
        source: &Value,
        issues: &Value,
        retained: &Value,
    ) -> Result<Value> {
        let repository = source["repository"]
            .as_str()
            .ok_or_else(|| invalid("GitHub source has no repository."))?;
        let matched = observations(repository, issues, true)?;
        let kept = observations(repository, retained, true)?;
        let mut seen = HashSet::new();
        if matched
            .iter()
            .chain(&kept)
            .any(|i| !seen.insert(i.id.to_string()))
        {
            return Err(conflict("GitHub synchronization repeated an issue."));
        }
        let site = site(repository);
        self.transaction(|s| {
            let saved = s.github_sync_state()?;
            if saved.as_ref().map(|v| &v["source"]) != Some(source) {
                return Err(conflict(
                    "The GitHub source changed during synchronization. Nothing was saved; sync again.",
                ));
            }
            s.import_source_rows(GITHUB, &site, &kept)?;
            let tasks = s.import_source_rows(GITHUB, &site, &matched)?;
            let at = s.time();
            let mut state = saved.unwrap_or_default();
            state["state"] = json!("connected");
            state["taskIds"] = json!(tasks.iter().map(|t| t["id"].clone()).collect::<Vec<_>>());
            state["issueCount"] = json!(tasks.len());
            state["lastSyncedAt"] = json!(at);
            state["lastAttemptAt"] = json!(at);
            if let Some(fields) = state.as_object_mut() {
                fields.remove("error");
                fields.remove("reason");
            }
            s.db.execute(UPSERT, [state.to_string()])?;
            Ok(state)
        })
    }
    /// Records why a sync could not complete. Earlier imported tickets and the
    /// last complete sync time are kept; nothing partial is saved.
    pub fn fail_github_sync(&self, source: &Value, reason: &str, error: &str) -> Result<()> {
        self.transaction(|s| {
            let Some(mut state) = s.github_sync_state()?.filter(|v| v["source"] == *source) else {
                // The operator replaced the source meanwhile; the result is stale.
                return Ok(());
            };
            state["state"] = json!("unavailable");
            state["reason"] = json!(reason);
            state["error"] = json!(error);
            state["lastAttemptAt"] = json!(s.time());
            s.db.execute(UPSERT, [state.to_string()])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_filters_are_normalized_and_bad_values_refused() {
        let source = normalize_source(&json!({
            "repository":" octo-org/hello.world ",
            "labels":"bug, crewroom ,Bug,",
            "assignee":"octo-cat",
            "milestone":"Sprint 1",
        }))
        .unwrap();
        assert_eq!(
            source,
            json!({"repository":"octo-org/hello.world","labels":["bug","crewroom"],"assignee":"octo-cat","milestone":"Sprint 1","state":"open"})
        );
        let numbered = normalize_source(
            &json!({"repository":"o/r","milestone":3,"state":"all","assignee":"none"}),
        )
        .unwrap();
        assert_eq!(numbered["milestone"], "3");
        assert_eq!(numbered["assignee"], "none");
        for bad in [
            json!({}),
            json!({"repository":"owner"}),
            json!({"repository":"o/r","state":"merged"}),
            json!({"repository":"o/r","assignee":"a b"}),
            json!({"repository":"o/r","milestone":0}),
            json!({"repository":"o/r","labels":[1]}),
            json!({"repository":"o/r","extra":true}),
        ] {
            assert!(normalize_source(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn issues_normalize_to_the_neutral_model_and_pull_requests_are_refused() {
        let issue = json!({"id":901,"number":12,"title":" Fix it ","body":null,"state":"open","html_url":"https://github.com/o/r/issues/12","updated_at":"2026-10-18T08:00:00Z"});
        let observed = observation("o/r", &issue).unwrap();
        assert_eq!(observed.key, "#12");
        assert_eq!(observed.id, "901");
        assert_eq!(observed.title, "Fix it");
        assert_eq!(observed.description, "");
        assert_eq!(observed.status, "open");
        assert_eq!(observed.project, "o/r");
        let mut pull = issue.clone();
        pull["pull_request"] = json!({"url":"https://api.github.com/repos/o/r/pulls/12"});
        assert!(observation("o/r", &pull).is_err());
        let mut elsewhere = issue.clone();
        elsewhere["html_url"] = json!("https://example.com/o/r/issues/12");
        assert!(observation("o/r", &elsewhere).is_err());
        let mut merged = issue;
        merged["state"] = json!("merged");
        assert!(observation("o/r", &merged).is_err());
        assert_eq!(site("Octo-Org/Repo"), "https://github.com/octo-org/repo");
    }
}
