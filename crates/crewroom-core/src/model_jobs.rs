//! Public model selections only. Provider credentials remain in private runtime configuration.
use crate::{Error, Result, authority::write_private};
use serde_json::{Value, json};
use std::path::Path;

pub const JOBS: [&str; 4] = ["code", "judgment", "review", "fast"];
pub const BUDGETS: [&str; 4] = ["unlimited", "large", "medium", "small"];
pub fn job(role: &Value) -> (&str, Option<&'static str>) {
    match role["job"].as_str() {
        Some(job) if JOBS.contains(&job) => (job, None),
        _ => (
            "judgment",
            Some("Missing or unknown job; using Judgment. Choose a job to clear this warning."),
        ),
    }
}
pub fn effort(budget: &str, job: &str) -> Result<&'static str> {
    let row = match budget {
        "unlimited" => ["xhigh", "max", "max", "medium"],
        "large" => ["high", "xhigh", "xhigh", "low"],
        "medium" => ["medium", "high", "high", "minimal"],
        "small" => ["low", "medium", "medium", "minimal"],
        _ => return Err(Error::Invalid("Unknown model budget.".into())),
    };
    Ok(row[JOBS.iter().position(|j| *j == job).unwrap_or(1)])
}
pub fn defaults(policy: &Value) -> Value {
    let mut jobs = json!({});
    for job in JOBS {
        jobs[job] = json!(
            policy["jobs"][job]
                .as_str()
                .or_else(|| policy["default"]["model"].as_str())
                .unwrap_or("")
        );
    }
    json!({"revision":0,"jobs":jobs,"budget":policy["budget"].as_str().unwrap_or("large"),"legacyDefault":!policy["jobs"].is_object() && policy["default"]["model"].as_str().is_some_and(|m| !m.is_empty())})
}
pub fn read(path: &Path, policy: &Value) -> Result<Value> {
    if !path.exists() {
        return Ok(defaults(policy));
    }
    validate(&serde_json::from_slice(&std::fs::read(path)?)?)
}
fn validate(value: &Value) -> Result<Value> {
    let budget = value["budget"]
        .as_str()
        .filter(|b| BUDGETS.contains(b))
        .ok_or_else(|| Error::Invalid("Unknown model budget.".into()))?;
    let revision = value["revision"]
        .as_u64()
        .ok_or_else(|| Error::Invalid("Model configuration revision is required.".into()))?;
    let mut jobs = json!({});
    for job in JOBS {
        let model = value["jobs"][job]
            .as_str()
            .map(str::trim)
            .filter(|s| s.len() <= 1000)
            .ok_or_else(|| Error::Invalid(format!("Invalid {job} model selector.")))?;
        jobs[job] = json!(model);
    }
    Ok(json!({"revision":revision,"budget":budget,"jobs":jobs,"legacyDefault":false}))
}
pub fn save(path: &Path, policy: &Value, input: &Value) -> Result<Value> {
    let current = read(path, policy)?;
    let mut value = validate(input)?;
    if current["revision"] != value["revision"] {
        return Err(Error::Conflict(
            "Model jobs changed. Reload before saving; your draft is retained.".into(),
        ));
    }
    value["revision"] = json!(
        current["revision"]
            .as_u64()
            .unwrap()
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("Model revision exhausted.".into()))?
    );
    write_private(path, &serde_json::to_vec_pretty(&value)?)?;
    Ok(value)
}
