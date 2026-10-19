//! FOR-79: per-project setting overrides and where each value comes from.
//!
//! A setting resolves as project override, then workspace, then built-in
//! default. Overridable settings are the job models and budget, which roles
//! may be assigned, the operator's project instructions and principles, and
//! run capacity. Workspace values for models and roles stay in their own
//! user configuration (`model-jobs.json`, the roles file); the workspace
//! instructions, principles and default capacity, and every project
//! override, are stored here in the `execution_settings` record, beside the
//! FOR-76 capacity values. A resolved role carries what applied in
//! `projectSettings`, which the assignment captures with the role and the
//! prompt renders. Saving a workspace value never changes a project
//! override, and an assignment captures the resolved values at admission, so
//! a later override never alters an existing run.
use crate::{
    Error, Result,
    capacity::MAX_CAPACITY,
    model_jobs::{BUDGETS, JOBS},
    store::{Store, invalid, strict},
};
use serde_json::{Map, Value, json};

pub const DEFAULT: &str = "default";
pub const WORKSPACE: &str = "workspace";
pub const PROJECT: &str = "project";

/// Keys a workspace may set here. Models and roles keep their own editors.
pub const WORKSPACE_KEYS: [&str; 3] = ["instructions", "principles", "capacity"];
const MAX_INSTRUCTIONS: usize = 12000;
const MAX_PRINCIPLES: usize = 50;

/// Every key a project may override, given the configured roles.
pub fn project_keys(roles: &[Value]) -> Vec<String> {
    let mut keys = vec!["models.budget".to_owned()];
    keys.extend(JOBS.iter().map(|job| format!("models.jobs.{job}")));
    keys.extend(
        roles
            .iter()
            .filter_map(|r| r["id"].as_str())
            .map(|id| format!("roles.{id}")),
    );
    keys.extend(["instructions", "principles", "capacity"].map(String::from));
    keys
}

/// Where a value comes from, with the full chain from the built-in default.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub value: Value,
    pub source: &'static str,
    /// `(source, value)` for each layer, lowest first; `None` when the layer
    /// does not set it.
    pub chain: Vec<(&'static str, Option<Value>)>,
}
impl Resolved {
    pub fn to_json(&self) -> Value {
        json!({"value":self.value,"source":self.source,"chain":self.chain.iter().map(|(s,v)|json!({"source":s,"value":v})).collect::<Vec<_>>()})
    }
}

/// Built-in default for a key.
pub fn default_value(key: &str) -> Value {
    match key {
        "models.budget" => json!("large"),
        "instructions" => json!(""),
        "principles" => json!([]),
        "capacity" => json!(1),
        k if k.starts_with("roles.") => json!(true),
        _ => json!(""),
    }
}

/// The workspace layer. `model_jobs` is the workspace model configuration
/// (`revision` 0 means never saved, so the default applies); `roles` is the
/// roles catalog, whose `enabled` flag is the workspace value.
pub fn workspace_value(
    key: &str,
    settings: &Value,
    model_jobs: &Value,
    roles: &[Value],
) -> Option<Value> {
    let saved_models = model_jobs["revision"].as_u64().is_some_and(|r| r > 0)
        || model_jobs["legacyDefault"] == true;
    if key == "models.budget" {
        return saved_models
            .then(|| model_jobs["budget"].clone())
            .filter(Value::is_string);
    }
    if let Some(job) = key.strip_prefix("models.jobs.") {
        return saved_models
            .then(|| model_jobs["jobs"][job].clone())
            .filter(Value::is_string);
    }
    if let Some(id) = key.strip_prefix("roles.") {
        return roles
            .iter()
            .find(|r| r["id"] == id)
            .map(|r| json!(r["enabled"] == true));
    }
    settings["workspace"]
        .get(key)
        .filter(|v| !v.is_null())
        .cloned()
}

pub fn project_value(key: &str, settings: &Value, project: &str) -> Option<Value> {
    if project.is_empty() {
        return None;
    }
    settings["projects"][project]
        .get(key)
        .filter(|v| !v.is_null())
        .cloned()
}

/// Project override, then workspace, then built-in default.
pub fn resolve(
    key: &str,
    settings: &Value,
    project: Option<&str>,
    model_jobs: &Value,
    roles: &[Value],
) -> Resolved {
    let default = default_value(key);
    let workspace = workspace_value(key, settings, model_jobs, roles);
    let mut chain = vec![
        (DEFAULT, Some(default.clone())),
        (WORKSPACE, workspace.clone()),
    ];
    let overridden = project.and_then(|p| project_value(key, settings, p));
    if project.is_some() {
        chain.push((PROJECT, overridden.clone()));
    }
    let (value, source) = match (overridden, workspace) {
        (Some(v), _) => (v, PROJECT),
        (None, Some(v)) => (v, WORKSPACE),
        (None, None) => (default, DEFAULT),
    };
    Resolved {
        value,
        source,
        chain,
    }
}

