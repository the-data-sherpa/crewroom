use crate::store::{conflict, integer, trim_text, valid_uuid};
use crate::{Error, Result, authority::write_private};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub struct Roles {
    path: PathBuf,
}
impl Roles {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn read(&self) -> Result<Vec<Value>> {
        let defaults: Vec<Value> = serde_json::from_str(include_str!("default-roles.json"))?;
        if !self.path.exists() {
            return Ok(defaults);
        }
        let saved: Vec<Value> = serde_json::from_slice(&std::fs::read(&self.path)?)?;
        let mut saved = saved;
        for role in &mut saved {
            if role["job"].is_null()
                && let Some(default) = defaults.iter().find(|d| d["id"] == role["id"])
            {
                role["job"] = default["job"].clone();
            }
        }
        let mut roles = Self::normalize(&saved)?;
        for role in defaults.into_iter().filter(|r| {
            r["id"] == "coordinator" || r["id"] == "delivery-reporter" || r["id"] == "navigator"
        }) {
            if !roles.iter().any(|r| r["id"] == role["id"]) {
                roles.push(role);
            }
        }
        Self::normalize(&roles)
    }
    fn normalize(roles: &[Value]) -> Result<Vec<Value>> {
        if !(3..=100).contains(&roles.len()) {
            return Err(Error::Invalid(
                "A role catalog must have 3–100 roles.".into(),
            ));
        }
        let mut seen = HashSet::new();
        let mut normalized = Vec::new();
        for value in roles {
            let mut role = validate_role(value)?;
            if let Some(role) = role.as_object_mut() {
                role.remove("projectSettings");
            }
            if !seen.insert(role["id"].clone().to_string()) {
                return Err(Error::Invalid("Role IDs must be unique.".into()));
            }
            if let Some(provenance) = value.get("learningProposalId") {
                let value = provenance
                    .as_str()
                    .filter(|s| valid_uuid(s))
                    .ok_or_else(|| Error::Invalid("learningProposalId must be a UUID.".into()))?;
                role["learningProposalId"] = json!(value);
            }
            if let Some(history) = value.get("history") {
                crate::role_history::validate(history, &role)?;
                role["history"] = history.clone();
            }
            normalized.push(role);
        }
        Ok(normalized)
    }
    pub fn create(&self, value: &Value) -> Result<Vec<Value>> {
        self.create_with_provenance(value, None)
    }
    pub fn create_from_draft(&self, value: &Value, proposal: &str) -> Result<Vec<Value>> {
        if !valid_uuid(proposal) {
            return Err(conflict("Invalid drafting provenance."));
        }
        self.create_with_provenance(value, Some(proposal))
    }
    fn create_with_provenance(&self, value: &Value, proposal: Option<&str>) -> Result<Vec<Value>> {
        let mut role = validate_role(value)?;
        role["revision"] = json!(1);
        let mut roles = self.read()?;
        if roles.iter().any(|r| r["id"] == role["id"]) {
            return Err(conflict("Role already exists."));
        }
        role["history"] = json!([crate::role_history::entry(
            &role,
            if proposal.is_some() {
                "runner-draft"
            } else {
                "created"
            },
            proposal
        )]);
        roles.push(role);
        self.save(roles)
    }
    pub fn update(&self, id: &str, value: &Value) -> Result<Vec<Value>> {
        self.update_with_provenance(id, value, None)
    }
    pub(crate) fn update_from_learning(
        &self,
        id: &str,
        value: &Value,
        proposal: &str,
    ) -> Result<Vec<Value>> {
        self.update_with_provenance(id, value, Some(proposal))
    }
    fn update_with_provenance(
        &self,
        id: &str,
        value: &Value,
        proposal: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut role = validate_role(value)?;
        let revision = integer(value, "revision", 1, u64::MAX)?;
        if role["id"] != id {
            return Err(conflict("The role ID cannot change."));
        }
        let mut roles = self.read()?;
        let previous = roles
            .iter_mut()
            .find(|r| r["id"] == id)
            .ok_or_else(|| conflict("Role not found."))?;
        let current_revision = previous["revision"].as_u64().unwrap_or(1);
        if current_revision != revision {
            return Err(conflict(&format!(
                "Role {id} changed since revision {revision}; current revision is {current_revision}. Reload the current definition before saving."
            )));
        }
        if ["coordinator", "triage", "implementer", "reviewer"].contains(&id)
            && role["enabled"] == false
        {
            return Err(conflict("The core delivery roles must stay enabled."));
        }
        role["revision"] = json!(
            revision
                .checked_add(1)
                .ok_or_else(|| conflict("Role revision exhausted."))?
        );
        if let Some(proposal) = proposal {
            role["learningProposalId"] = json!(proposal);
        }
        let mut history = crate::role_history::history(previous);
        history.push(crate::role_history::entry(
            &role,
            if proposal.is_some() {
                "learning"
            } else {
                "operator"
            },
            proposal,
        ));
        role["history"] = json!(history);
        *previous = role;
        self.save(roles)
    }
    fn save(&self, roles: Vec<Value>) -> Result<Vec<Value>> {
        let roles = Self::normalize(&roles)?;
        let principles = crate::principles::Principles::beside_roles(&self.path).read()?;
        for role in &roles {
            crate::principles::snapshot(&json!({}), role, &principles)?;
        }
        write_private(&self.path, &serde_json::to_vec_pretty(&roles)?)?;
        Ok(roles)
    }
}
pub fn validate_role(v: &Value) -> Result<Value> {
    let id = v["id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .ok_or_else(|| Error::Invalid("Invalid role ID.".into()))?;
    if !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(Error::Invalid("Invalid role ID.".into()));
    }
    let mut role = json!({"id":id});
    for (key, max) in [
        ("name", 100),
        ("category", 60),
        ("purpose", 1000),
        ("instructions", 12000),
        ("model", 1000),
    ] {
        let value = v[key]
            .as_str()
            .or_else(|| (key == "model" && v[key].is_null()).then_some(""))
            .map(trim_text)
            .filter(|s| (key == "model" || !s.is_empty()) && s.chars().count() <= max)
            .ok_or_else(|| Error::Invalid(format!("Invalid role {key}.")))?;
        role[key] = json!(value);
    }
    if let Some(ids) = v.get("principles") {
        let ids = ids
            .as_array()
            .filter(|a| a.len() <= 50)
            .ok_or_else(|| Error::Invalid("Principles must be a list of at most 50 IDs.".into()))?;
        let mut seen = HashSet::new();
        for id in ids {
            let id = id
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 64)
                .ok_or_else(|| Error::Invalid("Invalid principle reference.".into()))?;
            if !seen.insert(id) {
                return Err(Error::Invalid(
                    "Principle references must be distinct.".into(),
                ));
            }
        }
        role["principles"] = json!(ids);
    }
    // Missing fields are optional overrides; explicit saved choices survive migration.
    role["job"] = v.get("job").cloned().unwrap_or(Value::Null);
    if !role["job"].is_null()
        && (!role["job"].is_string() || role["job"].as_str().unwrap().len() > 60)
    {
        return Err(Error::Invalid("Invalid role job.".into()));
    }
    let effort = v["effort"]
        .as_str()
        .or_else(|| v["effort"].is_null().then_some("default"))
        .filter(|s| {
            [
                "", "default", "off", "minimal", "low", "medium", "high", "xhigh", "max",
            ]
            .contains(s)
        })
        .ok_or_else(|| Error::Invalid("Invalid reasoning effort.".into()))?;
    role["effort"] = json!(if effort.is_empty() { "default" } else { effort });
    role["enabled"] = json!(
        v["enabled"]
            .as_bool()
            .ok_or_else(|| Error::Invalid("enabled must be boolean.".into()))?
    );
    if v.get("revision").is_some() {
        role["revision"] = json!(integer(v, "revision", 1, u64::MAX)?);
    }
    // FOR-79: the project settings resolved at admission travel with the
    // assignment's role snapshot. The saved catalog never stores them.
    if let Some(settings) = v.get("projectSettings") {
        if !settings.is_object() || settings.to_string().chars().count() > 16000 {
            return Err(Error::Invalid("Invalid project settings on role.".into()));
        }
        role["projectSettings"] = settings.clone();
    }
    Ok(role)
}
