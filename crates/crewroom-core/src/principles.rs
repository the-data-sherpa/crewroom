//! Reusable user principles; assignment briefs carry copies, never live references.
use crate::{Error, Result, authority::write_private, roles::Roles};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
pub struct Principles {
    path: PathBuf,
}
impl Principles {
    pub fn beside_roles(path: &Path) -> Self {
        Self {
            path: path.with_file_name("principles.json"),
        }
    }
    pub fn read(&self) -> Result<Vec<Value>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&self.path)?)?;
        if rows.len() > 200 {
            return Err(Error::Invalid(
                "At most 200 principles are supported.".into(),
            ));
        }
        let mut seen = HashSet::new();
        rows.iter()
            .map(|r| {
                let r = validate(r)?;
                if !seen.insert(r["id"].clone().to_string()) {
                    return Err(Error::Invalid("Principle IDs must be unique.".into()));
                }
                Ok(r)
            })
            .collect()
    }
    pub fn save(&self, input: &Value) -> Result<Value> {
        let mut next = validate(input)?;
        let mut rows = self.read()?;
        if let Some(current) = rows.iter_mut().find(|r| r["id"] == next["id"]) {
            if current["revision"] != next["revision"] {
                return Err(Error::Conflict(
                    "Principle changed. Reload before saving; your draft is retained.".into(),
                ));
            }
            next["revision"] = json!(
                next["revision"]
                    .as_u64()
                    .unwrap()
                    .checked_add(1)
                    .ok_or_else(|| Error::Invalid("Principle revision exhausted.".into()))?
            );
            *current = next.clone();
        } else {
            if next["revision"] != 0 {
                return Err(Error::Conflict(
                    "Principle no longer exists. Create a new principle.".into(),
                ));
            }
            if rows.len() >= 200 {
                return Err(Error::Invalid(
                    "At most 200 principles are supported.".into(),
                ));
            }
            next["revision"] = json!(1);
            rows.push(next.clone());
        }
        write_private(&self.path, &serde_json::to_vec_pretty(&rows)?)?;
        Ok(next)
    }
    /// `settings` is `Store::project_settings()`: a principle that the
    /// workspace or a project applies to every role cannot be deleted
    /// either, or every later admission there would fail.
    pub fn delete(
        &self,
        id: &str,
        revision: u64,
        roles: &Roles,
        settings: &Value,
    ) -> Result<Value> {
        let mut rows = self.read()?;
        let current = rows
            .iter()
            .find(|r| r["id"] == id)
            .ok_or_else(|| Error::NotFound("Principle not found.".into()))?;
        if current["revision"] != revision {
            return Err(Error::Conflict(
                "Principle changed. Reload before deleting.".into(),
            ));
        }
        let used: Vec<_> = roles
            .read()?
            .into_iter()
            .filter(|r| {
                r["principles"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|v| v == id))
            })
            .map(|r| r["name"].as_str().unwrap_or("").to_owned())
            .collect();
        if !used.is_empty() {
            return Err(Error::Conflict(format!(
                "Detach this principle from these roles before deleting: {}.",
                used.join(", ")
            )));
        }
        let lists = |v: &Value| {
            v["principles"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|v| v == id))
        };
        let mut scopes = Vec::new();
        if lists(&settings["workspace"]) {
            scopes.push("workspace settings".to_owned());
        }
        for (repo, values) in settings["projects"].as_object().into_iter().flatten() {
            if lists(values) {
                scopes.push(format!("project {repo}"));
            }
        }
        if !scopes.is_empty() {
            return Err(Error::Conflict(format!(
                "Remove this principle from these settings before deleting: {}.",
                scopes.join(", ")
            )));
        }
        rows.retain(|r| r["id"] != id);
        write_private(&self.path, &serde_json::to_vec_pretty(&rows)?)?;
        Ok(json!({"deleted":id}))
    }
}
fn validate(input: &Value) -> Result<Value> {
    let mut output = json!({});
    for (key, max) in [("id", 64), ("name", 100), ("text", 12000)] {
        let value = input[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.chars().count() <= max)
            .ok_or_else(|| Error::Invalid(format!("Invalid principle {key}.")))?;
        if key == "id"
            && (!value.as_bytes()[0].is_ascii_lowercase()
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'))
        {
            return Err(Error::Invalid("Invalid principle ID.".into()));
        }
        output[key] = json!(value);
    }
    output["revision"] = json!(
        input["revision"]
            .as_u64()
            .ok_or_else(|| Error::Invalid("Principle revision is required.".into()))?
    );
    Ok(output)
}
/// Shared by initial admission, specialist insertion, and the assembled preview.
pub fn snapshot(brief: &Value, role: &Value, catalog: &[Value]) -> Result<Value> {
    if !brief.is_object() {
        return Err(Error::Invalid("A brief must be an object.".into()));
    }
    let mut brief = brief.clone();
    let mut copies = vec![];
    for id in role["principles"].as_array().into_iter().flatten() {
        let principle = catalog.iter().find(|p| p["id"] == *id).ok_or_else(|| {
            Error::Conflict(format!(
                "Referenced principle {} is unavailable.",
                id.as_str().unwrap_or("")
            ))
        })?;
        copies.push(principle.clone());
    }
    if !copies.is_empty() {
        brief["principles"] = json!(copies);
    } else if let Some(object) = brief.as_object_mut() {
        object.remove("principles");
    }
    Ok(brief)
}
