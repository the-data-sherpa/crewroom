//! A project's ticket source, suggested from its repository's publish remote
//! (FOR-47).
//!
//! A project is a configured repository. When it is linked, the remote it
//! publishes to is read from Git and classified:
//!
//! * `github.com/<owner>/<repo>` suggests GitHub Issues for tickets and GitHub
//!   for pull requests, with `owner/repo` prefilled;
//! * `bitbucket.org/...` suggests Jira for tickets and Bitbucket for pull
//!   requests; the Jira site cannot be inferred, so the operator picks it;
//! * anything else (another host, an unrecognised or missing remote) leaves
//!   the choice to the operator.
//!
//! Nothing is saved until the operator confirms once. The confirmation records
//! the remote it was made against, so a later remote that no longer matches is
//! reported as a mismatch; the saved source is never switched automatically.
//! Changing a saved source is refused while queued or open work references the
//! repository. Imported tickets carry their own source identity, so a change
//! never rewrites them or their history.
//!
//! Remote addresses carrying credentials, queries or fragments are never
//! classified, stored or displayed.
use crate::{
    Result,
    github_issues::parse_repository,
    jira::normalize_site,
    repositories::git,
    store::{Store, conflict, invalid, strict},
};
use rusqlite::params;
use serde_json::{Value, json};
use std::path::Path;

pub const GITHUB_HOST: &str = "github.com";
pub const BITBUCKET_HOST: &str = "bitbucket.org";

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
}

/// Splits a remote into its lowercase host and repository path (without
/// `.git`). Only `https://`, `ssh://` and scp-like `user@host:path` addresses
/// are accepted; remotes carrying credentials, queries or fragments are
/// rejected so that no secret is ever classified, stored or shown.
pub fn split_remote(remote: &str) -> Option<(String, String)> {
    if remote.is_empty()
        || remote.len() > 2048
        || remote.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let (host, path) = if remote.contains("://") {
        let url = url::Url::parse(remote).ok()?;
        let https = match url.scheme() {
            "https" => true,
            "ssh" => false,
            _ => return None,
        };
        if url.password().is_some()
            || (https && !url.username().is_empty())
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let host = match url.host()? {
            url::Host::Domain(host) => host.to_ascii_lowercase(),
            _ => return None,
        };
        (host, url.path().to_owned())
    } else {
        // scp-like `user@host:owner/name`; a `/` before `:` is a local path.
        let (authority, path) = remote.split_once(':')?;
        if authority.contains('/') || path.starts_with('/') {
            return None;
        }
        let (user, host) = authority.split_once('@')?;
        if user.is_empty() || user.contains('@') {
            return None;
        }
        (host.to_ascii_lowercase(), path.to_owned())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if !valid_host(&host)
        || path.is_empty()
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return None;
    }
    Some((host, path.into()))
}

/// The name of the remote a repository publishes its target branch to: the
/// remote named by a `remote/branch` target, the branch's push remote, the
/// default push remote, the branch's upstream remote, `origin`, or the only
/// remote. `None` when the checkout has no usable remote.
fn publish_remote_name(path: &Path, target: &str) -> Option<String> {
    let remotes: Vec<String> = git(path, &["remote"])
        .ok()?
        .lines()
        .map(str::to_owned)
        .filter(|r| !r.is_empty() && !r.starts_with('-'))
        .collect();
    let known = |name: &str| remotes.iter().any(|r| r == name);
    if let Some((remote, _)) = target.split_once('/')
        && known(remote)
    {
        return Some(remote.to_owned());
    }
    for key in [
        format!("branch.{target}.pushRemote"),
        "remote.pushDefault".to_owned(),
        format!("branch.{target}.remote"),
    ] {
        if let Ok(name) = git(path, &["config", "--get", &key])
            && known(&name)
        {
            return Some(name);
        }
    }
    if known("origin") {
        return Some("origin".into());
    }
    (remotes.len() == 1).then(|| remotes[0].clone())
}

