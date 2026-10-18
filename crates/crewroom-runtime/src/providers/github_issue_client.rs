//! GitHub issue writes for ticket delivery (FOR-48): read one issue's state and
//! labels, list its comments, add a comment, and add or remove a label.
//!
//! The token comes from the private credential callback for each request and is
//! never logged, serialized or returned. Refusals carry the remedy for issue
//! delivery (not publication) and keep GitHub's status so the outbox can tell a
//! rejected write from an unknown one.
use super::{
    schema::*,
    transport::{Transport, fixture_origin},
    *,
};
use serde_json::{Value, json};
use std::collections::HashSet;

const PAGE: usize = 100;

/// One GitHub issue as delivery needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueState {
    pub number: u64,
    /// `open` or `closed`, exactly as GitHub reports it.
    pub state: String,
    pub labels: Vec<String>,
    pub updated_at: Option<String>,
}

/// Rewrites a GitHub refusal with the remedy for issue delivery. The status and
/// whether the request was sent are kept; GitHub's own explanation is kept.
fn delivery_failure(error: Error) -> Error {
    let Error::Unavailable(mut e) = error else {
        return error;
    };
    let said = e
        .message
        .find(" GitHub said: ")
        .map(|i| e.message[i..].to_owned())
        .unwrap_or_default();
    let message = match e.status {
        Some(401) => "GitHub rejected the token (HTTP 401): it is invalid, expired or revoked. Reconnect GitHub in Settings → Integrations, or update CREWROOM_GITHUB_TOKEN, then retry the update.".to_owned(),
        Some(429) => "GitHub rate-limited this request. The update is kept and is retried after the limit resets.".to_owned(),
        Some(403) => "GitHub denied this request (HTTP 403): the token cannot write issues in this repository. Grant a fine-grained token Issues: Read and write on it (a classic token needs the repo scope), then retry the update.".to_owned(),
        Some(404) => "GitHub could not find this repository or issue with the saved token (HTTP 404). Add the repository to the token's repository access, then retry the update.".to_owned(),
        Some(410) => "GitHub reports that this issue was deleted or that issues are disabled (HTTP 410). The update cannot be delivered.".to_owned(),
        Some(422) => "GitHub refused the update (HTTP 422).".to_owned(),
        Some(status) => format!("GitHub returned HTTP {status}. The update is kept."),
        None => return Error::Unavailable(e),
    };
    e.message = format!("{message}{said}");
    Error::Unavailable(e)
}

