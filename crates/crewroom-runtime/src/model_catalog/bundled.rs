//! FOR-73 bundled model facts: a reviewed file compiled into Crewroom and
//! changed only by a Crewroom revision. It is never fetched. Live discovery
//! decides which models are available; these facts fill in the capabilities
//! discovery leaves unreported, and name the harness release that lists a
//! model the installed version does not.
use super::*;
use std::sync::OnceLock;

const FILE: &str = include_str!("bundled-models.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Current,
    Legacy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    pub harness: Harness,
    pub provider: String,
    pub selector: String,
    pub display_name: String,
    pub status: Status,
    pub efforts: Vec<String>,
    pub tools: bool,
    pub inputs: Vec<String>,
    /// Oldest harness release Crewroom reviewed that lists this model, as a
    /// bare `major.minor.patch`.
    pub minimum_harness_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema_version: u32,
    #[allow(dead_code)]
    reviewed_at: String,
    #[allow(dead_code)]
    about: String,
    #[allow(dead_code)]
    sources: Value,
    models: Vec<Model>,
}

fn release(value: &str) -> Option<[u64; 3]> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut result = [0; 3];
    for (slot, part) in result.iter_mut().zip(parts) {
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(result)
}

fn parse(text: &str) -> std::result::Result<Vec<Model>, String> {
    let document: Document =
        serde_json::from_str(text).map_err(|e| format!("Bundled model file is invalid: {e}"))?;
    if document.schema_version != 1 {
        return Err("Bundled model file has an unsupported schema.".into());
    }
    let mut seen = HashSet::new();
    for m in &document.models {
        let lists_valid = [&m.efforts, &m.inputs].into_iter().all(|v| {
            let mut unique = HashSet::new();
            !v.is_empty() && v.iter().all(|s| valid_text(s) && unique.insert(s))
        });
        if ![&m.provider, &m.selector, &m.display_name]
            .into_iter()
            .all(|s| valid_text(s))
            || !lists_valid
            || release(&m.minimum_harness_version).is_none()
            || !seen.insert(offering_key(&m.harness, &m.provider, &m.selector))
        {
            return Err(format!(
                "Bundled model file has an invalid or duplicate entry: {} {}/{}.",
                m.harness.as_str(),
                m.provider,
                m.selector
            ));
        }
    }
    Ok(document.models)
}

/// The reviewed facts. A build whose file does not parse is caught by tests;
/// at run time an invalid file contributes no facts instead of failing.
pub fn models() -> &'static [Model] {
    static MODELS: OnceLock<Vec<Model>> = OnceLock::new();
    MODELS.get_or_init(|| parse(FILE).unwrap_or_default())
}

pub fn find<'a>(
    models: &'a [Model],
    harness: &Harness,
    provider: &str,
    selector: &str,
) -> Option<&'a Model> {
    models
        .iter()
        .find(|m| &m.harness == harness && m.provider == provider && m.selector == selector)
}

/// A discovered offering with bundled facts filling capabilities discovery
/// left unreported. Reported facts are never replaced.
pub fn merge_with(models: &[Model], discovered: &Offering) -> Offering {
    let mut merged = discovered.clone();
    if let Some(m) = find(
        models,
        &discovered.harness,
        &discovered.provider,
        &discovered.selector,
    ) {
        let caps = &mut merged.capabilities;
        caps.efforts.get_or_insert_with(|| m.efforts.clone());
        caps.inputs.get_or_insert_with(|| m.inputs.clone());
        caps.tools.get_or_insert(m.tools);
    }
    merged
}
pub fn merge(discovered: &Offering) -> Offering {
    merge_with(models(), discovered)
}

/// Picker facts for one offering: status, where its facts came from, and the
/// capabilities still unknown after merging.
pub fn facts_with(models: &[Model], offering: &Offering) -> Value {
    let bundled = find(
        models,
        &offering.harness,
        &offering.provider,
        &offering.selector,
    );
    let caps = &offering.capabilities;
    let unknown: Vec<&str> = [
        ("efforts", caps.efforts.is_none()),
        ("inputs", caps.inputs.is_none()),
        ("tools", caps.tools.is_none()),
    ]
    .into_iter()
    .filter_map(|(name, missing)| missing.then_some(name))
    .collect();
    json!({
        "status": bundled.map(|m| m.status),
        "facts": if bundled.is_some() { "bundled" } else { "live-only" },
        "unknownCapabilities": unknown,
    })
}
pub fn facts(offering: &Offering) -> Value {
    facts_with(models(), offering)
}