/// Projects that override `key`, in their stored order.
pub fn overriding_projects(key: &str, settings: &Value) -> Vec<String> {
    settings["projects"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| v.get(key).is_some_and(|v| !v.is_null()))
        .map(|(k, _)| k.clone())
        .collect()
}

/// Roles as admitted in `project`: each role's `enabled` flag, principles
/// and instructions resolved through the project's overrides, with a
/// `projectSettings` record of the overrides that apply (captured with the
/// assignment). Model overrides are read back by the runtime's policy.
pub fn effective_roles(roles: &[Value], settings: &Value, project: &str) -> Vec<Value> {
    let none = json!({});
    let scope = (!project.is_empty()).then_some(project);
    let instructions = resolve("instructions", settings, scope, &none, roles);
    let principles = resolve("principles", settings, scope, &none, roles);
    let mut models = json!({});
    if let Some(budget) = project_value("models.budget", settings, project) {
        models["budget"] = budget;
    }
    for job in JOBS {
        if let Some(model) = project_value(&format!("models.jobs.{job}"), settings, project) {
            models["jobs"][job] = model;
        }
    }
    roles
        .iter()
        .map(|role| {
            // Already resolved for a project: applying again would repeat
            // the instructions.
            if role.get("projectSettings").is_some() {
                return role.clone();
            }
            let mut role = role.clone();
            let id = role["id"].as_str().unwrap_or("").to_owned();
            let mut applied = json!({"repositoryId": project});
            if let Some(enabled) = project_value(&format!("roles.{id}"), settings, project) {
                role["enabled"] = enabled.clone();
                applied["enabled"] = enabled;
            }
            let extra: Vec<Value> = principles
                .value
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| {
                    !role["principles"]
                        .as_array()
                        .is_some_and(|ids| ids.contains(p))
                })
                .cloned()
                .collect();
            if !extra.is_empty() {
                let mut ids = role["principles"].as_array().cloned().unwrap_or_default();
                ids.extend(extra.iter().cloned());
                role["principles"] = json!(ids);
                applied["principles"] = json!({"ids":extra,"source":principles.source});
            }
            if let Some(text) = instructions.value.as_str().filter(|t| !t.trim().is_empty()) {
                applied["instructions"] = json!({"text":text,"source":instructions.source});
            }
            if models.as_object().is_some_and(|m| !m.is_empty()) {
                applied["models"] = models.clone();
            }
            if applied.as_object().is_some_and(|m| m.len() > 1) {
                role["projectSettings"] = applied;
            }
            role
        })
        .collect()
}

fn validate(key: &str, value: &Value, roles: &[Value], principles: &[Value]) -> Result<Value> {
    if key == "models.budget" {
        return value
            .as_str()
            .filter(|b| BUDGETS.contains(b))
            .map(|b| json!(b))
            .ok_or_else(|| invalid("Unknown model budget."));
    }
    if let Some(job) = key.strip_prefix("models.jobs.") {
        if !JOBS.contains(&job) {
            return Err(invalid(&format!("Unknown job {job}.")));
        }
        return value
            .as_str()
            .map(str::trim)
            .filter(|m| !m.is_empty() && m.len() <= 1000)
            .map(|m| json!(m))
            .ok_or_else(|| invalid(&format!("Choose a model for the {job} job.")));
    }
    if let Some(id) = key.strip_prefix("roles.") {
        if !roles.iter().any(|r| r["id"] == id) {
            return Err(Error::NotFound(format!("Role {id} not found.")));
        }
        return value
            .as_bool()
            .map(|b| json!(b))
            .ok_or_else(|| invalid("Role availability must be true or false."));
    }
    match key {
        "instructions" => value
            .as_str()
            .map(str::trim)
            .filter(|t| t.chars().count() <= MAX_INSTRUCTIONS)
            .map(|t| json!(t))
            .ok_or_else(|| invalid("Instructions must be text of at most 12000 characters.")),
        "principles" => {
            let ids = value
                .as_array()
                .filter(|a| a.len() <= MAX_PRINCIPLES)
                .ok_or_else(|| invalid("Principles must be a list of at most 50 IDs."))?;
            let mut out: Vec<Value> = vec![];
            for id in ids {
                if !principles.iter().any(|p| p["id"] == *id) {
                    return Err(Error::NotFound(format!(
                        "Principle {} not found.",
                        id.as_str().unwrap_or("")
                    )));
                }
                if !out.contains(id) {
                    out.push(id.clone());
                }
            }
            Ok(json!(out))
        }
        "capacity" => {
            crate::store::integer(&json!({"capacity":value}), "capacity", 1, MAX_CAPACITY)
                .map(|n| json!(n))
        }
        _ => Err(invalid(&format!("Unknown setting {key}."))),
    }
}

