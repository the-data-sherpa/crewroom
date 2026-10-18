//! GitHub pull-request host for PR publication (FOR-46).
//!
//! Responses are validated and normalized into the provider-neutral publication
//! shape the Bitbucket host already produces (`OPEN`/`MERGED`/`DECLINED`,
//! `source`/`destination` branch, commit and repository identity, hosted
//! statuses and one PR check pipeline), so publication, readiness and
//! continuation verification stay provider-neutral. The repository identity key
//! is `github:<repository id>`: GitHub's immutable numeric id, not its name.
//!
//! The token comes from the private credential callback for each request and is
//! never logged, serialized or returned.
use super::{
    schema::*,
    transport::{Transport, fixture_origin},
    *,
};
use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Stored provider marker on a GitHub repository identity.
pub const PROVIDER: &str = "github";
const PAGE: usize = 100;

/// The durable repository identity key shared by PR, feedback and continuation
/// records.
pub fn identity(id: i64) -> String {
    format!("github:{id}")
}

fn name_part(value: &Value) -> Result<String> {
    let s = text(value, 1, 100, false)?;
    if s == "."
        || s == ".."
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(invalid());
    }
    Ok(s)
}

/// Validates configured GitHub repository metadata (`owner`, `name`, numeric
/// `id`) and returns its normalized identity. Already normalized identities are
/// accepted unchanged.
pub fn repository(value: &Value) -> Result<Value> {
    strict(value, &["owner", "name", "id", "provider", "uuid"])?;
    let owner = name_part(&value["owner"])?;
    let name = name_part(&value["name"])?;
    let id = integer(&value["id"], 1)?;
    let result = json!({"provider":PROVIDER,"owner":owner,"name":name,"id":id,"uuid":identity(id)});
    for key in ["provider", "uuid"] {
        if value.get(key).is_some_and(|v| *v != result[key]) {
            return Err(invalid());
        }
    }
    Ok(result)
}

/// Whether a stored repository identity names a GitHub repository.
pub fn is_github(repository: &Value) -> bool {
    repository["provider"] == PROVIDER
}

pub(super) fn rate_limited(headers: &HeaderMap) -> bool {
    headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0")
        || headers.contains_key("retry-after")
}

/// GitHub's own explanation of a refusal, bounded and free of control
/// characters. GitHub error bodies never echo the request's token.
pub(super) async fn error_message(response: reqwest::Response) -> String {
    let stream = crate::provider_body::stream(response);
    tokio::pin!(stream);
    let mut bytes = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            return String::new();
        }
        bytes.extend_from_slice(&chunk);
    }
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return String::new();
    };
    let mut parts = vec![];
    if let Some(message) = body["message"].as_str() {
        parts.push(message.to_owned());
    }
    if let Some(detail) = body["errors"]
        .as_array()
        .and_then(|e| e.first())
        .and_then(|e| e["message"].as_str())
    {
        parts.push(detail.to_owned());
    }
    parts
        .join(": ")
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect()
}

/// A GitHub refusal with the remedy the operator needs. Rate limits keep the
/// retryable 429 status.
pub(super) fn failure(status: u16, limited: bool, said: &str) -> Error {
    let (status, message) = match status {
        401 => (
            401,
            "GitHub rejected the saved token (HTTP 401): it is invalid, expired or revoked. Reconnect GitHub in Settings → Integrations, or update CREWROOM_GITHUB_TOKEN, then reconcile publication.".to_owned(),
        ),
        403 | 429 if limited || status == 429 => (
            429,
            "GitHub rate-limited this request. Saved publication intents are retained; reconcile after the limit resets.".to_owned(),
        ),
        403 => (
            403,
            "GitHub denied this request (HTTP 403): the token lacks permission for this repository. Grant a fine-grained token Pull requests: Read and write and Contents: Read and write on this repository (a classic token needs the repo scope), then reconcile publication.".to_owned(),
        ),
        404 => (
            404,
            "GitHub could not find this repository or pull request with the saved token (HTTP 404). Add the repository to the token's repository access (fine-grained tokens see only selected repositories) and check the publish remote, then reconcile publication.".to_owned(),
        ),
        422 => (
            422,
            "GitHub refused the pull request (HTTP 422). A missing branch, a repository rule or another open PR for this branch can cause this. Inspect the repository before reconciling.".to_owned(),
        ),
        other => (
            other,
            format!("GitHub returned HTTP {other}. Saved publication intents are retained."),
        ),
    };
    let message = if said.is_empty() {
        message
    } else {
        format!("{message} GitHub said: \"{said}\".")
    };
    Error::Unavailable(Unavailable {
        message,
        status: Some(status),
        request_sent: true,
    })
}

