//! GitHub connection by personal access token (FOR-43).
//!
//! Connect validates a pasted token with `GET /user` before it is saved through
//! the encrypted install store. `CREWROOM_GITHUB_TOKEN` overrides a saved token
//! and is reported as the active source. The last check is kept in memory only,
//! bound to a SHA-256 fingerprint of the token it checked, so a replaced or
//! overridden token is shown as "not checked" instead of inheriting an older
//! result. No view, response or log carries the token or its fingerprint.
pub(crate) mod issues;

use crewroom_core::{Error, Result};
use crewroom_runtime::providers::{
    config::TokenEnvironment,
    github::{self, Account, GitHubClient, Outcome, RepositoryAccess},
    secrets::{self, Integration, SecretStore, StoredToken},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// How the service reaches GitHub and reads the environment override.
pub struct GitHubOptions {
    pub client: GitHubClient,
    pub environment: TokenEnvironment,
}
impl GitHubOptions {
    /// github.com with `CREWROOM_GITHUB_TOKEN` from the service environment.
    pub fn production() -> Result<Self> {
        Ok(Self {
            client: GitHubClient::new(None).map_err(|e| Error::Conflict(e.to_string()))?,
            environment: Arc::new(|| std::env::var(Integration::GitHub.environment()).ok()),
        })
    }
}

#[derive(Clone)]
enum Observed {
    Connected(Account),
    Rejected(String),
    Unavailable(String),
}
#[derive(Clone)]
struct RepositoryCheck {
    name: String,
    checked_at: String,
    outcome: Outcome<RepositoryAccess>,
}
#[derive(Clone)]
struct Check {
    fingerprint: [u8; 32],
    source: &'static str,
    checked_at: String,
    observed: Observed,
    repository: Option<RepositoryCheck>,
}

pub(crate) struct GitHub {
    client: GitHubClient,
    environment: TokenEnvironment,
    secrets: SecretStore,
    last: Mutex<Option<Check>>,
    /// Serializes GitHub Issues syncs (FOR-45).
    issue_sync: tokio::sync::Mutex<()>,
}

enum Active {
    None,
    Unreadable,
    Token(&'static str, Zeroizing<String>),
}

fn fingerprint(token: &str) -> [u8; 32] {
    Sha256::digest(token.trim().as_bytes()).into()
}
fn now() -> String {
    crewroom_core::timestamp(crewroom_core::now_ms())
}

impl GitHub {
    pub(crate) fn new(options: GitHubOptions, secrets: SecretStore) -> Self {
        Self {
            client: options.client,
            environment: options.environment,
            secrets,
            last: Mutex::new(None),
            issue_sync: tokio::sync::Mutex::new(()),
        }
    }
    pub(crate) fn environment(&self) -> Option<String> {
        (self.environment)().filter(|v| !v.is_empty())
    }
    fn active(&self) -> Active {
        if let Some(token) = self.environment() {
            return Active::Token("environment", Zeroizing::new(token));
        }
        match self.secrets.token(Integration::GitHub) {
            StoredToken::Present(token) => Active::Token("stored", token),
            StoredToken::Unreadable => Active::Unreadable,
            StoredToken::Missing => Active::None,
        }
    }
    fn record(&self, check: Check) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some(check);
        }
    }
    fn last(&self) -> Option<Check> {
        self.last.lock().ok().and_then(|l| l.clone())
    }

    /// Validates a pasted token and saves it encrypted only when GitHub accepts it.
    pub(crate) async fn connect(&self, body: Value) -> Result<()> {
        let fields = body
            .as_object()
            .ok_or_else(|| Error::Invalid("Send a token object.".into()))?;
        if fields
            .keys()
            .any(|k| k != "token" && k != "replaceUnreadable")
        {
            return Err(Error::Invalid(
                "Connect GitHub accepts only token and replaceUnreadable.".into(),
            ));
        }
        let token = Zeroizing::new(
            fields
                .get("token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| Error::Invalid("Paste a GitHub personal access token.".into()))?
                .to_owned(),
        );
        let replace = match fields.get("replaceUnreadable") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(Error::Invalid(
                    "replaceUnreadable must be a boolean.".into(),
                ));
            }
        };
        let account = match self.client.user(&token).await {
            Outcome::Accepted(account) => account,
            Outcome::Rejected { reason, .. } | Outcome::NoAccess { reason, .. } => {
                return Err(Error::Invalid(format!("Token not saved. {reason}")));
            }
            Outcome::Unavailable { reason, .. } => {
                return Err(Error::Conflict(format!(
                    "Token not saved: it could not be validated. {reason}"
                )));
            }
        };
        let secrets = self.secrets.clone();
        let saved = token.clone();
        tokio::task::spawn_blocking(move || secrets.save(Integration::GitHub, &saved, replace))
            .await
            .map_err(|_| Error::Conflict("Saving the GitHub token was interrupted.".into()))??;
        self.record(Check {
            fingerprint: fingerprint(&token),
            source: "stored",
            checked_at: now(),
            observed: Observed::Connected(account),
            repository: None,
        });
        Ok(())
    }

    /// Re-checks the active token, and optionally one repository's visibility.
    pub(crate) async fn check(&self, body: Value) -> Result<()> {
        let fields = body
            .as_object()
            .ok_or_else(|| Error::Invalid("Send a check object.".into()))?;
        if fields.keys().any(|k| k != "repository") {
            return Err(Error::Invalid("Check accepts only repository.".into()));
        }
        let repository = match fields.get("repository") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.trim().is_empty() => None,
            Some(Value::String(s)) => Some(
                github::parse_repository(s)
                    .ok_or_else(|| Error::Invalid("Enter the repository as owner/name.".into()))?,
            ),
            Some(_) => return Err(Error::Invalid("repository must be text.".into())),
        };
        let (source, token) = match self.active() {
            Active::Token(source, token) => (source, token),
            Active::Unreadable => {
                return Err(Error::Conflict(format!(
                    "GitHub: {}. Connect again with a token.",
                    secrets::UNREADABLE
                )));
            }
            Active::None => {
                return Err(Error::Conflict(
                    "GitHub is not connected. Paste a personal access token and choose Connect GitHub."
                        .into(),
                ));
            }
        };
        let observed = match self.client.user(&token).await {
            Outcome::Accepted(account) => Observed::Connected(account),
            Outcome::Rejected { reason, .. } | Outcome::NoAccess { reason, .. } => {
                Observed::Rejected(reason)
            }
            Outcome::Unavailable { reason, .. } => Observed::Unavailable(reason),
        };
        let repository = match (&observed, repository) {
            (Observed::Connected(_), Some((owner, name))) => {
                let outcome = self.client.repository(&token, &owner, &name).await;
                Some(RepositoryCheck {
                    name: format!("{owner}/{name}"),
                    checked_at: now(),
                    outcome,
                })
            }
            _ => None,
        };
        self.record(Check {
            fingerprint: fingerprint(&token),
            source,
            checked_at: now(),
            observed,
            repository,
        });
        Ok(())
    }

    /// Removes the saved token. An environment override is unchanged.
    pub(crate) async fn disconnect(&self, body: Value) -> Result<()> {
        if body != json!({}) {
            return Err(Error::Invalid("Disconnect accepts no fields.".into()));
        }
        let secrets = self.secrets.clone();
        tokio::task::spawn_blocking(move || secrets.clear(Integration::GitHub))
            .await
            .map_err(|_| Error::Conflict("Disconnecting GitHub was interrupted.".into()))??;
        if let Ok(mut last) = self.last.lock()
            && last.as_ref().is_some_and(|c| c.source == "stored")
        {
            *last = None;
        }
        Ok(())
    }

    /// Public connection facts for Settings. Never the token or its fingerprint.
    pub(crate) fn view(&self) -> Value {
        let environment = self.environment();
        let credential = self
            .secrets
            .status(Integration::GitHub, environment.as_deref());
        let (source, token) = match self.active() {
            Active::None => {
                return json!({
                    "state": "disconnected",
                    "label": "Disconnected. Paste a personal access token to connect GitHub.",
                    "source": "none",
                    "credential": credential,
                });
            }
            Active::Unreadable => {
                return json!({
                    "state": "unreadable",
                    "label": format!("GitHub: {}", secrets::UNREADABLE),
                    "source": "unreadable",
                    "credential": credential,
                });
            }
            Active::Token(source, token) => (source, token),
        };
        let kind = github::token_kind(&token);
        let Some(check) = self.last().filter(|c| c.fingerprint == fingerprint(&token)) else {
            return json!({
                "state": "unchecked",
                "label": if source == "environment" {
                    "Using CREWROOM_GITHUB_TOKEN; not checked with GitHub since the service started. Choose Check connection."
                } else {
                    "Saved token not checked with GitHub since the service started. Choose Check connection."
                },
                "source": source,
                "tokenKind": kind,
                "credential": credential,
            });
        };
        let mut view = json!({
            "source": source,
            "tokenKind": kind,
            "checkedAt": check.checked_at,
            "credential": credential,
        });
        match &check.observed {
            Observed::Connected(account) => {
                view["state"] = json!("connected");
                view["label"] = json!(format!("Connected as @{}.", account.login));
                view["account"] = account.view();
                view["notes"] = json!(notes(kind, account));
            }
            Observed::Rejected(reason) => {
                view["state"] = json!("rejected");
                view["label"] = json!(format!(
                    "Expired or revoked: {reason} Connect again with a new token."
                ));
            }
            Observed::Unavailable(reason) => {
                view["state"] = json!("unavailable");
                view["label"] = json!(format!(
                    "Connection not confirmed: {reason} The token was not changed."
                ));
            }
        }
        if let Some(repository) = &check.repository {
            view["repository"] = repository_view(repository);
        }
        view
    }
}