/// Reads the address a configured repository publishes to. Runs Git; call it
/// on a blocking worker. `None` when no remote can be read.
pub fn read_remote(repository: &Value) -> Option<String> {
    let path = Path::new(repository["path"].as_str()?);
    let target = repository["targetBranch"].as_str().unwrap_or("");
    let name = publish_remote_name(path, target)?;
    git(path, &["remote", "get-url", "--push", &name])
        .ok()
        .filter(|url| !url.is_empty())
}

/// Classifies a publish remote. The result never contains the raw address.
pub fn observe(remote: Option<&str>) -> Value {
    let Some(remote) = remote else {
        return json!({"state":"missing","label":"no readable publish remote"});
    };
    let Some((host, path)) = split_remote(remote) else {
        return json!({"state":"unrecognised","label":"an unrecognised publish remote"});
    };
    let repository = parse_repository(&path).map(|(owner, name)| format!("{owner}/{name}"));
    match (host.as_str(), repository) {
        (GITHUB_HOST, Some(repository)) => json!({
            "state":"github","host":host,"repository":repository,
            "label":format!("{host}/{repository}"),
        }),
        (BITBUCKET_HOST, Some(repository)) => json!({
            "state":"bitbucket","host":host,"repository":repository,
            "label":format!("{host}/{repository}"),
        }),
        _ => json!({"state":"other","host":host,"label":host}),
    }
}

/// The pull-request host implied by an observed remote, if Crewroom has one.
pub fn pull_requests(observed: &Value) -> Value {
    match observed["state"].as_str() {
        Some("github") => json!("github"),
        Some("bitbucket") => json!("bitbucket"),
        _ => Value::Null,
    }
}

/// The source suggested for an observed remote. `tickets` is null when the
/// operator must choose; `needsSite` is true when a Jira site must be picked.
pub fn suggestion(observed: &Value) -> Value {
    match observed["state"].as_str() {
        Some("github") => json!({
            "tickets":"github","repository":observed["repository"],
            "pullRequests":"github","needsSite":false,
        }),
        Some("bitbucket") => json!({
            "tickets":"jira","pullRequests":"bitbucket","needsSite":true,
        }),
        _ => json!({"tickets":null,"pullRequests":null,"needsSite":false}),
    }
}

/// Validates the operator's confirmed choice: GitHub Issues with `owner/repo`,
/// or Jira with a site.
pub fn normalize_choice(body: &Value) -> Result<Value> {
    strict(body, &["tickets", "repository", "site"])?;
    match body["tickets"].as_str() {
        Some("github") => {
            if !body["site"].is_null() {
                return Err(invalid("A GitHub Issues source has no Jira site."));
            }
            let repository = body["repository"]
                .as_str()
                .and_then(parse_repository)
                .map(|(owner, name)| format!("{owner}/{name}"))
                .ok_or_else(|| invalid("Enter the GitHub repository as owner/name."))?;
            Ok(json!({"tickets":"github","repository":repository}))
        }
        Some("jira") => {
            if !body["repository"].is_null() {
                return Err(invalid("A Jira source has no GitHub repository."));
            }
            if body["site"].as_str().is_none_or(|s| s.trim().is_empty()) {
                return Err(invalid(
                    "Pick the Jira site; it cannot be inferred from the remote.",
                ));
            }
            Ok(json!({"tickets":"jira","site":normalize_site(&body["site"])?}))
        }
        _ => Err(invalid(
            "Choose GitHub Issues or Jira as the ticket source.",
        )),
    }
}

/// Identity of an observed remote for mismatch checks; GitHub and Bitbucket
/// names are case-insensitive.
fn remote_key(observed: &Value) -> (String, String, String) {
    let field = |key: &str| observed[key].as_str().unwrap_or("").to_ascii_lowercase();
    (field("state"), field("host"), field("repository"))
}

fn same_source(saved: &Value, choice: &Value) -> bool {
    let field = |v: &Value, key: &str| v[key].as_str().unwrap_or("").to_ascii_lowercase();
    ["tickets", "repository", "site"]
        .iter()
        .all(|key| field(saved, key) == field(choice, key))
}

