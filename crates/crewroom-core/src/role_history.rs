//! History and current definition share one atomic catalog write. API input cannot rewrite history.
use crate::{Result, roles::validate_role, store::invalid};
use serde_json::{Value, json};
pub fn entry(role: &Value, origin: &str, proposal: Option<&str>) -> Value {
    let mut value = json!({"revision":role["revision"].as_u64().unwrap_or(1),"definition":validate_role(role).expect("validated role"),"origin":origin});
    if origin != "imported" {
        value["at"] = json!(chrono::Utc::now().to_rfc3339());
    }
    if let Some(id) = proposal {
        value["proposalId"] = json!(id);
    }
    value
}
pub fn history(role: &Value) -> Vec<Value> {
    role["history"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![entry(role, "imported", None)])
}
pub fn validate(value: &Value, role: &Value) -> Result<()> {
    let rows = value
        .as_array()
        .filter(|r| !r.is_empty())
        .ok_or_else(|| invalid("Invalid role history."))?;
    let mut previous = None;
    for row in rows {
        let revision = row["revision"]
            .as_u64()
            .filter(|r| *r > 0)
            .ok_or_else(|| invalid("Invalid history revision."))?;
        if previous.is_some_and(|p: u64| p.checked_add(1) != Some(revision))
            || row["definition"]["id"] != role["id"]
            || row["definition"]["revision"].as_u64().unwrap_or(1) != revision
        {
            return Err(invalid("Role history is inconsistent."));
        }
        validate_role(&row["definition"])?;
        previous = Some(revision);
    }
    if previous != Some(role["revision"].as_u64().unwrap_or(1)) {
        return Err(invalid("Role history does not cover its current revision."));
    }
    Ok(())
}
pub fn diff(previous: Option<&Value>, next: &Value) -> Vec<Value> {
    [
        "name",
        "category",
        "purpose",
        "instructions",
        "job",
        "principles",
        "model",
        "effort",
        "enabled",
    ]
    .iter()
    .filter_map(|k| {
        let before = previous.map(|v| v[*k].clone()).unwrap_or(Value::Null);
        (before != next[*k]).then(|| json!({"field":k,"before":before,"after":next[*k]}))
    })
    .collect()
}
