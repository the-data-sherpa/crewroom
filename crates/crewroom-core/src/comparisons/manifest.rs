//! Validation of the immutable experiment, before any task or process exists.
use crate::{
    Result,
    roles::validate_role,
    store::{conflict, integer, invalid, text},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Stable JSON identity independent of object insertion order.
pub fn hash(value: &Value) -> String {
    fn ordered(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for key in keys {
                    out.insert(key.clone(), ordered(&map[key]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(ordered).collect()),
            value => value.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(crate::serialized::stringify(&ordered(value)).as_bytes())
    )
}

fn object(value: &Value, field: &str) -> Result<()> {
    if !value[field].is_object() || value[field].as_object().is_some_and(|v| v.is_empty()) {
        return Err(invalid(&format!(
            "Comparison {field} must be a nonempty snapshot."
        )));
    }
    Ok(())
}
fn digest(value: &Value, field: &str) -> Result<()> {
    let value = text(value, field, 64, 64)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(invalid(&format!(
            "{field} must be a lowercase SHA-256 digest."
        )));
    }
    Ok(())
}

pub fn validate(manifest: &Value) -> Result<()> {
    if manifest["version"] != 1 || !matches!(manifest["mode"].as_str(), Some("live" | "scripted")) {
        return Err(invalid("Unsupported comparison version or mode."));
    }
    let target = &manifest["target"];
    let kind = text(target, "kind", 1, 30)?;
    if !matches!(kind, "role" | "principle" | "playbook") {
        return Err(invalid("Unsupported instruction kind."));
    }
    text(target, "id", 1, 100)?;
    text(target, "draftId", 1, 200)?;
    integer(target, "draftVersion", 1, u64::MAX)?;
    integer(target, "baseRevision", 1, u64::MAX)?;
    object(target, "current")?;
    object(target, "proposed")?;
    for field in ["baseHash", "proposedHash"] {
        digest(target, field)?;
    }
    if hash(&target["current"]) != target["baseHash"]
        || hash(&target["proposed"]) != target["proposedHash"]
    {
        return Err(conflict(
            "Instruction snapshot hash does not match its payload.",
        ));
    }
    if target["current"] == target["proposed"] {
        return Err(invalid("Comparison needs a changed instruction revision."));
    }
    if target["current"]["id"] != target["id"]
        || target["proposed"]["id"] != target["id"]
        || target["current"]["revision"] != target["baseRevision"]
        || target["proposed"]["revision"] != target["baseRevision"]
    {
        return Err(conflict(
            "Target identity or base revision is inconsistent.",
        ));
    }
    let consumer = text(target, "consumer", 1, 30)?;
    if !matches!(consumer, "coordinator" | "specialist")
        || kind == "playbook" && consumer != "coordinator"
    {
        return Err(invalid("Unsupported target consumer."));
    }
    let mut substituted = target["current"].clone();
    match kind {
        "role" => substituted["instructions"] = target["proposed"]["instructions"].clone(),
        "principle" => substituted["text"] = target["proposed"]["text"].clone(),
        "playbook" => {
            substituted["description"] = target["proposed"]["description"].clone();
            substituted["steps"] = target["proposed"]["steps"].clone();
        }
        _ => unreachable!(),
    }
    if substituted != target["proposed"] {
        return Err(conflict(
            "A comparison changes instruction content only; all execution policy and target metadata must match.",
        ));
    }
    let execution = &manifest["execution"];
    for field in [
        "model",
        "coordinatorModel",
        "runner",
        "tools",
        "application",
        "platform",
    ] {
        object(execution, field)?;
    }
    if execution["capabilities"]
        .as_array()
        .is_none_or(Vec::is_empty)
    {
        return Err(invalid("Comparison capabilities must be pinned."));
    }
    text(execution, "credentialReference", 1, 500)?;
    let design = &manifest["design"];
    if design["repetitions"] != 2 || design["variants"] != json!(["current", "proposed"]) {
        return Err(invalid(
            "The version-one pack requires two repetitions and both instruction variants.",
        ));
    }
    object(design, "scorer")?;
    object(design, "coordinator")?;
    if design["variantMapping"] != json!({"A":"current","B":"proposed"}) {
        return Err(invalid(
            "The private version-one variant mapping must be pinned.",
        ));
    }
    let cell_limit = integer(design, "perCellActiveMs", 1000, 180_000)?;
    let total_limit = integer(design, "totalActiveMs", 1000, 2_880_000)?;
    if total_limit < cell_limit {
        return Err(invalid(
            "Comparison budget cannot be smaller than one cell allowance.",
        ));
    }
    let cases = manifest["cases"]
        .as_array()
        .filter(|v| v.len() == 4)
        .ok_or_else(|| invalid("The version-one comparison requires four frozen cases."))?;
    let mut seen = HashSet::new();
    for case in cases {
        let id = text(case, "id", 1, 100)?;
        if !seen.insert(id) {
            return Err(invalid("Case identities must be unique."));
        }
        integer(case, "version", 1, u64::MAX)?;
        text(case, "prompt", 1, 40_000)?;
        digest(case, "sourceHash")?;
        object(case, "repository")?;
        object(case, "requirements")?;
        validate_inputs(case, target, execution)?;
    }
    Ok(())
}

fn validate_inputs(case: &Value, target: &Value, execution: &Value) -> Result<()> {
    let inputs = &case["inputs"];
    for variant in ["current", "proposed"] {
        let team = inputs[variant]
            .as_array()
            .filter(|v| v.len() == 2)
            .ok_or_else(|| {
                invalid("Each case variant needs exactly one coordinator and one specialist.")
            })?;
        if team[0]["coordinator"] != true
            || team[1]["coordinator"] != false
            || team[0]["name"] == team[1]["name"]
        {
            return Err(invalid(
                "Frozen team order is coordinator, specialist, with distinct names.",
            ));
        }
        for actor in team {
            text(actor, "name", 1, 100)?;
            text(actor, "input", 1, 100_000)?;
            validate_role(&actor["role"])?;
            object(actor, "brief")?;
            if actor["model"]
                != execution[if actor["coordinator"] == true {
                    "coordinatorModel"
                } else {
                    "model"
                }]
            {
                return Err(conflict(
                    "Frozen actor model differs from the comparison execution identity.",
                ));
            }
        }
    }
    let consumer = usize::from(target["consumer"] != "coordinator");
    let mut expected = inputs["current"].clone();
    let current = &inputs["current"][consumer];
    match target["kind"].as_str().unwrap() {
        "role" => {
            if current["role"] != target["current"] {
                return Err(conflict(
                    "The selected consumer does not receive the current target role.",
                ));
            }
            expected[consumer]["role"] = target["proposed"].clone();
        }
        "principle" => {
            let principles = current["brief"]["principles"]
                .as_array()
                .ok_or_else(|| conflict("The selected consumer has no captured principles."))?;
            let matching: Vec<_> = principles
                .iter()
                .enumerate()
                .filter(|(_, p)| p["id"] == target["id"])
                .collect();
            if matching.len() != 1 || matching[0].1 != &target["current"] {
                return Err(conflict(
                    "The target principle must be captured exactly once by the selected consumer.",
                ));
            }
            expected[consumer]["brief"]["principles"][matching[0].0] = target["proposed"].clone();
        }
        "playbook" => {
            if current["brief"]["playbook"] != target["current"] {
                return Err(conflict(
                    "The coordinator must receive the pinned current playbook.",
                ));
            }
            expected[consumer]["brief"]["playbook"] = target["proposed"].clone();
        }
        _ => unreachable!(),
    }
    if expected != inputs["proposed"] {
        return Err(conflict(
            "Only the target instruction may differ between composed variant inputs.",
        ));
    }
    Ok(())
}
