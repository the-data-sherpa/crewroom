//! GitHub personal-access-token checks against the fixed github.com API origin.
//!
//! The token is passed in by the caller for one request and never logged,
//! serialized or returned. Results distinguish a token GitHub rejected
//! (invalid, expired or revoked) from GitHub being unreachable, so Settings can
//! say which one happened instead of treating every failure as "disconnected".
pub mod issues;
use super::transport::fixture_origin;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::{Value, json};
use std::time::Duration;

pub const API_ORIGIN: &str = "https://api.github.com";
const MAX_BODY: usize = 1024 * 1024;

/// The account a token authenticates as. Public profile facts only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    pub id: u64,
    pub name: Option<String>,
    pub html_url: Option<String>,
    /// From `X-OAuth-Scopes`; present for classic tokens only.
    pub scopes: Option<Vec<String>>,
    /// From `GitHub-Authentication-Token-Expiration`, when the token expires.
    pub expires_at: Option<String>,
}
impl Account {
    pub fn view(&self) -> Value {
        json!({
            "login": self.login,
            "id": self.id,
            "name": self.name,
            "htmlUrl": self.html_url,
            "scopes": self.scopes,
            "expiresAt": self.expires_at,
        })
    }
}

/// One observed answer from GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    Accepted(T),
    /// GitHub answered and refused the token (HTTP 401): invalid, expired or revoked.
    Rejected {
        status: u16,
        reason: String,
    },
    /// GitHub answered with no access to the resource (403/404 that is not a
    /// rate limit). For a repository, fine-grained tokens see 404 when the
    /// repository was not selected.
    NoAccess {
        status: u16,
        reason: String,
    },
    /// No usable answer: network failure, rate limit, timeout, or 5xx.
    Unavailable {
        reason: String,
        request_sent: bool,
    },
}

/// Repository facts visible to the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryAccess {
    pub full_name: String,
    pub private: bool,
    /// The account's role-derived permissions as GitHub reports them
    /// (`pull`, `push`, ...). For fine-grained tokens this reflects the user's
    /// role, not the token's narrower grant.
    pub permissions: Value,
}

/// The kind of token, from its documented prefix. Never echoes the value.
pub fn token_kind(token: &str) -> &'static str {
    if token.starts_with("github_pat_") {
        "fine-grained"
    } else if token.starts_with("ghp_") {
        "classic"
    } else {
        "other"
    }
}

/// Validates `owner/name` before it becomes part of an API path.
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