/// A credential, permission or visibility refusal from GitHub proves a PR write
/// had no effect. It is reported with its remedy and the intent stays pending,
/// so a reconcile after the remedy retries it instead of leaving it blocked.
fn refused_without_effect(error: Error) -> Error {
    match error {
        Error::Unavailable(mut e) if matches!(e.status, Some(401 | 403 | 404)) => {
            e.request_sent = false;
            Error::Unavailable(e)
        }
        other => other,
    }
}
fn sent(error: Error) -> Error {
    Error::Unavailable(Unavailable {
        message: error.to_string(),
        status: None,
        request_sent: true,
    })
}
fn query(pairs: &[(&str, &str)]) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        q.append_pair(k, v);
    }
    q.finish()
}

fn check_state(status: &str, conclusion: Option<&str>) -> Value {
    if status != "completed" {
        return json!({"name": if status == "in_progress" {"IN_PROGRESS"} else {"PENDING"}});
    }
    let result = match conclusion.unwrap_or("") {
        "success" => "SUCCESSFUL",
        "failure" | "timed_out" | "action_required" | "startup_failure" => "FAILED",
        "cancelled" => "STOPPED",
        "neutral" => "NEUTRAL",
        "skipped" => "SKIPPED",
        "stale" => "STALE",
        _ => "UNKNOWN",
    };
    json!({"name":"COMPLETED","result":{"name":result}})
}
fn status_state(state: &str) -> &'static str {
    match state {
        "success" => "SUCCESSFUL",
        "failure" | "error" => "FAILED",
        _ => "INPROGRESS",
    }
}