impl Store {
    /// `{"workspace": {key: value}, "projects": {repositoryId: {key: value}}}`.
    /// Capacity is read from the FOR-76 values in the same record.
    pub fn project_settings(&self) -> Result<Value> {
        let stored = self.execution_settings()?;
        let mut workspace = stored["workspaceSettings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if let Some(capacity) = stored["defaultCapacity"].as_u64() {
            workspace.insert("capacity".into(), json!(capacity));
        }
        let mut projects: Map<String, Value> = stored["projectSettings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (repo, capacity) in stored["projects"].as_object().into_iter().flatten() {
            let entry = projects.entry(repo.clone()).or_insert_with(|| json!({}));
            entry["capacity"] = capacity.clone();
        }
        projects.retain(|_, v| v.as_object().is_some_and(|m| !m.is_empty()));
        Ok(json!({"workspace":workspace,"projects":projects}))
    }

    /// Sets or resets settings for one project (`repositoryId`) or, with
    /// `repositoryId` null, the workspace. `set` maps keys to values;
    /// `reset` lists keys that inherit again. A workspace change never
    /// touches a project override.
    pub fn save_project_settings(
        &self,
        body: &Value,
        roles: &[Value],
        principles: &[Value],
    ) -> Result<Value> {
        strict(body, &["repositoryId", "set", "reset"])?;
        let set = match body.get("set") {
            None => Map::new(),
            Some(v) => v
                .as_object()
                .cloned()
                .ok_or_else(|| invalid("set must be an object."))?,
        };
        let reset: Vec<String> = match body.get("reset") {
            None => vec![],
            Some(v) => v
                .as_array()
                .ok_or_else(|| invalid("reset must be a list of setting keys."))?
                .iter()
                .map(|k| k.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| invalid("reset must be a list of setting keys."))?,
        };
        if set.is_empty() && reset.is_empty() {
            return Err(invalid("Nothing to change."));
        }
        if reset.iter().any(|k| set.contains_key(k)) {
            return Err(invalid("A setting cannot be set and reset together."));
        }
        let project = match body.get("repositoryId") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) => Some(id.clone()),
            Some(_) => return Err(invalid("repositoryId must be a string or null.")),
        };
        self.transaction(|s| {
            let allowed: Vec<String> = match &project {
                Some(repo) => {
                    if s.record("repositories", "id", repo)?
                        .is_none_or(|r| r.get("retiredAt").is_some())
                    {
                        return Err(Error::NotFound("Project not found.".into()));
                    }
                    project_keys(roles)
                }
                None => WORKSPACE_KEYS.map(String::from).to_vec(),
            };
            let scope = if project.is_some() {
                "a project"
            } else {
                "the workspace here"
            };
            for key in set.keys().chain(reset.iter()) {
                if !allowed.contains(key) && !(key.starts_with("roles.") && project.is_some()) {
                    return Err(invalid(&format!("{key} cannot be set for {scope}.")));
                }
            }
            let mut stored = s.execution_settings()?;
            for (key, value) in &set {
                let value = validate(key, value, roles, principles)?;
                match (&project, key.as_str()) {
                    (Some(repo), "capacity") => stored["projects"][repo] = value,
                    (None, "capacity") => stored["defaultCapacity"] = value,
                    (Some(repo), _) => {
                        if !stored["projectSettings"].is_object() {
                            stored["projectSettings"] = json!({});
                        }
                        if !stored["projectSettings"][repo].is_object() {
                            stored["projectSettings"][repo] = json!({});
                        }
                        stored["projectSettings"][repo][key] = value;
                    }
                    (None, _) => {
                        if !stored["workspaceSettings"].is_object() {
                            stored["workspaceSettings"] = json!({});
                        }
                        stored["workspaceSettings"][key] = value;
                    }
                }
            }
            for key in &reset {
                let remove = |v: &mut Value, k: &str| {
                    if let Some(m) = v.as_object_mut() {
                        m.remove(k);
                    }
                };
                match (&project, key.as_str()) {
                    (Some(repo), "capacity") => remove(&mut stored["projects"], repo),
                    (None, "capacity") => remove(&mut stored, "defaultCapacity"),
                    (Some(repo), _) => {
                        if stored["projectSettings"][repo].is_object() {
                            remove(&mut stored["projectSettings"][repo], key);
                            if stored["projectSettings"][repo]
                                .as_object()
                                .is_some_and(|m| m.is_empty())
                            {
                                remove(&mut stored["projectSettings"], repo);
                            }
                        }
                    }
                    (None, _) => {
                        if stored["workspaceSettings"].is_object() {
                            remove(&mut stored["workspaceSettings"], key);
                        }
                    }
                }
            }
            s.write_execution_settings(&stored)?;
            s.project_settings()
        })
    }
}
