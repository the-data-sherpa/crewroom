//! Reads a repository's issues for GitHub Issues intake (FOR-45).
//!
//! Lists `GET /repos/{owner}/{name}/issues` with the saved filters, follows
//! GitHub's `Link: rel="next"` pagination on the same origin only, and drops
//! pull requests (the issues endpoint returns both). Failures keep GitHub's
//! distinctions so the workbench can say which one happened: the token was
//! refused, the repository is not reachable with it (or the app/token is not
//! installed or granted there), issues are disabled, the rate limit was hit,
//! or GitHub could not be reached. The token is used for one request at a
//! time and is never logged, serialized or returned.
use super::{GitHubClient, MAX_BODY};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;
use std::time::Duration;

/// Most raw items (issues and pull requests) one sync will page through.
pub const MAX_ITEMS: usize = 3000;
/// Most issues one sync may import; matches the durable intake limit.
pub const MAX_ISSUES: usize = crewroom_core::github_issues::MAX_ISSUES;

/// Filters as saved by `crewroom_core::github_issues::normalize_source`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    /// A milestone number, `*`, `none`, or a title resolved before listing.
    pub milestone: Option<String>,
    pub state: String,
}
impl Filters {
    pub fn from_source(source: &Value) -> Self {
        let text = |key: &str| source[key].as_str().map(str::to_owned);
        Self {
            labels: source["labels"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            assignee: text("assignee"),
            milestone: text("milestone"),
            state: text("state").unwrap_or_else(|| "open".into()),
        }
    }
}

/// Why a read did not produce a complete answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// HTTP 401: the token is invalid, expired or revoked.
    Rejected(String),
    /// HTTP 403/404 that is not a rate limit: the repository does not exist or
    /// the token (or GitHub App installation) has no access to it.
    NoAccess { status: u16, reason: String },
    /// HTTP 410: the repository has issues disabled.
    IssuesDisabled(String),
    /// Primary or secondary rate limit. `reset_at` is GitHub's reset time.
    RateLimited {
        reason: String,
        reset_at: Option<String>,
    },
    /// Network failure, timeout, 5xx, or an unexpected/oversized response.
    Unavailable(String),
    /// A filter GitHub cannot apply, such as an unknown milestone title.
    Filter(String),
}
impl Failure {
    /// Stable machine-readable reason recorded with the sync state.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Rejected(_) => "rejected",
            Self::NoAccess { .. } => "no_access",
            Self::IssuesDisabled(_) => "issues_disabled",
            Self::RateLimited { .. } => "rate_limited",
            Self::Unavailable(_) => "unreachable",
            Self::Filter(_) => "filter",
        }
    }
    /// Operator-facing explanation for `repository`.
    pub fn message(&self, repository: &str) -> String {
        match self {
            Self::Rejected(reason) => format!("{reason} Connect GitHub again with a new token."),
            Self::NoAccess { status, reason } => format!(
                "No access to {repository} (HTTP {status}): the repository does not exist, or the token or GitHub App is not installed on it or not granted Issues access. {reason}"
            )
            .trim_end()
            .to_owned(),
            Self::IssuesDisabled(reason) => {
                format!("Issues are disabled on {repository}. {reason}")
                    .trim_end()
                    .to_owned()
            }
            Self::RateLimited { reason, reset_at } => match reset_at {
                Some(at) => format!("{reason} The limit resets at {at}."),
                None => reason.clone(),
            },
            Self::Unavailable(reason) | Self::Filter(reason) => reason.clone(),
        }
    }
}

/// One complete listing: matching issues in GitHub's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub issues: Vec<Value>,
    /// Pull requests the endpoint returned and Crewroom excluded.
    pub pull_requests: usize,
    pub pages: usize,
}

