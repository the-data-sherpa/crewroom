//! A project's confirmed Jira site drives that project's Jira refresh (FOR-53).
//!
//! Each project whose confirmed ticket source (FOR-47) is a Jira site has its
//! own refresh record: its filters (a Jira project key, a saved filter, an
//! assignee and extra JQL, combined with AND), the last refresh result and the
//! tickets it matched. Projects never share a record, so they never share
//! filters. A refresh imports through the provider-neutral
//! `import_source_rows`; tickets first imported by a project's refresh target
//! that project. Jira status is stored as the ticket's source status only;
//! nothing here reads or writes Crewroom workflow state or agent activity.
//! Transport and credentials stay in the service; this module validates and
//! records.
//!
//! Before schema 35, Jira refresh was one workspace-level setting: the saved
//! filter and assignee in the private Jira connection file, with its result in
//! `jira_sync`. That record is now the *workspace setting*: schema 35 and the
//! service's start-up offer copy it into each project that confirmed its site
//! and has no filters yet, then mark it adopted. If no project uses its site,
//! it is reported and never refreshes; the next project that confirms that
//! site adopts it.
use crate::{
    Result,
    jira::normalize_site,
    store::{Store, conflict, invalid, parse, strict, trim_text},
    ticket_source::JIRA,
};
use rusqlite::params;
use serde_json::{Value, json};

/// Most issues one refresh may import. Larger sources must be narrowed.
pub const MAX_ISSUES: usize = 1000;
const FILTERS: [&str; 4] = ["projectKey", "filterId", "assignee", "jql"];
const UPSERT: &str = "INSERT INTO jira_project_sync VALUES(?,?) ON CONFLICT(repository_id) DO UPDATE SET data=excluded.data";

