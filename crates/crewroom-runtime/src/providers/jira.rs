use super::{
    schema::*,
    transport::{Transport, fixture_origin},
    *,
};
use serde_json::{Value, json};
use std::collections::HashSet;
const OPTIONAL: [&str; 6] = [
    "inProgressStatusId",
    "inProgressTransitionId",
    "reviewStatusId",
    "reviewTransitionId",
    "doneStatusId",
    "doneTransitionId",
];
pub fn source(value: &Value) -> Result<Value> {
    strict(
        value,
        &[
            "siteUrl",
            "cloudId",
            "filterId",
            "assigneeAccountId",
            "name",
            "inProgressStatusId",
            "inProgressTransitionId",
            "reviewStatusId",
            "reviewTransitionId",
            "doneStatusId",
            "doneTransitionId",
        ],
    )?;
    let site = crewroom_core::jira::normalize_site(&value["siteUrl"]).map_err(|_| invalid())?;
    let cloud = string(&value["cloudId"])?;
    if !crewroom_core::store::valid_uuid(&cloud) {
        return Err(invalid());
    }
    let mut result = json!({"siteUrl":site,"cloudId":cloud,"filterId":digits(&value["filterId"])?,"assigneeAccountId":account(&value["assigneeAccountId"])?,"name":text(&value["name"],1,120,true)?});
    for key in OPTIONAL {
        if let Some(value) = value.get(key) {
            result[key] = json!(digits(value)?);
        }
    }
    Ok(result)
}
fn issue(value: &Value) -> Result<Value> {
    crewroom_core::jira::normalize_issue(value).map_err(|_| invalid())
}
#[derive(Clone)]
pub struct JiraClient {
    pub source: Value,
    transport: Transport,
}
impl JiraClient {
    pub fn new(input: &Value, credentials: Credentials, fixture: Option<&str>) -> Result<Self> {
        let source = source(input)?;
        let base = match fixture {
            Some(value) => fixture_origin(value, Provider::Jira)?,
            None => format!(
                "https://api.atlassian.com/ex/jira/{}",
                source["cloudId"].as_str().unwrap()
            ),
        };
        let transport = Transport::new(Provider::Jira, format!("{base}/rest/api/3/"), credentials)?;
        Ok(Self { source, transport })
    }
    pub async fn request(
        &self,
        path: &str,
        method: reqwest::Method,
        body: Option<Value>,
        before: Option<&BeforeDispatch>,
    ) -> Result<Option<Value>> {
        self.transport.request(path, method, body, before).await
    }
    async fn get(&self, path: &str) -> Result<Value> {
        Ok(self
            .request(path, reqwest::Method::GET, None, None)
            .await?
            .unwrap_or(Value::Null))
    }
    pub async fn issue(&self, id: &str) -> Result<Value> {
        let value = issue(
            &self
                .get(&format!(
                    "issue/{}?fields=summary,description,status,priority,project,updated",
                    component(id)
                ))
                .await?,
        )?;
        if value["id"] != id {
            return Err(unavailable("Jira returned a different issue identity."));
        }
        Ok(value)
    }
    pub async fn comments(&self, id: &str) -> Result<Vec<Value>> {
        let mut comments = vec![];
        let mut start = 0;
        loop {
            let page = self
                .get(&format!(
                    "issue/{}/comment?startAt={start}&maxResults=100&orderBy=created",
                    component(id)
                ))
                .await?;
            let rows = array(&page["comments"])?;
            let at = integer(&page["startAt"], -9_007_199_254_740_991)?;
            let total = integer(&page["total"], 0)?;
            let rows=rows.iter().map(|row|{object(row)?;Ok(json!({"id":string(&row["id"])?,"body":row.get("body").ok_or_else(invalid)?}))}).collect::<Result<Vec<_>>>()?;
            if at != start as i64 || comments.len() + rows.len() > 10000 {
                return Err(unavailable(
                    "Jira comment reconciliation exceeded its bounds.",
                ));
            }
            let count = rows.len();
            comments.extend(rows);
            if comments.len() as i64 >= total {
                return Ok(comments);
            }
            if count == 0 {
                return Err(unavailable("Jira comment pagination did not complete."));
            }
            start += count;
        }
    }
    pub async fn add_comment(
        &self,
        id: &str,
        body: Value,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        let result = self
            .request(
                &format!("issue/{}/comment", component(id)),
                reqwest::Method::POST,
                Some(json!({"body":body})),
                before,
            )
            .await?
            .unwrap_or(Value::Null);
        Ok(json!({"id":text(&result["id"],1,usize::MAX,false)?}))
    }
    pub async fn transitions(&self, id: &str) -> Result<Vec<Value>> {
        let response = self
            .get(&format!(
                "issue/{}/transitions?expand=transitions.fields",
                component(id)
            ))
            .await?;
        array(&response["transitions"])?
            .iter()
            .map(|row| {
                let mut result =
                    json!({"id":string(&row["id"])?,"to":{"id":string(&row["to"]["id"])?}});
                if let Some(fields) = row.get("fields") {
                    let mut normalized = json!({});
                    for (key, value) in object(fields)? {
                        object(value)?;
                        let mut field = json!({});
                        if let Some(required) = value.get("required") {
                            field["required"] = json!(boolean(required)?);
                        }
                        normalized[key] = field;
                    }
                    result["fields"] = normalized;
                }
                Ok(result)
            })
            .collect()
    }
    pub async fn transition(
        &self,
        id: &str,
        transition_id: &str,
        before: Option<&BeforeDispatch>,
    ) -> Result<()> {
        self.request(
            &format!("issue/{}/transitions", component(id)),
            reqwest::Method::POST,
            Some(json!({"transition":{"id":transition_id}})),
            before,
        )
        .await?;
        Ok(())
    }
    pub async fn search(&self) -> Result<Vec<Value>> {
        let mut issues = vec![];
        let mut identities = HashSet::new();
        let mut tokens = HashSet::new();
        let mut next: Option<String> = None;
        loop {
            let jql = format!(
                "filter = {} AND assignee = {} ORDER BY Rank ASC",
                self.source["filterId"].as_str().unwrap(),
                crewroom_core::serialized::stringify(&self.source["assigneeAccountId"])
            );
            let query = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query
                    .append_pair("jql", &jql)
                    .append_pair(
                        "fields",
                        "summary,description,status,priority,project,updated",
                    )
                    .append_pair("maxResults", "100");
                if let Some(next) = &next {
                    query.append_pair("nextPageToken", next);
                }
                query.finish()
            };
            let page = self.get(&format!("search/jql?{query}")).await?;
            let rows = array(&page["issues"])?
                .iter()
                .map(issue)
                .collect::<Result<Vec<_>>>()?;
            let last = boolean(&page["isLast"])?;
            let token = page.get("nextPageToken").map(string).transpose()?;
            for row in rows {
                if issues.len() >= 1000
                    || !identities.insert(row["id"].as_str().unwrap().to_owned())
                {
                    return Err(unavailable(
                        "Jira search exceeded the intake limit or repeated an issue. No partial import was saved.",
                    ));
                }
                issues.push(row);
            }
            if last {
                return Ok(issues);
            }
            let token = token.filter(|s| !s.is_empty()).ok_or_else(|| {
                unavailable("Jira pagination did not complete. No partial import was saved.")
            })?;
            if tokens.len() >= 100 || !tokens.insert(token.clone()) {
                return Err(unavailable(
                    "Jira pagination did not complete. No partial import was saved.",
                ));
            }
            next = Some(token);
        }
    }
}