impl GitHubClient {
    /// Every issue in `owner/name` matching `filters`, across all pages.
    pub async fn list_issues(
        &self,
        token: &str,
        owner: &str,
        name: &str,
        filters: &Filters,
    ) -> Result<Listing, Failure> {
        let milestone = match filters.milestone.as_deref() {
            None => None,
            Some(m) if m == "*" || m == "none" || m.bytes().all(|b| b.is_ascii_digit()) => {
                Some(m.to_owned())
            }
            Some(title) => Some(self.milestone_number(token, owner, name, title).await?),
        };
        let query = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("state", &filters.state);
            query.append_pair("per_page", "100");
            if !filters.labels.is_empty() {
                query.append_pair("labels", &filters.labels.join(","));
            }
            if let Some(assignee) = &filters.assignee {
                query.append_pair("assignee", assignee);
            }
            if let Some(milestone) = &milestone {
                query.append_pair("milestone", milestone);
            }
            query.finish()
        };
        let mut next = Some(format!("{}/repos/{owner}/{name}/issues?{query}", self.base));
        let mut listing = Listing {
            issues: vec![],
            pull_requests: 0,
            pages: 0,
        };
        let mut seen = 0usize;
        while let Some(url) = next.take() {
            let (headers, body) = self.read(token, &url).await?;
            let items = body
                .as_array()
                .ok_or_else(|| Failure::Unavailable(unexpected()))?;
            listing.pages += 1;
            seen += items.len();
            if seen > MAX_ITEMS {
                return Err(Failure::Filter(format!(
                    "More than {MAX_ITEMS} issues and pull requests match. Narrow the filters; nothing was imported."
                )));
            }
            for item in items {
                if item.get("pull_request").is_some_and(|v| !v.is_null()) {
                    listing.pull_requests += 1;
                } else {
                    listing.issues.push(item.clone());
                }
            }
            if listing.issues.len() > MAX_ISSUES {
                return Err(Failure::Filter(format!(
                    "More than {MAX_ISSUES} issues match. Narrow the filters; nothing was imported."
                )));
            }
            next = next_page(&headers, &self.base)?;
        }
        Ok(listing)
    }

    /// One issue by number, used to re-read issues with open runs that no
    /// longer match the filters. Returns `None` for a pull request.
    pub async fn issue(
        &self,
        token: &str,
        owner: &str,
        name: &str,
        number: u64,
    ) -> Result<Option<Value>, Failure> {
        let url = format!("{}/repos/{owner}/{name}/issues/{number}", self.base);
        let (_, body) = self.read(token, &url).await?;
        if !body.is_object() {
            return Err(Failure::Unavailable(unexpected()));
        }
        Ok(body
            .get("pull_request")
            .is_none_or(Value::is_null)
            .then_some(body))
    }

    async fn milestone_number(
        &self,
        token: &str,
        owner: &str,
        name: &str,
        title: &str,
    ) -> Result<String, Failure> {
        let mut next = Some(format!(
            "{}/repos/{owner}/{name}/milestones?state=all&per_page=100",
            self.base
        ));
        let mut pages = 0;
        while let Some(url) = next.take() {
            pages += 1;
            if pages > 20 {
                break;
            }
            let (headers, body) = self.read(token, &url).await?;
            let items = body
                .as_array()
                .ok_or_else(|| Failure::Unavailable(unexpected()))?;
            if let Some(number) = items
                .iter()
                .find(|m| m["title"].as_str() == Some(title))
                .and_then(|m| m["number"].as_u64())
            {
                return Ok(number.to_string());
            }
            next = next_page(&headers, &self.base)?;
        }
        Err(Failure::Filter(format!(
            "Milestone \"{title}\" was not found in {owner}/{name}. Use its exact title or number."
        )))
    }

    async fn read(&self, token: &str, url: &str) -> Result<(HeaderMap, Value), Failure> {
        let token = token.trim();
        if token.is_empty() || token.chars().any(|c| c.is_control() || c == ' ') {
            return Err(Failure::Rejected(
                "The token is empty or contains spaces or line breaks.".into(),
            ));
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                Failure::Rejected("The token contains characters GitHub does not accept.".into())
            })?;
        authorization.set_sensitive(true);
        let request = self
            .client
            .get(url)
            .header(AUTHORIZATION, authorization)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(USER_AGENT, "crewroom");
        let response = match tokio::time::timeout(Duration::from_secs(30), request.send()).await {
            Ok(Ok(response)) => response,
            _ => {
                return Err(Failure::Unavailable(
                    "GitHub could not be reached. Check the network and try again; nothing was imported.".into(),
                ));
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
        let said = if message.is_empty() {
            String::new()
        } else {
            format!("GitHub said: \"{message}\".")
        };
        match status {
            200 => body
                .map(|b| (headers, b))
                .ok_or_else(|| Failure::Unavailable(unexpected())),
            401 => Err(Failure::Rejected(
                format!("GitHub rejected the token (HTTP 401): it is invalid, expired or revoked. {said}")
                    .trim_end()
                    .to_owned(),
            )),
            403 | 429 if status == 429 || super::rate_limited(&headers) => {
                Err(Failure::RateLimited {
                    reason: format!(
                        "GitHub rate-limited this request (HTTP {status}); nothing was imported. {said}"
                    )
                    .trim_end()
                    .to_owned(),
                    reset_at: reset_at(&headers),
                })
            }
            403 | 404 => Err(Failure::NoAccess {
                status,
                reason: said,
            }),
            410 => Err(Failure::IssuesDisabled(said)),
            _ => Err(Failure::Unavailable(format!(
                "GitHub returned HTTP {status}. Try again later; nothing was imported."
            ))),
        }
    }
}