fn notes(kind: &str, account: &Account) -> Vec<String> {
    let mut notes = Vec::new();
    match (kind, &account.scopes) {
        (_, Some(scopes)) => {
            if !scopes.iter().any(|s| s == "repo") {
                notes.push(if scopes.iter().any(|s| s == "public_repo") {
                    "Classic token has public_repo only: private repositories are not reachable.".to_owned()
                } else {
                    "Classic token lacks the repo scope: Crewroom cannot read issues or open pull requests with it.".to_owned()
                });
            }
        }
        ("fine-grained", None) => notes.push(
            "GitHub does not report a fine-grained token's permissions. Issues, pull requests and contents access are confirmed when Crewroom first uses them.".into(),
        ),
        _ => {}
    }
    if let Some(expires) = &account.expires_at {
        notes.push(format!("Token expires {expires}."));
    }
    notes
}

fn repository_view(check: &RepositoryCheck) -> Value {
    let name = &check.name;
    let (access, label, extra) = match &check.outcome {
        Outcome::Accepted(repository) => {
            let push = repository.permissions["push"] == true;
            (
                "granted",
                format!(
                    "This token can see {}{}.",
                    repository.full_name,
                    if push {
                        " (your account can push)"
                    } else {
                        " (your account cannot push)"
                    }
                ),
                json!({"private": repository.private, "permissions": repository.permissions}),
            )
        }
        Outcome::NoAccess { status, .. } => (
            "none",
            format!(
                "No access to {name} (HTTP {status}). The repository does not exist or this token cannot see it; for a fine-grained token, add it under Repository access."
            ),
            json!({}),
        ),
        Outcome::Rejected { reason, .. } => ("rejected", reason.clone(), json!({})),
        Outcome::Unavailable { reason, .. } => (
            "unavailable",
            format!("Access to {name} not confirmed: {reason}"),
            json!({}),
        ),
    };
    let mut view = json!({
        "name": name,
        "access": access,
        "label": label,
        "checkedAt": check.checked_at,
    });
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            view[key] = value.clone();
        }
    }
    view
}