/// Whether a saved source still matches the observed remote: the remote it was
/// confirmed against must be the one the repository publishes to now.
pub fn matches(saved: &Value, observed: &Value) -> bool {
    remote_key(&saved["remote"]) == remote_key(observed)
}

/// Keys of queued or open work that references a repository. Matches the
/// store's `referencing_runs` used when a change is committed.
pub fn blocking_keys(runs: &[Value], repository_id: &str) -> Vec<String> {
    let mut keys: Vec<String> = vec![];
    for run in runs {
        if ["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""))
            || run["taskSnapshot"]["repositoryId"] != repository_id
        {
            continue;
        }
        let key = run["taskSnapshot"]["key"]
            .as_str()
            .or_else(|| run["taskSnapshot"]["title"].as_str())
            .unwrap_or("a run")
            .to_owned();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Jira sites the operator can pick from: those of imported Jira tickets, of
/// other projects' saved sources, and of the configured Jira refresh source.
pub fn jira_sites(workspace: &Value) -> Vec<String> {
    let mut sites: Vec<String> = vec![];
    let mut add = |site: Option<&str>| {
        if let Some(site) = site.and_then(|s| normalize_site(&json!(s)).ok())
            && !sites.contains(&site)
        {
            sites.push(site);
        }
    };
    for task in workspace["tasks"].as_array().into_iter().flatten() {
        if task["source"] == "jira" {
            add(task["sourceSite"].as_str());
        }
    }
    for repository in workspace["repositories"].as_array().into_iter().flatten() {
        if repository["ticketSource"]["tickets"] == "jira" {
            add(repository["ticketSource"]["site"].as_str());
        }
    }
    add(workspace["jira"]["source"]["siteUrl"].as_str());
    sites.sort();
    sites
}

fn pull_request_name(value: &Value) -> &'static str {
    match value.as_str() {
        Some("github") => "GitHub",
        Some("bitbucket") => "Bitbucket",
        _ => "unavailable for this remote",
    }
}

fn source_label(saved: &Value) -> String {
    let tickets = match saved["tickets"].as_str() {
        Some("github") => format!(
            "GitHub Issues {}",
            saved["repository"].as_str().unwrap_or("")
        ),
        _ => format!("Jira {}", saved["site"].as_str().unwrap_or("")),
    };
    format!(
        "{tickets} · pull requests: {}",
        pull_request_name(&saved["pullRequests"])
    )
}

/// The operator-facing projection for one repository. `observed` is the
/// current remote classification, `blockers` the keys of work that prevents a
/// change, and `sites` the Jira sites to offer.
pub fn view(repository: &Value, observed: &Value, blockers: &[String], sites: &[String]) -> Value {
    let saved = repository.get("ticketSource").filter(|v| v.is_object());
    let remote = observed["label"].as_str().unwrap_or("");
    let suggested = suggestion(observed);
    let (state, label, warning) = match saved {
        None => {
            let label = match observed["state"].as_str() {
                Some("github") => format!(
                    "Suggested from {remote}: GitHub Issues ({}) for tickets and GitHub for pull requests. Confirm to save.",
                    observed["repository"].as_str().unwrap_or("")
                ),
                Some("bitbucket") => format!(
                    "Suggested from {remote}: Jira for tickets and Bitbucket for pull requests. Pick the Jira site; it cannot be inferred from the remote."
                ),
                Some("other") => format!(
                    "The repository publishes to {remote}, which has no suggested ticket source. Choose GitHub Issues or Jira."
                ),
                _ => format!(
                    "Crewroom found {remote}, so no source can be suggested. Choose GitHub Issues or Jira."
                ),
            };
            ("unconfirmed", label, None)
        }
        Some(saved) => {
            let label = format!("Ticket source: {}.", source_label(saved));
            if matches(saved, observed) {
                ("confirmed", label, None)
            } else {
                let confirmed = saved["remote"]["label"].as_str().unwrap_or("its remote");
                (
                    "mismatch",
                    label,
                    Some(format!(
                        "Source no longer matches remote: it was confirmed for {confirmed}, and the repository now has {remote}. Nothing was changed; confirm a new source to switch."
                    )),
                )
            }
        }
    };
    let change_blocked = (saved.is_some() && !blockers.is_empty()).then(|| {
        format!(
            "Changing the ticket source is blocked while queued or open work references this repository: {}. Finish or cancel that work first.",
            blockers.join(", ")
        )
    });
    json!({
        "state": state,
        "label": label,
        "warning": warning,
        "remote": observed,
        "suggestion": suggested,
        "saved": saved,
        "jiraSites": sites,
        "changeBlocked": change_blocked,
    })
}

impl Store {
    /// A configured repository record, for reading its remote before a confirmation.
    pub fn configured_repository(&self, id: &str) -> Result<Value> {
        self.repository_by_id(id)
    }

    /// Saves the operator's confirmed ticket source for a repository. `observed`
    /// is the remote classification read just before this call; it is stored
    /// so a later remote change is reported, not acted on. Confirming the same
    /// source against the same remote changes nothing. Any other change to a
    /// saved source is refused while queued or open work references the
    /// repository. Imported tickets are never rewritten.
    pub fn confirm_ticket_source(&self, id: &str, body: &Value, observed: &Value) -> Result<Value> {
        let choice = normalize_choice(body)?;
        self.transaction(|s| {
            let mut repository = s.repository_by_id(id)?;
            if repository.get("retiredAt").is_some() {
                return Err(conflict(
                    "Retired repositories cannot change their ticket source.",
                ));
            }
            let saved = repository
                .get("ticketSource")
                .filter(|v| v.is_object())
                .cloned();
            if let Some(saved) = &saved {
                if same_source(saved, &choice) && matches(saved, observed) {
                    return Ok(repository);
                }
                s.refuse_referenced(&s.referencing_runs(id)?, "change the ticket source")?;
            }
            let mut record = choice;
            record["pullRequests"] = pull_requests(observed);
            record["remote"] = observed.clone();
            record["confirmedAt"] = json!(s.time());
            if let Some(saved) = saved {
                let mut history = repository["ticketSourceHistory"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                history.push(saved);
                repository["ticketSourceHistory"] = json!(history);
            }
            // The confirmed source drives this project's sync (FOR-45).
            s.attach_github_source(id, &record)?;
            repository["ticketSource"] = record;
            s.db.execute(
                "UPDATE repositories SET data=? WHERE id=?",
                params![repository.to_string(), id],
            )?;
            Ok(repository)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_are_classified_without_keeping_the_address() {
        for remote in [
            "https://github.com/Octo-Fixture/Sandbox.git",
            "git@github.com:Octo-Fixture/Sandbox.git",
            "ssh://git@github.com/Octo-Fixture/Sandbox",
        ] {
            assert_eq!(
                observe(Some(remote)),
                json!({"state":"github","host":"github.com","repository":"Octo-Fixture/Sandbox","label":"github.com/Octo-Fixture/Sandbox"}),
                "{remote}"
            );
        }
        assert_eq!(
            observe(Some("git@bitbucket.org:fixture/repository.git"))["state"],
            "bitbucket"
        );
        assert_eq!(
            observe(Some("https://gitlab.example/group/sub/repo.git")),
            json!({"state":"other","host":"gitlab.example","label":"gitlab.example"})
        );
        // GitHub Enterprise and nested paths are not github.com repositories.
        assert_eq!(
            observe(Some("git@github.example.com:o/r.git"))["state"],
            "other"
        );
        assert_eq!(
            observe(Some("https://github.com/o/group/r"))["state"],
            "other"
        );
        let credentialed = observe(Some("https://user:synthetic-secret@github.com/o/r.git"));
        assert_eq!(credentialed["state"], "unrecognised");
        assert!(!credentialed.to_string().contains("synthetic-secret"));
        assert_eq!(observe(Some("/srv/git/repo.git"))["state"], "unrecognised");
        assert_eq!(observe(None)["state"], "missing");
    }

    #[test]
    fn suggestions_follow_the_remote_host() {
        let github = suggestion(&observe(Some("git@github.com:o/r.git")));
        assert_eq!(github["tickets"], "github");
        assert_eq!(github["repository"], "o/r");
        assert_eq!(github["pullRequests"], "github");
        let bitbucket = suggestion(&observe(Some("git@bitbucket.org:w/s.git")));
        assert_eq!(bitbucket["tickets"], "jira");
        assert_eq!(bitbucket["pullRequests"], "bitbucket");
        assert_eq!(bitbucket["needsSite"], true);
        assert!(bitbucket.get("site").is_none());
        for remote in [Some("https://gitlab.example/o/r"), None] {
            let other = suggestion(&observe(remote));
            assert!(other["tickets"].is_null());
            assert!(other["pullRequests"].is_null());
        }
    }

    #[test]
    fn choices_are_validated() {
        assert_eq!(
            normalize_choice(&json!({"tickets":"github","repository":" o/r "})).unwrap(),
            json!({"tickets":"github","repository":"o/r"})
        );
        assert_eq!(
            normalize_choice(&json!({"tickets":"jira","site":"https://fixture.atlassian.net/"}))
                .unwrap(),
            json!({"tickets":"jira","site":"https://fixture.atlassian.net"})
        );
        for bad in [
            json!({}),
            json!({"tickets":"local"}),
            json!({"tickets":"github"}),
            json!({"tickets":"github","repository":"o/r","site":"https://x.example"}),
            json!({"tickets":"jira"}),
            json!({"tickets":"jira","site":"http://insecure.example"}),
            json!({"tickets":"jira","site":"https://x.example","repository":"o/r"}),
            json!({"tickets":"github","repository":"o/r","extra":1}),
        ] {
            assert!(normalize_choice(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn view_states_mismatch_and_blockers() {
        let github = observe(Some("git@github.com:o/r.git"));
        let fresh = view(&json!({"id":"r1"}), &github, &[], &[]);
        assert_eq!(fresh["state"], "unconfirmed");
        assert!(fresh["label"].as_str().unwrap().contains("Confirm to save"));
        let saved =
            json!({"tickets":"github","repository":"o/r","pullRequests":"github","remote":github});
        let repository = json!({"id":"r1","ticketSource":saved});
        let confirmed = view(
            &repository,
            &observe(Some("https://github.com/O/R")),
            &[],
            &[],
        );
        assert_eq!(confirmed["state"], "confirmed");
        assert!(confirmed["warning"].is_null());
        let moved = view(
            &repository,
            &observe(Some("git@bitbucket.org:w/s.git")),
            &["FOR-1".into()],
            &[],
        );
        assert_eq!(moved["state"], "mismatch");
        let warning = moved["warning"].as_str().unwrap();
        assert!(warning.starts_with("Source no longer matches remote"));
        assert!(warning.contains("github.com/o/r") && warning.contains("bitbucket.org/w/s"));
        // The saved source is reported unchanged beside the new suggestion.
        assert_eq!(moved["saved"]["tickets"], "github");
        assert_eq!(moved["suggestion"]["tickets"], "jira");
        assert!(moved["changeBlocked"].as_str().unwrap().contains("FOR-1"));
    }

    #[test]
    fn jira_sites_are_collected_once() {
        let workspace = json!({
            "tasks":[
                {"source":"jira","sourceSite":"https://b.example"},
                {"source":"jira","sourceSite":"https://b.example"},
                {"source":"github","sourceSite":"https://github.com/o/r"},
            ],
            "repositories":[{"ticketSource":{"tickets":"jira","site":"https://a.example"}}],
            "jira":{"source":{"siteUrl":"https://c.example"}},
        });
        assert_eq!(
            jira_sites(&workspace),
            [
                "https://a.example",
                "https://b.example",
                "https://c.example"
            ]
        );
    }
}