#[derive(Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    base: String,
}
impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("base", &self.base)
            .finish()
    }
}
impl GitHubClient {
    /// `fixture` overrides the github.com origin with a loopback test server only.
    pub fn new(fixture: Option<&str>) -> super::Result<Self> {
        let base = match fixture {
            Some(value) => fixture_origin(value, super::Provider::GitHub)?,
            None => API_ORIGIN.to_owned(),
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| super::unavailable("GitHub client could not be created."))?;
        Ok(Self { client, base })
    }
    pub fn origin(&self) -> &str {
        &self.base
    }
    /// `GET /user`: who the token authenticates as.
    pub async fn user(&self, token: &str) -> Outcome<Account> {
        match self.get(token, "/user").await {
            Ok((headers, body)) => {
                let Some(login) = body["login"].as_str().filter(|s| !s.is_empty()) else {
                    return invalid_response();
                };
                let text = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                };
                Outcome::Accepted(Account {
                    login: login.to_owned(),
                    id: body["id"].as_u64().unwrap_or(0),
                    name: body["name"].as_str().map(str::to_owned),
                    html_url: body["html_url"].as_str().map(str::to_owned),
                    scopes: headers.get("x-oauth-scopes").map(|v| {
                        v.to_str()
                            .unwrap_or("")
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect()
                    }),
                    expires_at: text("github-authentication-token-expiration"),
                })
            }
            Err(outcome) => outcome,
        }
    }
    /// `GET /repos/{owner}/{name}`: whether the token can see the repository.
    pub async fn repository(
        &self,
        token: &str,
        owner: &str,
        name: &str,
    ) -> Outcome<RepositoryAccess> {
        match self.get(token, &format!("/repos/{owner}/{name}")).await {
            Ok((_, body)) => {
                let Some(full_name) = body["full_name"].as_str() else {
                    return invalid_response();
                };
                Outcome::Accepted(RepositoryAccess {
                    full_name: full_name.to_owned(),
                    private: body["private"].as_bool().unwrap_or(false),
                    permissions: body["permissions"].clone(),
                })
            }
            Err(outcome) => outcome,
        }
    }
    async fn get<T>(&self, token: &str, path: &str) -> Result<(HeaderMap, Value), Outcome<T>> {
        let token = token.trim();
        if token.is_empty() || token.chars().any(|c| c.is_control() || c == ' ') {
            return Err(Outcome::Rejected {
                status: 0,
                reason: "The token is empty or contains spaces or line breaks.".into(),
            });
        }
        let Ok(mut authorization) = HeaderValue::from_str(&format!("Bearer {token}")) else {
            return Err(Outcome::Rejected {
                status: 0,
                reason: "The token contains characters GitHub does not accept.".into(),
            });
        };
        authorization.set_sensitive(true);
        let request = self
            .client
            .get(format!("{}{path}", self.base))
            .header(AUTHORIZATION, authorization)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(USER_AGENT, "crewroom");
        let response = match tokio::time::timeout(Duration::from_secs(15), request.send()).await {
            Ok(Ok(response)) => response,
            _ => {
                return Err(Outcome::Unavailable {
                    reason: "GitHub could not be reached. Check the network and try again.".into(),
                    request_sent: true,
                });
            }
        };
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = read_body(response).await;
        let message = body
            .as_ref()
            .and_then(|b| b["message"].as_str())
            .map(|m| m.chars().take(200).collect::<String>())
            .unwrap_or_default();
        let with_message = |prefix: String| {
            if message.is_empty() {
                prefix
            } else {
                format!("{prefix} GitHub said: \"{message}\".")
            }
        };
        match status {
            200 => body.map(|b| (headers, b)).ok_or_else(invalid_response),
            401 => Err(Outcome::Rejected {
                status,
                reason: with_message(
                    "GitHub rejected the token (HTTP 401): it is invalid, expired or revoked."
                        .into(),
                ),
            }),
            403 | 429 if rate_limited(&headers) || status == 429 => Err(Outcome::Unavailable {
                reason: with_message(format!(
                    "GitHub rate-limited this request (HTTP {status}). Try again later."
                )),
                request_sent: true,
            }),
            403 | 404 => Err(Outcome::NoAccess {
                status,
                reason: with_message(format!("GitHub denied access (HTTP {status}).")),
            }),
            _ => Err(Outcome::Unavailable {
                reason: format!("GitHub returned HTTP {status}. Try again later."),
                request_sent: true,
            }),
        }
    }
}
fn invalid_response<T>() -> Outcome<T> {
    Outcome::Unavailable {
        reason: "GitHub returned an unexpected response.".into(),
        request_sent: true,
    }
}
fn rate_limited(headers: &HeaderMap) -> bool {
    headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0")
        || headers.contains_key("retry-after")
}
async fn read_body(response: reqwest::Response) -> Option<Value> {
    let stream = crate::provider_body::stream(response);
    tokio::pin!(stream);
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kinds_and_repository_names_are_checked_without_echoing_values() {
        assert_eq!(token_kind("github_pat_synthetic"), "fine-grained");
        assert_eq!(token_kind("ghp_synthetic"), "classic");
        assert_eq!(token_kind("synthetic"), "other");
        assert_eq!(
            parse_repository(" octo-org/hello.world "),
            Some(("octo-org".into(), "hello.world".into()))
        );
        for bad in [
            "", "owner", "owner/", "/name", "a/b/c", "o/..", "o/n?x", "o w/n",
        ] {
            assert_eq!(parse_repository(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_loopback_fixtures_replace_the_github_origin() {
        assert_eq!(GitHubClient::new(None).unwrap().origin(), API_ORIGIN);
        assert!(GitHubClient::new(Some("http://127.0.0.1:9/")).is_ok());
        assert!(GitHubClient::new(Some("https://example.com/")).is_err());
    }
}