/// Bundled models the installed harness version does not list because it
/// predates them. Information only: Crewroom never updates a harness.
pub fn needs_update_with(
    models: &[Model],
    harness: &Harness,
    installed: &str,
    discovered: &[Offering],
) -> Vec<Value> {
    // OMP lists models by configured provider, not by release, so a model it
    // omits is never missing only for version reasons.
    if harness == &Harness::Omp {
        return vec![];
    }
    let Some(current) = compatibility::parse(harness, installed) else {
        return vec![];
    };
    models
        .iter()
        .filter(|m| &m.harness == harness)
        .filter(|m| {
            !discovered
                .iter()
                .any(|o| o.provider == m.provider && o.selector == m.selector)
        })
        .filter(|m| release(&m.minimum_harness_version).is_some_and(|min| min > current))
        .map(|m| {
            let label = harness.label();
            json!({
                "harness": harness,
                "provider": m.provider,
                "selector": m.selector,
                "displayName": m.display_name,
                "status": m.status,
                "efforts": m.efforts,
                "minimumHarnessVersion": m.minimum_harness_version,
                "installedVersion": installed,
                "reason": format!(
                    "Listed by {label} {} or newer; this service runs {installed}. Update {label} yourself, then refresh models. Crewroom never updates harnesses.",
                    m.minimum_harness_version
                ),
            })
        })
        .collect()
}
pub fn needs_update(harness: &Harness, installed: &str, discovered: &[Offering]) -> Vec<Value> {
    needs_update_with(models(), harness, installed, discovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offering(harness: Harness, provider: &str, selector: &str) -> Offering {
        Offering {
            key: offering_key(&harness, provider, selector),
            harness,
            provider: provider.into(),
            selector: selector.into(),
            display_name: "Live name".into(),
            api: None,
            observed_model: None,
            capabilities: Capabilities {
                efforts: None,
                inputs: None,
                tools: None,
            },
            access: Access::Unknown,
            execution_profile: None,
            inference: None,
            billing: None,
        }
    }

    #[test]
    fn the_shipped_file_is_reviewed_and_covers_each_harness() {
        let models = parse(FILE).expect("bundled file parses");
        assert_eq!(models.len(), super::models().len());
        for harness in [Harness::Omp, Harness::Codex, Harness::ClaudeCode] {
            assert!(models.iter().any(|m| m.harness == harness), "{harness:?}");
        }
        assert!(
            models
                .iter()
                .any(|m| m.harness == Harness::Codex && m.status == Status::Legacy)
        );
        // The file is compiled in: nothing in this module reads a URL or path.
        let source = include_str!("bundled.rs");
        assert!(!source.contains(concat!("reqwest", "::")));
        assert!(!source.contains(concat!("std::fs", "::read")));
    }

    #[test]
    fn invalid_or_duplicate_entries_are_rejected() {
        let entry = json!({"harness":"codex","provider":"codex","selector":"m","displayName":"M","status":"current",
            "efforts":["low"],"tools":true,"inputs":["text"],"minimumHarnessVersion":"0.1.0"});
        let doc = |models: Value| {
            json!({"schemaVersion":1,"reviewedAt":"x","about":"x","sources":{},"models":models})
                .to_string()
        };
        assert_eq!(parse(&doc(json!([entry]))).unwrap().len(), 1);
        assert!(parse(&doc(json!([entry, entry]))).is_err());
        let mut bad = entry.clone();
        bad["minimumHarnessVersion"] = json!("1.0");
        assert!(parse(&doc(json!([bad]))).is_err());
        let mut bad = entry.clone();
        bad["apiKey"] = json!("secret");
        assert!(parse(&doc(json!([bad]))).is_err());
    }

    #[test]
    fn bundled_facts_fill_only_what_discovery_left_unknown() {
        let models = parse(FILE).unwrap();
        let mut live = offering(Harness::ClaudeCode, "claude-code", "sonnet");
        live.capabilities.efforts = Some(vec!["high".into()]);
        let merged = merge_with(&models, &live);
        assert_eq!(merged.capabilities.efforts, Some(vec!["high".into()]));
        assert_eq!(merged.capabilities.tools, Some(true));
        assert_eq!(
            merged.capabilities.inputs,
            Some(vec!["text".into(), "image".into()])
        );
        assert_eq!(merged.display_name, "Live name");
        let facts = facts_with(&models, &merged);
        assert_eq!(facts["facts"], "bundled");
        assert_eq!(facts["status"], "current");
        assert_eq!(facts["unknownCapabilities"], json!([]));
        // A model the live list reports but the file lacks stays listed with
        // its unknown capabilities stated.
        let unknown = offering(Harness::Codex, "codex", "brand-new");
        let merged = merge_with(&models, &unknown);
        assert_eq!(merged, unknown);
        let facts = facts_with(&models, &merged);
        assert_eq!(facts["facts"], "live-only");
        assert_eq!(facts["status"], Value::Null);
        assert_eq!(
            facts["unknownCapabilities"],
            json!(["efforts", "inputs", "tools"])
        );
        let legacy = facts_with(&models, &offering(Harness::Codex, "codex", "gpt-5.5"));
        assert_eq!(legacy["status"], "legacy");
    }

    #[test]
    fn a_model_missing_only_for_version_reasons_names_the_update() {
        let models = parse(FILE).unwrap();
        let listed = vec![offering(Harness::ClaudeCode, "claude-code", "sonnet")];
        let missing = needs_update_with(
            &models,
            &Harness::ClaudeCode,
            "2.1.296 (Claude Code)",
            &listed,
        );
        let fable = missing
            .iter()
            .find(|m| m["selector"] == "fable")
            .expect("fable needs a newer Claude Code");
        assert_eq!(fable["minimumHarnessVersion"], "2.1.303");
        let reason = fable["reason"].as_str().unwrap();
        assert!(reason.contains("Claude Code 2.1.303 or newer"), "{reason}");
        assert!(reason.contains("never updates harnesses"), "{reason}");
        // Sonnet is listed, so it is not reported as needing an update.
        assert!(missing.iter().all(|m| m["selector"] != "sonnet"));
        // A current harness that omits a model gets no update claim.
        assert!(
            needs_update_with(
                &models,
                &Harness::ClaudeCode,
                "2.1.303 (Claude Code)",
                &listed
            )
            .is_empty()
        );
        assert!(needs_update_with(&models, &Harness::Codex, "garbage", &[]).is_empty());
        assert!(!needs_update_with(&models, &Harness::Codex, "codex-cli 0.150.0", &[]).is_empty());
        // OMP lists by configured provider, never by release.
        assert!(needs_update_with(&models, &Harness::Omp, "omp/18.4.12", &[]).is_empty());
    }
}
