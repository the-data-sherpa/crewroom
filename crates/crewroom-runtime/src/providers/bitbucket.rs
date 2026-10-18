pub use super::bitbucket_schema::repository;
use super::{
    bitbucket_schema as wire,
    schema::*,
    transport::{Transport, fixture_origin},
    *,
};
use serde_json::{Value, json};
use std::collections::HashSet;
#[derive(Clone)]
pub struct BitbucketClient {
    pub repository: Value,
    origin: String,
    prefix: String,
    transport: Transport,
}
impl BitbucketClient {
    pub fn new(input: &Value, credentials: Credentials, fixture: Option<&str>) -> Result<Self> {
        let repository = repository(input)?;
        let origin = fixture
            .map(|s| fixture_origin(s, Provider::Bitbucket))
            .transpose()?
            .unwrap_or_else(|| "https://api.bitbucket.org".into());
        let prefix = format!(
            "/2.0/repositories/{}/{}",
            repository["workspace"].as_str().unwrap(),
            repository["slug"].as_str().unwrap()
        );
        let transport = Transport::new(
            Provider::Bitbucket,
            format!("{origin}{prefix}"),
            credentials,
        )?;
        Ok(Self {
            repository,
            origin,
            prefix,
            transport,
        })
    }
    async fn get(&self, path: &str) -> Result<Value> {
        Ok(self
            .transport
            .request(path, reqwest::Method::GET, None, None)
            .await?
            .unwrap_or(Value::Null))
    }
    async fn pages(
        &self,
        mut path: String,
        mut on_page: impl FnMut(&[Value]) -> Result<()> + Send,
    ) -> Result<Vec<Value>> {
        let endpoint = url::Url::parse(&format!("{}{}{path}", self.origin, self.prefix))
            .map_err(|_| invalid())?
            .path()
            .to_owned();
        let mut visited = HashSet::new();
        let mut values = vec![];
        loop {
            if visited.len() >= 100 || !visited.insert(path.clone()) {
                return Err(unavailable("Bitbucket pagination did not complete."));
            }
            let page = self.get(&path).await?;
            let rows = array(&page["values"])?;
            let next = page.get("next").map(url_text).transpose()?;
            on_page(rows)?;
            values.extend(rows.iter().cloned());
            if values.len() > 10000 {
                return Err(unavailable("Bitbucket pagination exceeded its item limit."));
            }
            let Some(next) = next else { return Ok(values) };
            let next = url::Url::parse(&next).map_err(|_| invalid())?;
            if next.origin().ascii_serialization() != self.origin
                || next.path() != endpoint
                || !next.username().is_empty()
                || next.password().is_some()
                || next.fragment().is_some_and(|s| !s.is_empty())
            {
                return Err(unavailable(
                    "Bitbucket pagination left its configured endpoint.",
                ));
            }
            path = next
                .path()
                .strip_prefix(&self.prefix)
                .ok_or_else(invalid)?
                .to_owned();
            if let Some(query) = next.query().filter(|s| !s.is_empty()) {
                path.push('?');
                path.push_str(query);
            }
        }
    }
    async fn all(&self, path: String) -> Result<Vec<Value>> {
        self.pages(path, |_| Ok(())).await
    }
    async fn full_hash(&self, value: &str) -> Result<String> {
        if hash(&json!(value), false).is_ok() {
            return Ok(value.into());
        }
        let commit = self.get(&format!("/commit/{value}")).await?;
        let full = hash(&commit["hash"], false)
            .map_err(|_| unavailable("Bitbucket could not resolve a pull request commit."))?;
        if !full.starts_with(value) {
            return Err(unavailable(
                "Bitbucket could not resolve a pull request commit.",
            ));
        }
        Ok(full)
    }
    async fn parse_pr(&self, value: &Value) -> Result<Value> {
        let mut pr = wire::pull_request(value)
            .map_err(|_| unavailable("Bitbucket returned an unexpected pull request."))?;
        let expected = format!(
            "https://bitbucket.org/{}/{}/pull-requests/{}",
            self.repository["workspace"].as_str().unwrap(),
            self.repository["slug"].as_str().unwrap(),
            pr["id"].as_i64().unwrap()
        );
        if pr["source"]["repository"]["uuid"] != self.repository["uuid"]
            || pr["destination"]["repository"]["uuid"] != self.repository["uuid"]
            || pr["links"]["html"]["href"] != expected
        {
            return Err(unavailable(
                "Bitbucket returned a different repository or PR identity.",
            ));
        }
        for key in ["source", "destination"] {
            let value = self
                .full_hash(pr[key]["commit"]["hash"].as_str().unwrap())
                .await?;
            pr[key]["commit"]["hash"] = json!(value);
        }
        if pr.get("merge_commit").is_some_and(|v| !v.is_null()) {
            let value = self
                .full_hash(pr["merge_commit"]["hash"].as_str().unwrap())
                .await?;
            pr["merge_commit"]["hash"] = json!(value);
        }
        Ok(pr)
    }
    pub async fn verify_repository(&self) -> Result<Value> {
        let raw = self.get("").await?;
        let mut result = wire::repository_identity(&raw)?;
        result["full_name"] = json!(string(&raw["full_name"])?);
        if result["uuid"] != self.repository["uuid"]
            || result["full_name"]
                != format!(
                    "{}/{}",
                    self.repository["workspace"].as_str().unwrap(),
                    self.repository["slug"].as_str().unwrap()
                )
        {
            return Err(unavailable(
                "Bitbucket repository identity changed. Publication is blocked.",
            ));
        }
        Ok(result)
    }
    pub async fn pull_requests(&self, branch: &str, target: &str) -> Result<Vec<Value>> {
        let query = {
            let mut q = url::form_urlencoded::Serializer::new(String::new());
            q.append_pair(
                "q",
                &format!(
                    "source.branch.name = {} AND destination.branch.name = {}",
                    crewroom_core::serialized::stringify(&json!(branch)),
                    crewroom_core::serialized::stringify(&json!(target))
                ),
            )
            .append_pair("pagelen", "50");
            for state in ["OPEN", "MERGED", "DECLINED", "SUPERSEDED"] {
                q.append_pair("state", state);
            }
            q.finish()
        };
        let rows = self.all(format!("/pullrequests?{query}")).await?;
        let prs =
            futures_util::future::try_join_all(rows.iter().map(|row| self.parse_pr(row))).await?;
        let mut ids = HashSet::new();
        if prs.iter().any(|pr| {
            !ids.insert(pr["id"].as_i64().unwrap())
                || pr["source"]["branch"]["name"] != branch
                || pr["destination"]["branch"]["name"] != target
        }) {
            return Err(unavailable("Bitbucket returned an inconsistent PR search."));
        }
        Ok(prs)
    }
    pub async fn pull_request(&self, id: i64) -> Result<Value> {
        integer(&json!(id), 1)?;
        let pr = self
            .parse_pr(&self.get(&format!("/pullrequests/{id}")).await?)
            .await?;
        if pr["id"] != id {
            return Err(unavailable("Bitbucket returned a different PR identity."));
        }
        Ok(pr)
    }
    pub async fn comments(&self, id: i64) -> Result<Value> {
        integer(&json!(id), 1)?;
        let mut comments = vec![];
        let mut ids = HashSet::new();
        let result = self
            .pages(format!("/pullrequests/{id}/comments?pagelen=100"), |rows| {
                for row in rows {
                    let comment = wire::comment(row)
                        .map_err(|_| unavailable("Bitbucket returned inconsistent PR comments."))?;
                    if !ids.insert(comment["id"].as_i64().unwrap())
                        || comment.get("pullrequest").is_some_and(|v| v["id"] != id)
                    {
                        return Err(unavailable("Bitbucket returned inconsistent PR comments."));
                    }
                    comments.push(comment);
                }
                Ok(())
            })
            .await;
        Ok(match result {
            Ok(_) => json!({"state":"complete","comments":comments}),
            Err(error) => {
                json!({"state":if comments.is_empty(){"unavailable"}else{"partial"},"comments":comments,"reason":error.to_string()})
            }
        })
    }
    pub async fn observe(&self, id: i64) -> Result<Value> {
        let pr = self.pull_request(id).await?;
        let head = pr["source"]["commit"]["hash"].as_str().unwrap();
        let statuses = self
            .all(format!("/commit/{head}/statuses?pagelen=100"))
            .await?
            .iter()
            .map(wire::status)
            .collect::<Result<Vec<_>>>()?;
        let mut ids = HashSet::new();
        if statuses
            .iter()
            .any(|s| !ids.insert(s["key"].as_str().unwrap()))
        {
            return Err(unavailable("Bitbucket repeated a commit status identity."));
        }
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("target.commit.hash", head)
            .append_pair("pagelen", "100")
            .append_pair("sort", "-created_on")
            .finish();
        let rows = self
            .all(format!("/pipelines/?{query}"))
            .await?
            .iter()
            .map(|v| {
                wire::pipeline(v)
                    .map_err(|_| unavailable("Bitbucket returned an unexpected pipeline."))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut matching = rows
            .into_iter()
            .filter(|p| {
                let t = &p["target"];
                t["type"] == "pipeline_pullrequest_target"
                    && t["commit"]["hash"] == head
                    && t["pullrequest"]["id"] == id
                    && t["source"] == pr["source"]["branch"]["name"]
                    && t["destination"] == pr["destination"]["branch"]["name"]
                    && t["selector"]["type"] == "pull-requests"
            })
            .collect::<Vec<_>>();
        matching.sort_by(|a, b| {
            b["build_number"]
                .as_i64()
                .unwrap()
                .cmp(&a["build_number"].as_i64().unwrap())
                .then_with(|| {
                    b["run_number"]
                        .as_i64()
                        .unwrap_or(0)
                        .cmp(&a["run_number"].as_i64().unwrap_or(0))
                })
                .then_with(|| {
                    crewroom_core::source_time::parse(b["created_on"].as_str().unwrap()).cmp(
                        &crewroom_core::source_time::parse(a["created_on"].as_str().unwrap()),
                    )
                })
        });
        let mut pipelines = vec![];
        if let Some(mut latest) = matching.into_iter().next() {
            if latest["target"].get("destination_commit").is_some() {
                let full = self
                    .full_hash(
                        latest["target"]["destination_commit"]["hash"]
                            .as_str()
                            .unwrap(),
                    )
                    .await?;
                latest["target"]["destination_commit"]["hash"] = json!(full);
            }
            let steps = self
                .all(format!(
                    "/pipelines/{}/steps/?pagelen=100",
                    component(latest["uuid"].as_str().unwrap())
                ))
                .await?
                .iter()
                .map(wire::step)
                .collect::<Result<Vec<_>>>()?;
            let mut ids = HashSet::new();
            if steps
                .iter()
                .any(|s| !ids.insert(s["uuid"].as_str().unwrap()))
            {
                return Err(unavailable("Bitbucket repeated a pipeline step identity."));
            }
            latest["steps"] = json!(steps);
            pipelines.push(latest);
        }
        let current = self.pull_request(id).await?;
        if crewroom_core::serialized::stringify(&current)
            != crewroom_core::serialized::stringify(&pr)
        {
            return Err(unavailable(
                "PR changed while hosted evidence was being read. Refresh before assessing readiness.",
            ));
        }
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
        let body = json!({"title":input["title"],"description":input["description"],"source":{"branch":{"name":input["branch"]},"commit":{"hash":input["head"]}},"destination":{"branch":{"name":input["target"]}},"close_source_branch":false});
        let value = self
            .transport
            .request("/pullrequests", reqwest::Method::POST, Some(body), before)
            .await?
            .unwrap_or(Value::Null);
        self.parse_pr(&value).await.map_err(sent)
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
                &format!("/pullrequests/{id}"),
                reqwest::Method::PUT,
                Some(json!({"title":input["title"],"description":input["description"]})),
                before,
            )
            .await?
            .unwrap_or(Value::Null);
        let pr = self.parse_pr(&value).await.map_err(sent)?;
        if pr["id"] != id {
            return Err(sent(unavailable(
                "Bitbucket returned a different PR identity.",
            )));
        }
        Ok(pr)
    }
}
fn sent(error: Error) -> Error {
    Error::Unavailable(Unavailable {
        message: error.to_string(),
        status: None,
        request_sent: true,
    })
}