#[derive(Clone)]
pub struct GitHubIssueClient {
    repository: String,
    transport: Transport,
}
impl GitHubIssueClient {
    /// `repository` is `owner/name`; `fixture` replaces api.github.com with a
    /// loopback test origin only.
    pub fn new(repository: &str, credentials: Credentials, fixture: Option<&str>) -> Result<Self> {
        let (owner, name) = super::github::parse_repository(repository)
            .ok_or_else(|| Error::Invalid("Invalid GitHub repository.".into()))?;
        let origin = fixture
            .map(|s| fixture_origin(s, Provider::GitHub))
            .transpose()?
            .unwrap_or_else(|| super::github::API_ORIGIN.into());
        Ok(Self {
            repository: format!("{owner}/{name}"),
            transport: Transport::new(
                Provider::GitHub,
                format!("{origin}/repos/{owner}/{name}"),
                credentials,
            )?,
        })
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    async fn request(
        &self,
        path: &str,
        method: reqwest::Method,
        body: Option<Value>,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        self.transport
            .request(path, method, body, before)
            .await
            .map(|v| v.unwrap_or(Value::Null))
            .map_err(delivery_failure)
    }
    /// The issue's state and label names. A pull request under this number is
    /// refused: it is not the ticket's issue.
    pub async fn issue(&self, number: u64) -> Result<IssueState> {
        let raw = self
            .request(
                &format!("/issues/{number}"),
                reqwest::Method::GET,
                None,
                None,
            )
            .await?;
        let shape = || unavailable("GitHub returned an unexpected issue.");
        if raw.get("pull_request").is_some_and(|v| !v.is_null()) {
            return Err(unavailable(
                "GitHub reports this number is a pull request, not the ticket's issue.",
            ));
        }
        let observed = integer(&raw["number"], 1).map_err(|_| shape())?;
        let state = string(&raw["state"]).map_err(|_| shape())?;
        if observed as u64 != number || !matches!(state.as_str(), "open" | "closed") {
            return Err(shape());
        }
        let labels = array(&raw["labels"])
            .map_err(|_| shape())?
            .iter()
            .map(|l| text(&l["name"], 1, 100, false))
            .collect::<Result<Vec<_>>>()
            .map_err(|_| shape())?;
        let updated_at = match raw.get("updated_at") {
            None | Some(Value::Null) => None,
            Some(v) => Some(date(v).map_err(|_| shape())?),
        };
        Ok(IssueState {
            number,
            state,
            labels,
            updated_at,
        })
    }
    /// Every comment on the issue as `{id, body}`, across pages.
    pub async fn comments(&self, number: u64) -> Result<Vec<Value>> {
        let mut comments = vec![];
        let mut ids = HashSet::new();
        for page in 1..=100 {
            let rows = self
                .request(
                    &format!("/issues/{number}/comments?per_page={PAGE}&page={page}"),
                    reqwest::Method::GET,
                    None,
                    None,
                )
                .await?;
            let rows = array(&rows)
                .map_err(|_| unavailable("GitHub returned inconsistent issue comments."))?;
            for row in rows {
                let id = integer(&row["id"], 1)
                    .map_err(|_| unavailable("GitHub returned inconsistent issue comments."))?;
                if !ids.insert(id) {
                    return Err(unavailable("GitHub returned inconsistent issue comments."));
                }
                comments.push(json!({"id": id, "body": row["body"].as_str().unwrap_or("")}));
            }
            if rows.len() < PAGE {
                return Ok(comments);
            }
        }
        Err(unavailable("GitHub comment pagination did not complete."))
    }
    /// Adds one comment and returns GitHub's comment id.
    pub async fn add_comment(
        &self,
        number: u64,
        body: &str,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        let created = self
            .request(
                &format!("/issues/{number}/comments"),
                reqwest::Method::POST,
                Some(json!({"body": body})),
                before,
            )
            .await?;
        integer(&created["id"], 1)
            .map(|id| json!(id))
            .map_err(|_| sent("GitHub returned an unexpected comment receipt."))
    }
    pub async fn add_label(
        &self,
        number: u64,
        label: &str,
        before: Option<&BeforeDispatch>,
    ) -> Result<()> {
        self.request(
            &format!("/issues/{number}/labels"),
            reqwest::Method::POST,
            Some(json!({"labels": [label]})),
            before,
        )
        .await
        .map(|_| ())
    }
    pub async fn remove_label(
        &self,
        number: u64,
        label: &str,
        before: Option<&BeforeDispatch>,
    ) -> Result<()> {
        self.request(
            &format!("/issues/{number}/labels/{}", component(label)),
            reqwest::Method::DELETE,
            None,
            before,
        )
        .await
        .map(|_| ())
    }
}

fn sent(message: &str) -> Error {
    Error::Unavailable(Unavailable {
        message: message.into(),
        status: None,
        request_sent: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_name_the_issue_delivery_remedy_and_keep_their_status() {
        let failure = |status: u16| {
            delivery_failure(Error::Unavailable(Unavailable {
                message: "Publication text. GitHub said: \"Resource not accessible\".".into(),
                status: Some(status),
                request_sent: true,
            }))
        };
        for (status, needle) in [
            (401, "Reconnect GitHub"),
            (403, "Issues: Read and write"),
            (404, "repository access"),
            (410, "cannot be delivered"),
            (429, "rate-limited"),
        ] {
            let Error::Unavailable(e) = failure(status) else {
                panic!()
            };
            assert!(e.message.contains(needle), "{status}: {}", e.message);
            assert!(!e.message.contains("publication"), "{}", e.message);
            assert!(
                e.message
                    .ends_with("GitHub said: \"Resource not accessible\".")
            );
            assert_eq!(e.status, Some(status));
            assert!(e.request_sent);
        }
    }

    #[test]
    fn only_owner_name_repositories_and_loopback_fixtures_are_accepted() {
        let credentials: Credentials =
            std::sync::Arc::new(|| Box::pin(async { Ok(Credential::Bearer("synthetic".into())) }));
        assert!(GitHubIssueClient::new("owner/name", credentials.clone(), None).is_ok());
        assert!(GitHubIssueClient::new("owner", credentials.clone(), None).is_err());
        assert!(GitHubIssueClient::new("owner/na/me", credentials.clone(), None).is_err());
        assert!(
            GitHubIssueClient::new("owner/name", credentials, Some("https://example.com/"))
                .is_err()
        );
    }
}
