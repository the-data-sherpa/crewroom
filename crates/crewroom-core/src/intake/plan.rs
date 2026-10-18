//! Bounded implementation-plan schema. Package proposals confer no tool authority.
use crate::{
    Result,
    store::{invalid, strict, trimmed_text},
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn strings(value: &Value, min: usize, max: usize) -> Result<()> {
    let list = value
        .as_array()
        .filter(|v| (min..=100).contains(&v.len()))
        .ok_or_else(|| invalid("Expected a bounded text list."))?;
    if list.iter().any(|v| {
        v.as_str()
            .is_none_or(|s| s.trim().is_empty() || s.chars().count() > max)
    }) {
        return Err(invalid("Plan text is empty or exceeds its limit."));
    }
    Ok(())
}

pub fn validate(content: &Value, criteria: &[Value]) -> Result<()> {
    strict(
        content,
        &[
            "schemaVersion",
            "goal",
            "scope",
            "exclusions",
            "assumptions",
            "decisions",
            "workPackages",
        ],
    )?;
    if content["schemaVersion"] != 1 || content.to_string().len() > 100_000 {
        return Err(invalid("Use plan schema version 1 within 100 KB."));
    }
    trimmed_text(content, "goal", 1, 4000)?;
    for field in ["scope", "exclusions", "assumptions", "decisions"] {
        strings(&content[field], usize::from(field == "scope"), 4000)?;
    }
    let packages = content["workPackages"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 50)
        .ok_or_else(|| invalid("A plan needs 1–50 named work packages."))?;
    let mut graph = HashMap::new();
    let mut coverage = HashSet::new();
    for p in packages {
        strict(
            p,
            &[
                "id",
                "name",
                "dependsOn",
                "outputs",
                "criteria",
                "validation",
            ],
        )?;
        let id = trimmed_text(p, "id", 1, 64)?;
        if p["id"] != id
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(invalid(
                "Package IDs use letters, numbers, hyphens or underscores.",
            ));
        }
        trimmed_text(p, "name", 1, 200)?;
        strings(&p["outputs"], 1, 4000)?;
        strings(&p["validation"], 1, 4000)?;
        strings(&p["dependsOn"], 0, 64)?;
        let dependencies: Vec<_> = p["dependsOn"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        if dependencies.iter().collect::<HashSet<_>>().len() != dependencies.len()
            || graph.insert(id, dependencies).is_some()
        {
            return Err(invalid("Package and dependency IDs must be distinct."));
        }
        let refs = p["criteria"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 100)
            .ok_or_else(|| invalid("Every package needs criterion references."))?;
        let mut unique = HashSet::new();
        for c in refs {
            let index = c
                .as_u64()
                .filter(|i| (*i as usize) < criteria.len())
                .ok_or_else(|| invalid("Unknown criterion reference."))?;
            if !unique.insert(index) {
                return Err(invalid("Duplicate criterion reference."));
            }
            coverage.insert(index as usize);
        }
    }
    if criteria.is_empty() || coverage.len() != criteria.len() {
        return Err(invalid("The plan must cover every accepted criterion."));
    }
    let mut remaining = graph.clone();
    let mut done = HashSet::new();
    while !remaining.is_empty() {
        let ready: Vec<_> = remaining
            .iter()
            .filter(|(_, deps)| deps.iter().all(|d| done.contains(d)))
            .map(|(id, _)| *id)
            .collect();
        if ready.is_empty() {
            return Err(invalid(
                "Package dependencies contain a cycle or unknown ID.",
            ));
        }
        for id in ready {
            remaining.remove(id);
            done.insert(id);
        }
    }
    Ok(())
}