#[derive(Clone)]
pub struct GitHubPullClient {
    pub repository: Value,
    transport: Transport,
}
impl GitHubPullClient {
    /// `fixture` replaces api.github.com with a loopback test origin only.
    pub fn new(input: &Value, credentials: Credentials, fixture: Option<&str>) -> Result<Self> {
        let repository = repository(input)?;
        let origin = fixture
            .map(|s| fixture_origin(s, Provider::GitHub))
            .transpose()?
            .unwrap_or_else(|| super::github::API_ORIGIN.into());
        let transport = Transport::new(
            Provider::GitHub,
            format!(
                "{origin}/repos/{}/{}",
                repository["owner"].as_str().unwrap(),
                repository["name"].as_str().unwrap()
            ),
            credentials,
        )?;
        Ok(Self {
            repository,
            transport,
        })
    }
    fn full_name(&self) -> String {
        format!(
            "{}/{}",
            self.repository["owner"].as_str().unwrap(),
            self.repository["name"].as_str().unwrap()
        )
    }
    async fn get(&self, path: &str) -> Result<Value> {
        Ok(self
            .transport
            .request(path, reqwest::Method::GET, None, None)
            .await?
            .unwrap_or(Value::Null))
    }
    /// Page-numbered listing; `field` names the array inside an object page.
    async fn pages(
        &self,
        path: &str,
        field: Option<&str>,
        mut on_page: impl FnMut(&[Value]) -> Result<()> + Send,
    ) -> Result<Vec<Value>> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut values = vec![];
        for page in 1..=100 {
            let body = self
                .get(&format!("{path}{separator}per_page={PAGE}&page={page}"))
                .await?;
            let rows = array(field.map_or(&body, |f| &body[f]))?;
            on_page(rows)?;
            values.extend(rows.iter().cloned());
            if values.len() > 10000 {
                return Err(unavailable("GitHub pagination exceeded its item limit."));
            }
            if rows.len() < PAGE {
                return Ok(values);
            }
        }
        Err(unavailable("GitHub pagination did not complete."))
    }
    async fn all(&self, path: &str, field: Option<&str>) -> Result<Vec<Value>> {
        self.pages(path, field, |_| Ok(())).await
    }
    fn reference(&self, value: &Value) -> Result<Value> {
        let repository = &value["repo"];
        let uuid = if repository.is_null() {
            // A deleted fork has no repository; it cannot match this identity.
            Value::Null
        } else {
            json!(identity(integer(&repository["id"], 1)?))
        };
        Ok(
            json!({"branch":{"name":string(&value["ref"])?},"commit":{"hash":hash(&value["sha"],false)?},"repository":{"uuid":uuid}}),
        )
    }
    /// Normalizes and verifies one GitHub pull request.
    fn parse_pr(&self, value: &Value) -> Result<Value> {
        let shape = || unavailable("GitHub returned an unexpected pull request.");
        let id = integer(&value["number"], 1).map_err(|_| shape())?;
        let merged = value
            .get("merged_at")
            .is_some_and(|v| !v.is_null() && date(v).is_ok());
        let state = match (value["state"].as_str(), merged) {
            (Some("open"), _) => "OPEN",
            (Some("closed"), true) => "MERGED",
            (Some("closed"), false) => "DECLINED",
            _ => return Err(shape()),
        };
        let html = url_text(&value["html_url"]).map_err(|_| shape())?;
        let mut pr = json!({
            "id": id,
            "title": string(&value["title"]).map_err(|_| shape())?,
            "description": value["body"].as_str().unwrap_or(""),
            "state": state,
            "updated_on": date(&value["updated_at"]).map_err(|_| shape())?,
            "source": self.reference(&value["head"]).map_err(|_| shape())?,
            "destination": self.reference(&value["base"]).map_err(|_| shape())?,
            "links": {"html": {"href": html}},
            "draft": value["draft"].as_bool().unwrap_or(false),
            "merge_commit": Value::Null,
        });
        if state == "MERGED" {
            pr["merge_commit"] =
                json!({"hash":hash(&value["merge_commit_sha"],false).map_err(|_| shape())?});
        }
        let expected = format!("https://github.com/{}/pull/{id}", self.full_name());
        if pr["source"]["repository"]["uuid"] != self.repository["uuid"]
            || pr["destination"]["repository"]["uuid"] != self.repository["uuid"]
            || !html.eq_ignore_ascii_case(&expected)
        {
            return Err(unavailable(
                "GitHub returned a different repository or PR identity.",
            ));
        }
        Ok(pr)
    }
    /// Confirms the configured repository is still the same GitHub repository
    /// and that the signed-in account can write to it.
    pub async fn verify_repository(&self) -> Result<Value> {
        let raw = self.get("").await?;
        let id = integer(&raw["id"], 1)?;
        let full_name = string(&raw["full_name"])?;
        if json!(id) != self.repository["id"] || !full_name.eq_ignore_ascii_case(&self.full_name())
        {
            return Err(unavailable(
                "GitHub repository identity changed. Publication is blocked.",
            ));
        }
        if raw["permissions"]["push"] == false {
            return Err(Error::Unavailable(Unavailable {
                message: format!(
                    "GitHub reports that the connected account cannot push to {full_name}. Ask for write access to the repository or connect an account that has it, then reconcile publication."
                ),
                status: Some(403),
                request_sent: false,
            }));
        }
        Ok(json!({"uuid":identity(id),"full_name":full_name}))
    }
    pub async fn pull_requests(&self, branch: &str, target: &str) -> Result<Vec<Value>> {
        let head = format!("{}:{branch}", self.repository["owner"].as_str().unwrap());
        let rows = self
            .all(
                &format!(
                    "/pulls?{}",
                    query(&[("state", "all"), ("head", &head), ("base", target)])
                ),
                None,
            )
            .await?;
        let prs = rows
            .iter()
            .map(|row| self.parse_pr(row))
            .collect::<Result<Vec<_>>>()?;
        let mut ids = HashSet::new();
        if prs.iter().any(|pr| {
            !ids.insert(pr["id"].as_i64().unwrap())
                || pr["source"]["branch"]["name"] != branch
                || pr["destination"]["branch"]["name"] != target
        }) {
            return Err(unavailable("GitHub returned an inconsistent PR search."));
        }
        Ok(prs)
    }
    pub async fn pull_request(&self, id: i64) -> Result<Value> {
        integer(&json!(id), 1)?;
        let pr = self.parse_pr(&self.get(&format!("/pulls/{id}")).await?)?;
        if pr["id"] != id {
            return Err(unavailable("GitHub returned a different PR identity."));
        }
        Ok(pr)
    }
    fn comment(value: &Value, review: bool) -> Result<Value> {
        let mut result = json!({"id":integer(&value["id"],1)?,"created_on":date(&value["created_at"])?,"updated_on":date(&value["updated_at"])?,"deleted":false,"content":{"raw":text(&value["body"],0,65536,false)?}});
        if value["user"].is_object() {
            result["user"] = json!({"uuid":identity(integer(&value["user"]["id"],1)?),"display_name":string(&value["user"]["login"])?});
        }
        if review {
            let mut inline = json!({"path":text(&value["path"],0,1000,false)?,"outdated":value["line"].is_null()});
            for (key, from) in [("to", "line"), ("from", "original_line")] {
                inline[key] = match value.get(from) {
                    Some(v) if !v.is_null() => json!(integer(v, 1)?),
                    _ => Value::Null,
                };
            }
            result["inline"] = inline;
            if let Some(parent) = value.get("in_reply_to_id").filter(|v| !v.is_null()) {
                result["parent"] = json!({"id":integer(parent,1)?});
            }
        }
        Ok(result)
    }
    /// Conversation and review comments. A failed read keeps what was read and
    /// reports it as partial or unavailable rather than as an empty review.
    pub async fn comments(&self, id: i64) -> Result<Value> {
        integer(&json!(id), 1)?;
        let mut comments = vec![];
        let mut ids = HashSet::new();
        let mut result = Ok(());
        for (path, review) in [
            (format!("/issues/{id}/comments"), false),
            (format!("/pulls/{id}/comments"), true),
        ] {
            result = self
                .pages(&path, None, |rows| {
                    for row in rows {
                        let comment = Self::comment(row, review).map_err(|_| {
                            unavailable("GitHub returned inconsistent PR comments.")
                        })?;
                        if !ids.insert(comment["id"].as_i64().unwrap()) {
                            return Err(unavailable("GitHub returned inconsistent PR comments."));
                        }
                        comments.push(comment);
                    }
                    Ok(())
                })
                .await
                .map(|_| ());
            if result.is_err() {
                break;
            }
        }
        Ok(match result {
            Ok(()) => json!({"state":"complete","comments":comments}),
            Err(error) => {
                json!({"state":if comments.is_empty(){"unavailable"}else{"partial"},"comments":comments,"reason":error.to_string()})
            }
        })
    }
    /// Hosted evidence for the PR's current head: commit statuses and check
    /// runs, plus the check runs as one PR pipeline bound to head and base.
    pub async fn observe(&self, id: i64) -> Result<Value> {
        let pr = self.pull_request(id).await?;
        let head = pr["source"]["commit"]["hash"].as_str().unwrap().to_owned();
        let mut statuses: HashMap<String, Value> = HashMap::new();
        let mut keep = |status: Value| {
            let key = status["key"].as_str().unwrap().to_owned();
            let newer = statuses.get(&key).is_none_or(|old| {
                crewroom_core::source_time::parse(status["updated_on"].as_str().unwrap())
                    > crewroom_core::source_time::parse(old["updated_on"].as_str().unwrap())
            });
            if newer {
                statuses.insert(key, status);
            }
        };
        let bad_status = || unavailable("GitHub returned an unexpected commit status.");
        for raw in self.all(&format!("/commits/{head}/statuses"), None).await? {
            let context = string(&raw["context"]).map_err(|_| bad_status())?;
            let mut status = json!({"key":context,"name":context,"state":status_state(raw["state"].as_str().unwrap_or("")),"updated_on":date(&raw["updated_at"]).map_err(|_| bad_status())?});
            if let Ok(url) = url_text(&raw["target_url"]) {
                status["url"] = json!(url);
            }
            keep(status);
        }
        let bad_check = || unavailable("GitHub returned an unexpected check run.");
        let mut runs: HashMap<String, Value> = HashMap::new();
        for raw in self
            .all(&format!("/commits/{head}/check-runs"), Some("check_runs"))
            .await?
        {
            let name = string(&raw["name"]).map_err(|_| bad_check())?;
            let run_id = integer(&raw["id"], 1).map_err(|_| bad_check())?;
            if hash(&raw["head_sha"], false).ok().as_deref() != Some(head.as_str()) {
                return Err(bad_check());
            }
            // A re-run replaces the earlier run of the same name.
            if runs
                .get(&name)
                .is_none_or(|old| old["id"].as_i64().unwrap() < run_id)
            {
                runs.insert(name, raw);
            }
        }
        let mut steps = vec![];
        let mut bases = HashSet::new();
        let mut linked = true;
        let mut started: Option<String> = None;
        let mut names = runs.keys().cloned().collect::<Vec<_>>();
        names.sort();
        for name in names {
            let raw = &runs[&name];
            let state = check_state(
                raw["status"].as_str().unwrap_or(""),
                raw["conclusion"].as_str(),
            );
            let updated = raw["completed_at"]
                .as_str()
                .or(raw["started_at"].as_str())
                .map(|s| json!(s))
                .and_then(|v| date(&v).ok())
                .unwrap_or_else(|| pr["updated_on"].as_str().unwrap().to_owned());
            if started.as_ref().is_none_or(|s| {
                crewroom_core::source_time::parse(&updated) < crewroom_core::source_time::parse(s)
            }) {
                started = Some(updated.clone());
            }
            let status_name = state["result"]["name"]
                .as_str()
                .map(|r| {
                    if r == "SUCCESSFUL" || r == "FAILED" {
                        r
                    } else {
                        "INPROGRESS"
                    }
                })
                .unwrap_or("INPROGRESS");
            keep(json!({"key":name,"name":name,"state":status_name,"updated_on":updated}));
            let link = raw["pull_requests"]
                .as_array()
                .and_then(|prs| prs.iter().find(|p| p["number"] == id));
            match link.and_then(|p| hash(&p["base"]["sha"], false).ok()) {
                Some(base) => {
                    bases.insert(base);
                }
                None => linked = false,
            }
            steps.push(
                json!({"uuid":format!("github-check:{}",raw["id"]),"name":name,"state":state}),
            );
        }
        let mut pipelines = vec![];
        if !steps.is_empty() {
            let states = steps.iter().map(|s| &s["state"]).collect::<Vec<_>>();
            let state = if states.iter().all(|s| s["name"] == "COMPLETED") {
                let result = if states.iter().any(|s| s["result"]["name"] == "FAILED") {
                    "FAILED"
                } else if states.iter().any(|s| s["result"]["name"] == "STOPPED") {
                    "STOPPED"
                } else if states.iter().all(|s| {
                    ["SUCCESSFUL", "NEUTRAL", "SKIPPED"]
                        .contains(&s["result"]["name"].as_str().unwrap_or(""))
                }) {
                    "SUCCESSFUL"
                } else {
                    "UNKNOWN"
                };
                json!({"name":"COMPLETED","result":{"name":result}})
            } else {
                json!({"name":"IN_PROGRESS"})
            };
            let mut target = json!({"type":"pipeline_pullrequest_target","commit":{"hash":head},"source":pr["source"]["branch"]["name"],"destination":pr["destination"]["branch"]["name"],"selector":{"type":"pull-requests","pattern":"github-checks"}});
            // The base is known only when every check run names this PR with one
            // base commit; otherwise the pipeline cannot vouch for head and base.
            if linked && bases.len() == 1 {
                target["destination_commit"] = json!({"hash":bases.into_iter().next()});
                target["pullrequest"] = json!({"id":id});
            }
            let build = runs
                .values()
                .map(|r| r["id"].as_i64().unwrap())
                .max()
                .unwrap();
            pipelines.push(json!({"uuid":format!("github-checks:{head}"),"build_number":build,"created_on":started,"state":state,"target":target,"steps":steps}));
        }
        let current = self.pull_request(id).await?;
        if crewroom_core::serialized::stringify(&current)
            != crewroom_core::serialized::stringify(&pr)
        {
            return Err(unavailable(
                "PR changed while hosted evidence was being read. Refresh before assessing readiness.",
            ));
        }
        let mut statuses = statuses.into_values().collect::<Vec<_>>();
        statuses.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        Ok(
            json!({"observedAt":crewroom_core::timestamp(crewroom_core::now_ms()),"pr":pr,"statuses":statuses,"pipelines":pipelines}),
        )
    }
    pub async fn create_pull_request(
        &self,
        input: &Value,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        hash(&input["head"], false)?;
        let body = json!({"title":input["title"],"body":input["description"],"head":input["branch"],"base":input["target"],"draft":false,"maintainer_can_modify":false});
        let value = self
            .transport
            .request("/pulls", reqwest::Method::POST, Some(body), before)
            .await
            .map_err(refused_without_effect)?
            .unwrap_or(Value::Null);
        let pr = self.parse_pr(&value).map_err(sent)?;
        if pr["source"]["commit"]["hash"] != input["head"] {
            return Err(sent(unavailable(
                "GitHub opened the PR on a different head commit than the published candidate.",
            )));
        }
        Ok(pr)
    }
    pub async fn update_pull_request(
        &self,
        id: i64,
        input: &Value,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        integer(&json!(id), 1)?;
        let value = self
            .transport
            .request(
                &format!("/pulls/{id}"),
                reqwest::Method::PATCH,
                Some(json!({"title":input["title"],"body":input["description"]})),
                before,
            )
            .await
            .map_err(refused_without_effect)?
            .unwrap_or(Value::Null);
        let pr = self.parse_pr(&value).map_err(sent)?;
        if pr["id"] != id {
            return Err(sent(unavailable(
                "GitHub returned a different PR identity.",
            )));
        }
        Ok(pr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_identity_is_the_numeric_github_id() {
        let configured = json!({"owner":"octo-fixture","name":"hello.world","id":42});
        let normalized = repository(&configured).unwrap();
        assert_eq!(
            normalized,
            json!({"provider":"github","owner":"octo-fixture","name":"hello.world","id":42,"uuid":"github:42"})
        );
        // Normalization is idempotent; a forged identity key is refused.
        assert_eq!(repository(&normalized).unwrap(), normalized);
        let mut forged = normalized.clone();
        forged["uuid"] = json!("github:7");
        assert!(repository(&forged).is_err());
        for bad in [
            json!({"owner":"o","name":"n"}),
            json!({"owner":"o","name":"n","id":0}),
            json!({"owner":"o/x","name":"n","id":1}),
            json!({"owner":"o","name":"..","id":1}),
            json!({"owner":"o","name":"n","id":1,"workspace":"x"}),
        ] {
            assert!(repository(&bad).is_err(), "{bad}");
        }
        assert!(is_github(&normalized));
        assert!(!is_github(
            &json!({"uuid":"{x}","workspace":"w","slug":"s"})
        ));
    }

    #[test]
    fn refusals_name_their_remedy_and_rate_limits_stay_retryable() {
        let message = |e: Error| match e {
            Error::Unavailable(u) => (u.status, u.message),
            Error::Invalid(m) => (None, m),
        };
        let (status, text) = message(failure(401, false, "Bad credentials"));
        assert_eq!(status, Some(401));
        assert!(text.contains("Reconnect GitHub in Settings → Integrations"));
        assert!(text.contains("GitHub said: \"Bad credentials\""));
        let (status, text) = message(failure(403, false, ""));
        assert_eq!(status, Some(403));
        assert!(text.contains("Pull requests: Read and write"));
        assert!(text.contains("Contents: Read and write"));
        let (status, text) = message(failure(404, false, "Not Found"));
        assert_eq!(status, Some(404));
        assert!(text.contains("repository access"));
        let (status, _) = message(failure(403, true, "API rate limit exceeded"));
        assert_eq!(status, Some(429));
        let (status, text) = message(failure(422, false, "Validation Failed"));
        assert_eq!(status, Some(422));
        assert!(text.contains("Inspect the repository"));
        // Credential/permission refusals had no effect, so the intent stays
        // pending; other refusals keep their sent marker.
        for (code, sent) in [(401, false), (403, false), (404, false), (422, true)] {
            match refused_without_effect(failure(code, false, "")) {
                Error::Unavailable(u) => assert_eq!(u.request_sent, sent, "{code}"),
                Error::Invalid(_) => panic!("refusals stay unavailable"),
            }
        }
    }

    #[test]
    fn check_runs_map_to_pipeline_step_states() {
        assert_eq!(
            check_state("completed", Some("success")),
            json!({"name":"COMPLETED","result":{"name":"SUCCESSFUL"}})
        );
        assert_eq!(
            check_state("completed", Some("timed_out"))["result"]["name"],
            "FAILED"
        );
        assert_eq!(
            check_state("completed", Some("skipped"))["result"]["name"],
            "SKIPPED"
        );
        assert_eq!(check_state("queued", None), json!({"name":"PENDING"}));
        assert_eq!(
            check_state("in_progress", None),
            json!({"name":"IN_PROGRESS"})
        );
        assert_eq!(status_state("error"), "FAILED");
        assert_eq!(status_state("pending"), "INPROGRESS");
    }
}
