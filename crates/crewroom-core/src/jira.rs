//! Jira intake records. Provider transport and credentials are separate concerns.
use crate::{
    Result,
    source_time::parse as source_timestamp,
    store::{Store, conflict, invalid, trim_text},
    ticket_source::{JIRA, SourceObservation},
};
use serde_json::{Value, json};
use std::collections::HashSet;

fn string(value: &Value, min: usize, max: usize, trim: bool) -> Result<String> {
    let value = value.as_str().ok_or_else(|| invalid("Expected text."))?;
    let value = if trim { trim_text(value) } else { value };
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid("Text is outside its permitted size."));
    }
    Ok(value.to_owned())
}
fn description(value: &Value, depth: usize) -> String {
    if let Some(s) = value.as_str() {
        return s.to_owned();
    }
    if depth > 30
        || !value.is_object()
        || ["text", "type"]
            .iter()
            .any(|key| value.get(*key).is_some_and(|v| !v.is_string()))
        || value.get("content").is_some_and(|v| !v.is_array())
    {
        return String::new();
    }
    let kind = value["type"].as_str().unwrap_or("");
    if kind == "hardBreak" {
        return "\n".into();
    }
    let mut content = value["text"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            value["content"]
                .as_array()
                .map(|a| a.iter().map(|v| description(v, depth + 1)).collect())
                .unwrap_or_default()
        });
    if ["paragraph", "heading", "listItem"].contains(&kind) {
        content.push('\n');
    }
    content
}
fn issue_input(issue: &Value) -> Result<Value> {
    let issue_id = string(&issue["id"], 1, 200, false)?;
    let key = string(&issue["key"], 1, usize::MAX, false)?;
    let (project, number) = key
        .rsplit_once('-')
        .ok_or_else(|| invalid("Invalid Jira issue key."))?;
    if project.is_empty()
        || !project.as_bytes()[0].is_ascii_alphabetic()
        || !project
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        || number.is_empty()
        || !number.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(invalid("Invalid Jira issue key."));
    }
    let raw = &issue["fields"];
    let mut status = json!({"name":string(&raw["status"]["name"],1,80,true)?});
    if let Some(id) = raw["status"].get("id") {
        status["id"] = json!(string(id, 0, usize::MAX, false)?);
    }
    let mut fields = json!({"summary":string(&raw["summary"],1,300,true)?,"status":status});
    if let Some(description) = raw.get("description") {
        fields["description"] = description.clone();
    }
    if let Some(updated) = raw.get("updated") {
        let updated = string(updated, 0, usize::MAX, false)?;
        if source_timestamp(&updated).is_none() {
            return Err(invalid("Invalid source timestamp."));
        }
        fields["updated"] = json!(updated);
    }
    for (key, max, nullable) in [("priority", 80, true), ("project", 100, false)] {
        if let Some(value) = raw.get(key) {
            if nullable && value.is_null() {
                fields[key] = Value::Null;
            } else {
                fields[key] = json!({"name":string(&value["name"],0,max,false)?});
            }
        }
    }
    Ok(json!({"id":issue_id,"key":key,"fields":fields}))
}
fn issues_input(value: &Value, min: usize, unique: bool) -> Result<Vec<Value>> {
    let values = value
        .as_array()
        .filter(|a| (min..=1000).contains(&a.len()))
        .ok_or_else(|| invalid("Invalid Jira batch size."))?;
    let mut seen = HashSet::new();
    let mut normalized = vec![];
    for value in values {
        let issue = issue_input(value)?;
        if unique && !seen.insert(issue["id"].clone().to_string()) {
            return Err(invalid("The import contains duplicate issue IDs."));
        }
        normalized.push(issue);
    }
    Ok(normalized)
}
fn site_origin(value: &Value, import: bool) -> Result<String> {
    let site = url::Url::parse(
        value
            .as_str()
            .ok_or_else(|| invalid("Invalid Jira site address."))?,
    )
    .map_err(|_| invalid("Invalid Jira site address."))?;
    if import
        && (site.scheme() != "https"
            || !site.username().is_empty()
            || site.password().is_some()
            || site.query().is_some_and(|v| !v.is_empty())
            || site.fragment().is_some_and(|v| !v.is_empty())
            || site.path() != "/")
    {
        return Err(invalid(
            "Use your Jira HTTPS site address, without a path or credentials.",
        ));
    }
    Ok(site.origin().ascii_serialization())
}
/// Normalize provider input with the same schema used by durable intake.
pub fn normalize_issue(value: &Value) -> Result<Value> {
    issue_input(value)
}
pub fn normalize_site(value: &Value) -> Result<String> {
    site_origin(value, true)
}
pub fn description_text(value: &Value) -> String {
    description(value, 0)
}
impl Store {
    pub fn import_jira(&self, body: &Value) -> Result<Value> {
        let site = site_origin(&body["siteUrl"], true)?;
        let issues = issues_input(&body["issues"], 1, true)?;
        self.transaction(|s| Ok(json!(s.import_jira_rows(&site, &issues)?)))
    }
    fn import_jira_rows(&self, site: &str, issues: &[Value]) -> Result<Vec<Value>> {
        let observations = issues
            .iter()
            .map(|issue| {
                let fields = &issue["fields"];
                let description = description(&fields["description"], 0);
                SourceObservation {
                    id: issue["id"].clone(),
                    key: issue["key"].clone(),
                    title: fields["summary"].as_str().unwrap().to_owned(),
                    description: trim_text(&description).to_owned(),
                    status: fields["status"]["name"].clone(),
                    status_id: fields["status"].get("id").cloned(),
                    priority: fields["priority"]["name"]
                        .as_str()
                        .unwrap_or("Unspecified")
                        .to_owned(),
                    project: fields["project"]["name"].as_str().unwrap_or("").to_owned(),
                    updated_at: fields.get("updated").cloned(),
                    url: format!("{site}/browse/{}", issue["key"].as_str().unwrap()),
                }
            })
            .collect::<Vec<_>>();
        self.import_source_rows(JIRA, site, &observations)
    }
    pub fn jira_state(&self) -> Result<Option<Value>> {
        self.record("jira_sync", "id", "1")
    }
    pub fn commit_jira_sync(
        &self,
        source: &Value,
        input: &Value,
        retained: &Value,
    ) -> Result<Value> {
        let issues = issues_input(input, 0, true)?;
        let retained = issues_input(retained, 0, false)?;
        let mut seen = HashSet::new();
        if issues
            .iter()
            .chain(&retained)
            .any(|i| !seen.insert(i["id"].clone().to_string()))
        {
            return Err(conflict("Jira synchronization repeated an issue."));
        }
        let site = site_origin(&source["siteUrl"], false)?;
        self.transaction(|s| {
            s.import_jira_rows(&site,&retained)?;let tasks=s.import_jira_rows(&site,&issues)?;let at=s.time();
            let state=json!({"source":source,"state":"connected","taskIds":tasks.iter().map(|t|t["id"].clone()).collect::<Vec<_>>(),"lastSyncedAt":at,"lastAttemptAt":at});
            s.db.execute("INSERT INTO jira_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",[state.to_string()])?;Ok(state)
        })
    }
    pub fn fail_jira_sync(&self, source: &Value, error: &str) -> Result<()> {
        let saved = self.jira_state()?;
        let expected = crate::serialized::stringify(source);
        // The retained source identity compares serialized key order as well as values.
        let mut state = if let Some(saved) = saved.filter(|s| {
            let actual = crate::serialized::stringify(&s["source"]);
            actual == expected
        }) {
            saved
        } else {
            json!({"taskIds":[]})
        };
        state["source"] = source.clone();
        state["state"] = json!("unavailable");
        state["error"] = json!(error);
        state["lastAttemptAt"] = json!(self.time());
        self.db.execute(
            "INSERT INTO jira_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            [state.to_string()],
        )?;
        Ok(())
    }
}