fn optional(body: &Value, key: &str, max: usize, label: &str) -> Result<Option<String>> {
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

/// Validates a project's Jira filters. At least a project key, a saved filter
/// or JQL is required, so a project never refreshes a whole Jira site. Extra
/// JQL may not carry its own ORDER BY: tickets keep Jira's rank order.
pub fn normalize_filters(body: &Value) -> Result<Value> {
    strict(body, &FILTERS)?;
    let project = optional(body, "projectKey", 20, "Project key")?
        .map(|k| k.to_ascii_uppercase())
        .map(|k| {
            let valid = k.as_bytes()[0].is_ascii_alphabetic()
                && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
            if valid {
                Ok(k)
            } else {
                Err(invalid(
                    "Project key must start with a letter and use letters, digits or _ (for example CR).",
                ))
            }
        })
        .transpose()?;
    let filter = match &body["filterId"] {
        Value::Number(n) => Some(
            n.as_u64()
                .filter(|n| *n > 0)
                .map(|n| n.to_string())
                .ok_or_else(|| invalid("Saved filter ID must be a positive number."))?,
        ),
        _ => optional(body, "filterId", 18, "Saved filter ID")?
            .map(|f| {
                if f.bytes().all(|c| c.is_ascii_digit()) && !f.trim_start_matches('0').is_empty() {
                    Ok(f)
                } else {
                    Err(invalid("Saved filter ID must be a positive number."))
                }
            })
            .transpose()?,
    };
    let assignee = optional(body, "assignee", 128, "Assignee")?
        .map(|a| {
            if a.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
            {
                Ok(a)
            } else {
                Err(invalid("Assignee must be a Jira account ID."))
            }
        })
        .transpose()?;
    let jql = optional(body, "jql", 2000, "JQL")?
        .map(|q| {
            if q.chars().any(char::is_control) {
                Err(invalid("JQL must be one line of text."))
            } else if q.to_ascii_lowercase().contains("order by") {
                Err(invalid(
                    "Leave ORDER BY out of the JQL; tickets keep Jira's rank order.",
                ))
            } else {
                Ok(q)
            }
        })
        .transpose()?;
    if project.is_none() && filter.is_none() && jql.is_none() {
        return Err(invalid(
            "Enter a Jira project key, a saved filter ID or JQL, so this project does not import the whole site.",
        ));
    }
    Ok(json!({"projectKey":project,"filterId":filter,"assignee":assignee,"jql":jql}))
}

/// Whether a saved source names any filter. A newly confirmed site has none
/// until the operator saves them or the workspace setting is adopted.
pub fn has_filters(source: &Value) -> bool {
    ["projectKey", "filterId", "jql"]
        .iter()
        .any(|k| source[*k].as_str().is_some_and(|v| !v.is_empty()))
}

/// The JQL a project's refresh sends: every saved filter, combined with AND,
/// in Jira's rank order. `None` when the source has no filters.
pub fn jql(source: &Value) -> Option<String> {
    if !has_filters(source) {
        return None;
    }
    let mut clauses = vec![];
    if let Some(filter) = source["filterId"].as_str() {
        clauses.push(format!("filter = {filter}"));
    }
    if let Some(project) = source["projectKey"].as_str() {
        clauses.push(format!(
            "project = {}",
            crate::serialized::stringify(&json!(project))
        ));
    }
    if let Some(assignee) = source["assignee"].as_str() {
        clauses.push(format!(
            "assignee = {}",
            crate::serialized::stringify(&json!(assignee))
        ));
    }
    if let Some(extra) = source["jql"].as_str() {
        clauses.push(format!("({extra})"));
    }
    Some(format!("{} ORDER BY Rank ASC", clauses.join(" AND ")))
}

/// Operator-facing text for a source's filters.
pub fn filters_text(source: &Value) -> String {
    let mut parts = vec![];
    for (key, label) in [
        ("projectKey", "project"),
        ("filterId", "saved filter"),
        ("assignee", "assignee"),
        ("jql", "JQL"),
    ] {
        if let Some(value) = source[key].as_str() {
            parts.push(format!("{label} {value}"));
        }
    }
    if parts.is_empty() {
        "none saved".into()
    } else {
        parts.join(" · ")
    }
}

fn empty_source(site: &str) -> Value {
    json!({"siteUrl":site,"projectKey":null,"filterId":null,"assignee":null,"jql":null})
}

fn fresh(source: Value, at: String) -> Value {
    json!({"source":source,"state":"not_synced","taskIds":[],"savedAt":at})
}

/// The project-level source equivalent to a workspace-level Jira setting: its
/// saved filter and assignee on its site. `None` when it names no filter.
pub fn workspace_source(setting: &Value) -> Option<Value> {
    let site = normalize_site(&setting["siteUrl"]).ok()?;
    let filter = setting["filterId"].as_str().filter(|f| !f.is_empty())?;
    let mut source = empty_source(&site);
    source["filterId"] = json!(filter);
    if let Some(assignee) = setting["assigneeAccountId"]
        .as_str()
        .filter(|a| !a.is_empty())
    {
        source["assignee"] = json!(assignee);
    }
    Some(source)
}

fn same_site(a: &Value, b: &str) -> bool {
    a.as_str().is_some_and(|a| a.eq_ignore_ascii_case(b))
}

impl Store {
    /// Every project's Jira refresh record, ordered by project id. Each
    /// carries its `repositoryId`.
    pub fn jira_project_sources(&self) -> Result<Vec<Value>> {
        let mut statement = self
            .db
            .prepare("SELECT repository_id,data FROM jira_project_sync ORDER BY repository_id")?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, data)| {
                let mut state = parse(data)?;
                state["repositoryId"] = json!(id);
                Ok(state)
            })
            .collect()
    }
    /// One project's Jira refresh record, if its confirmed source is Jira.
    pub fn jira_project_source(&self, repository_id: &str) -> Result<Option<Value>> {
        Ok(self
            .record("jira_project_sync", "repository_id", repository_id)?
            .map(|mut state| {
                state["repositoryId"] = json!(repository_id);
                state
            }))
    }
    /// The workspace-level Jira refresh setting from before schema 35, while
    /// no project has adopted it. It never refreshes.
    pub fn legacy_jira_setting(&self) -> Result<Option<Value>> {
        Ok(self
            .jira_state()?
            .filter(|s| s.get("adoptedBy").is_none() && workspace_source(&s["source"]).is_some()))
    }
    fn put_jira_source(&self, repository_id: &str, state: &Value) -> Result<()> {
        let mut data = state.clone();
        if let Some(fields) = data.as_object_mut() {
            fields.remove("repositoryId");
        }
        self.db
            .execute(UPSERT, params![repository_id, data.to_string()])?;
        Ok(())
    }
    fn mark_legacy_adopted(&self, legacy: &Value, by: &[String]) -> Result<()> {
        let mut record = legacy.clone();
        record["adoptedBy"] = json!(by);
        record["adoptedAt"] = json!(self.time());
        self.db.execute(
            "INSERT INTO jira_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            [record.to_string()],
        )?;
        Ok(())
    }
    /// A project's record carrying the workspace setting's filters and last
    /// refresh result.
    fn adopted(&self, legacy: &Value, site: &str) -> Option<Value> {
        let mut source = workspace_source(&legacy["source"])?;
        source["siteUrl"] = json!(site);
        let mut state = json!({
            "source": source,
            "state": legacy["state"].as_str().filter(|s| *s == "connected").unwrap_or("not_synced"),
            "taskIds": legacy["taskIds"].as_array().cloned().unwrap_or_default(),
            "adoptedFrom": "workspace",
            "savedAt": self.time(),
        });
        for key in ["lastSyncedAt", "lastAttemptAt"] {
            if let Some(value) = legacy.get(key) {
                state[key] = value.clone();
            }
        }
        if let Some(name) = legacy["source"]["name"].as_str() {
            state["adoptedName"] = json!(name);
        }
        Some(state)
    }
    /// The Jira site a project confirmed as its ticket source.
    fn jira_project(&self, repository_id: &str) -> Result<String> {
        let repository = self.repository_by_id(repository_id)?;
        if repository.get("retiredAt").is_some() {
            return Err(conflict(
                "Retired repositories cannot refresh a Jira ticket source.",
            ));
        }
        let source = &repository["ticketSource"];
        if source["tickets"] != JIRA.kind {
            return Err(conflict(
                "Confirm a Jira site as this project's ticket source under Settings → Repositories first.",
            ));
        }
        source["site"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| conflict("The project's Jira source has no site."))
    }
    /// Gives a project the Jira refresh record its confirmed source implies.
    /// Confirming the same site keeps its filters and last refresh; another
    /// site adopts the unadopted workspace setting for that site, or starts
    /// with no filters. GitHub Issues has no Jira record, so refresh from the
    /// old site stops. Imported tickets are never rewritten.
    pub(crate) fn attach_jira_source(&self, repository_id: &str, choice: &Value) -> Result<()> {
        let Some(site) = choice["site"]
            .as_str()
            .filter(|_| choice["tickets"] == JIRA.kind)
        else {
            self.db.execute(
                "DELETE FROM jira_project_sync WHERE repository_id=?",
                [repository_id],
            )?;
            return Ok(());
        };
        if let Some(saved) = self.jira_project_source(repository_id)?
            && same_site(&saved["source"]["siteUrl"], site)
        {
            return Ok(());
        }
        let legacy = self.legacy_jira_setting()?.filter(|l| {
            workspace_source(&l["source"]).is_some_and(|s| same_site(&s["siteUrl"], site))
        });
        let state = match legacy.as_ref().and_then(|l| self.adopted(l, site)) {
            Some(state) => {
                self.mark_legacy_adopted(legacy.as_ref().unwrap(), &[repository_id.to_owned()])?;
                state
            }
            None => fresh(empty_source(site), self.time()),
        };
        self.put_jira_source(repository_id, &state)
    }
    /// Copies the unadopted workspace setting into every project that
    /// confirmed its site and has no filters of its own. Returns the projects
    /// that adopted it; none leaves it reported and unused.
    fn adopt_legacy_jira_setting(&self) -> Result<Vec<String>> {
        let Some(legacy) = self.legacy_jira_setting()? else {
            return Ok(vec![]);
        };
        let Some(site) = workspace_source(&legacy["source"]).map(|s| s["siteUrl"].clone()) else {
            return Ok(vec![]);
        };
        let mut statement = self
            .db
            .prepare("SELECT id,data FROM repositories ORDER BY rowid")?;
        let repositories = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut adopted = vec![];
        for (id, data) in repositories {
            let repository = parse(data)?;
            let source = &repository["ticketSource"];
            if repository.get("retiredAt").is_some() || source["tickets"] != JIRA.kind {
                continue;
            }
            let Some(project_site) = source["site"].as_str().filter(|s| same_site(&site, s)) else {
                continue;
            };
            if self
                .jira_project_source(&id)?
                .is_some_and(|s| has_filters(&s["source"]))
            {
                continue;
            }
            if let Some(state) = self.adopted(&legacy, project_site) {
                self.put_jira_source(&id, &state)?;
                adopted.push(id);
            }
        }
        if !adopted.is_empty() {
            self.mark_legacy_adopted(&legacy, &adopted)?;
        }
        Ok(adopted)
    }
    /// Schema 35: every project whose confirmed ticket source is a Jira site
    /// gets its own refresh record, and the workspace-level setting moves into
    /// the projects that confirmed its site.
    pub(crate) fn migrate_jira_project_sources(&self) -> Result<()> {
        let mut statement = self
            .db
            .prepare("SELECT id,data FROM repositories ORDER BY rowid")?;
        let repositories = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, data) in repositories {
            let repository = parse(data)?;
            let source = &repository["ticketSource"];
            if let Some(site) = source["site"]
                .as_str()
                .filter(|_| source["tickets"] == JIRA.kind)
                && self.jira_project_source(&id)?.is_none()
            {
                self.put_jira_source(&id, &fresh(empty_source(site), self.time()))?;
            }
        }
        self.adopt_legacy_jira_setting()?;
        Ok(())
    }
    /// Records the workspace-level setting still present in the private Jira
    /// connection file and moves it into the projects that confirmed its
    /// site. Offering the same setting again changes nothing, so an adopted
    /// setting is never copied twice. Returns the workspace setting state:
    /// `adopted` (with the projects), `unused` or `none`.
    pub fn offer_workspace_jira_setting(&self, setting: &Value) -> Result<Value> {
        let Some(offered) = workspace_source(setting) else {
            return Ok(json!({"state":"none"}));
        };
        self.transaction(|s| {
            let saved = s.jira_state()?;
            let known = saved
                .as_ref()
                .and_then(|v| workspace_source(&v["source"]))
                .is_some_and(|known| known == offered);
            if !known {
                let mut record = json!({"source":setting,"state":"not_synced","taskIds":[]});
                // The previous result belongs to the setting it was taken with.
                if let Some(saved) = saved.filter(|v| v.get("adoptedBy").is_none()) {
                    for key in ["lastSyncedAt", "lastAttemptAt"] {
                        if let Some(value) = saved.get(key) {
                            record[key] = value.clone();
                        }
                    }
                }
                s.db.execute(
                    "INSERT INTO jira_sync VALUES(1,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                    [record.to_string()],
                )?;
            }
            s.adopt_legacy_jira_setting()?;
            let current = s.jira_state()?.unwrap_or_default();
            Ok(match current.get("adoptedBy") {
                Some(by) => json!({"state":"adopted","projects":by}),
                None => json!({"state":"unused"}),
            })
        })
    }
    /// Saves a project's filters. The site is the one its confirmed ticket
    /// source names. Changed filters start as not refreshed; tickets imported
    /// earlier keep their source, refinements and history.
    pub fn save_jira_filters(&self, repository_id: &str, body: &Value) -> Result<Value> {
        let filters = normalize_filters(body)?;
        self.transaction(|s| {
            let site = s.jira_project(repository_id)?;
            let mut source = filters.clone();
            source["siteUrl"] = json!(site);
            let source = json!({
                "siteUrl": source["siteUrl"], "projectKey": source["projectKey"],
                "filterId": source["filterId"], "assignee": source["assignee"], "jql": source["jql"],
            });
            let saved = s.jira_project_source(repository_id)?;
            if let Some(saved) = &saved
                && saved["source"] == source
            {
                return Ok(saved.clone());
            }
            let mut state = fresh(source, s.time());
            state["repositoryId"] = json!(repository_id);
            s.put_jira_source(repository_id, &state)?;
            Ok(state)
        })
    }
    /// Commits one complete observation of a project's Jira source
    /// atomically. `issues` matched its filters in Jira's rank order;
    /// `retained` are this project's imported issues outside the filters that
    /// were re-read because their runs are open. Tickets imported for the
    /// first time target this project. Nothing is saved when the project's
    /// source changed meanwhile.
    pub fn commit_jira_project_sync(
        &self,
        repository_id: &str,
        source: &Value,
        issues: &Value,
        retained: &Value,
    ) -> Result<Value> {
        let matched = crate::jira::batch(issues, true)?;
        let kept = crate::jira::batch(retained, false)?;
        let mut seen = std::collections::HashSet::new();
        if matched
            .iter()
            .chain(&kept)
            .any(|i| !seen.insert(i["id"].to_string()))
        {
            return Err(conflict("Jira synchronization repeated an issue."));
        }
        self.transaction(|s| {
            let site = s.jira_project(repository_id)?;
            let saved = s.jira_project_source(repository_id)?;
            if saved.as_ref().map(|v| &v["source"]) != Some(source)
                || !same_site(&source["siteUrl"], &site)
            {
                return Err(conflict(
                    "The project's Jira source changed during refresh. Nothing was saved; refresh again.",
                ));
            }
            s.import_jira_project_rows(&site, &kept, None)?;
            let tasks = s.import_jira_project_rows(&site, &matched, Some(repository_id))?;
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
            s.put_jira_source(repository_id, &state)?;
            Ok(state)
        })
    }
    /// Records why a project's refresh could not complete (`not_connected`,
    /// `rejected`, `unreachable`, `unavailable` or `invalid`). Earlier imported
    /// tickets and the last complete refresh time are kept; nothing partial is
    /// saved. A result for a source the operator replaced meanwhile is dropped.
    pub fn fail_jira_project_sync(
        &self,
        repository_id: &str,
        source: &Value,
        reason: &str,
        error: &str,
    ) -> Result<()> {
        self.transaction(|s| {
            let Some(mut state) = s
                .jira_project_source(repository_id)?
                .filter(|v| v["source"] == *source)
            else {
                return Ok(());
            };
            state["state"] = json!("unavailable");
            state["reason"] = json!(reason);
            state["error"] = json!(error);
            state["lastAttemptAt"] = json!(s.time());
            s.put_jira_source(repository_id, &state)?;
            Ok(())
        })
    }
    /// Imported ticket order for the workspace: each project's last refresh in
    /// project order, then the workspace setting's last result.
    pub(crate) fn jira_task_order(&self) -> Result<Vec<Value>> {
        let mut order = vec![];
        for source in self.jira_project_sources()? {
            order.extend(source["taskIds"].as_array().cloned().unwrap_or_default());
        }
        if let Some(legacy) = self.jira_state()? {
            order.extend(legacy["taskIds"].as_array().cloned().unwrap_or_default());
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_normalized_and_combined_into_ranked_jql() {
        let filters = normalize_filters(&json!({
            "projectKey":" cr ","filterId":"12345","assignee":"operator-1","jql":"labels = crewroom",
        }))
        .unwrap();
        assert_eq!(filters["projectKey"], "CR");
        let mut source = filters.clone();
        source["siteUrl"] = json!("https://fixture.atlassian.net");
        assert_eq!(
            jql(&source).unwrap(),
            r#"filter = 12345 AND project = "CR" AND assignee = "operator-1" AND (labels = crewroom) ORDER BY Rank ASC"#
        );
        assert_eq!(
            filters_text(&source),
            "project CR · saved filter 12345 · assignee operator-1 · JQL labels = crewroom"
        );
        assert_eq!(
            normalize_filters(&json!({"filterId":7})).unwrap()["filterId"],
            "7"
        );
        for bad in [
            json!({}),
            json!({"assignee":"operator-1"}),
            json!({"projectKey":"1CR"}),
            json!({"projectKey":"C R"}),
            json!({"filterId":"0"}),
            json!({"filterId":"abc"}),
            json!({"projectKey":"CR","assignee":"a\"b"}),
            json!({"jql":"project = CR ORDER BY created"}),
            json!({"jql":"a\nb"}),
            json!({"projectKey":"CR","siteUrl":"https://other.example"}),
        ] {
            assert!(normalize_filters(&bad).is_err(), "{bad}");
        }
        assert!(jql(&empty_source("https://x.example")).is_none());
    }

    #[test]
    fn the_workspace_setting_maps_to_the_former_query() {
        let source = workspace_source(&json!({
            "siteUrl":"https://fixture.atlassian.net/","cloudId":"x","filterId":"12345",
            "assigneeAccountId":"operator-1","name":"Assigned work",
        }))
        .unwrap();
        assert_eq!(
            jql(&source).unwrap(),
            r#"filter = 12345 AND assignee = "operator-1" ORDER BY Rank ASC"#
        );
        assert!(workspace_source(&json!({"siteUrl":"https://fixture.atlassian.net"})).is_none());
    }
}