fn unexpected() -> String {
    "GitHub returned an unexpected response; nothing was imported.".into()
}

/// GitHub's reset time from `x-ratelimit-reset` (epoch seconds) or `retry-after`.
fn reset_at(headers: &HeaderMap) -> Option<String> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<i64>().ok())
    };
    if let Some(after) = header("retry-after") {
        return Some(crewroom_core::timestamp(
            crewroom_core::now_ms() + after.max(0) * 1000,
        ));
    }
    header("x-ratelimit-reset")
        .filter(|s| *s > 0)
        .map(|s| crewroom_core::timestamp(s * 1000))
}

/// The `rel="next"` URL from a `Link` header. A next page on any other origin
/// is refused so the token is never sent elsewhere.
fn next_page(headers: &HeaderMap, base: &str) -> Result<Option<String>, Failure> {
    let Some(link) = headers.get("link").and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    for part in link.split(',') {
        let mut pieces = part.split(';');
        let Some(target) = pieces
            .next()
            .map(str::trim)
            .and_then(|t| t.strip_prefix('<'))
            .and_then(|t| t.strip_suffix('>'))
        else {
            continue;
        };
        if pieces.any(|p| p.trim() == "rel=\"next\"") {
            let same_origin = url::Url::parse(target)
                .ok()
                .is_some_and(|u| u.origin().ascii_serialization() == base);
            if !same_origin {
                return Err(Failure::Unavailable(
                    "GitHub pointed the next page at another origin; nothing was imported.".into(),
                ));
            }
            return Ok(Some(target.to_owned()));
        }
    }
    Ok(None)
}

async fn read_body(response: reqwest::Response) -> Option<Value> {
    let stream = crate::provider_body::stream(response);
    tokio::pin!(stream);
    let mut bytes = Vec::new();
    // A page of 100 issues with long bodies can exceed the account endpoint's limit.
    let limit = MAX_BODY * 16;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn link(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("link", value.parse().unwrap());
        headers
    }

    #[test]
    fn next_page_follows_only_the_same_origin() {
        let base = "https://api.github.com";
        assert_eq!(
            next_page(
                &link(
                    "<https://api.github.com/repositories/1/issues?page=2>; rel=\"next\", <https://api.github.com/repositories/1/issues?page=5>; rel=\"last\""
                ),
                base
            ),
            Ok(Some(
                "https://api.github.com/repositories/1/issues?page=2".into()
            ))
        );
        assert_eq!(
            next_page(
                &link("<https://api.github.com/repositories/1/issues?page=1>; rel=\"prev\""),
                base
            ),
            Ok(None)
        );
        assert_eq!(next_page(&HeaderMap::new(), base), Ok(None));
        assert!(
            next_page(
                &link("<https://example.com/issues?page=2>; rel=\"next\""),
                base
            )
            .is_err()
        );
    }

    #[test]
    fn filters_come_from_the_saved_source_and_failures_explain_themselves() {
        let filters = Filters::from_source(
            &json!({"repository":"o/r","labels":["bug","p1"],"assignee":"none","milestone":"2","state":"all"}),
        );
        assert_eq!(filters.labels, ["bug", "p1"]);
        assert_eq!(filters.assignee.as_deref(), Some("none"));
        assert_eq!(filters.milestone.as_deref(), Some("2"));
        assert_eq!(filters.state, "all");
        let limited = Failure::RateLimited {
            reason: "GitHub rate-limited this request (HTTP 403).".into(),
            reset_at: Some("2026-10-18T09:00:00.000Z".into()),
        };
        assert_eq!(limited.kind(), "rate_limited");
        assert!(
            limited
                .message("o/r")
                .ends_with("resets at 2026-10-18T09:00:00.000Z.")
        );
        let none = Failure::NoAccess {
            status: 404,
            reason: String::new(),
        };
        assert!(none.message("o/r").contains("not installed"));
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset", "1792310400".parse().unwrap());
        assert_eq!(
            reset_at(&headers).as_deref(),
            Some(crewroom_core::timestamp(1_792_310_400_000).as_str())
        );
    }
}
