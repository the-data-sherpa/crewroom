//! Execution profiles approved per discovered offering (FOR-87).
//!
//! Discovery lists what a harness offers; it never grants a profile. The
//! operator approves an offering explicitly and chooses efforts within the
//! advertised set. The service derives the profile from the discovered facts
//! and writes it through the validated runner save path. An approved profile
//! means agents may run on the offering; it is not proven inference.
use super::*;
use crewroom_runtime::model_catalog::{Capabilities, Harness, Offering};

/// The APIs an OMP execution profile may name (see `policy::with_profile`).
pub const OMP_APIS: [&str; 3] = [
    "openai-responses",
    "anthropic-messages",
    "google-generative-ai",
];
const NOTE: &str = "Approving lets agents run on a model through this harness. It does not prove the model answers: access is confirmed only when an agent runs. Discovery never approves a model.";

/// Whether the configuration names this harness for the runner.
pub(super) fn harness_configured(config: &Value, harness: &Harness) -> bool {
    match harness {
        Harness::Omp => config["ompBinary"].as_str().is_some_and(|s| !s.is_empty()),
        Harness::Codex => {
            config["codex"]["binary"].is_string() && config["codex"]["authFile"].is_string()
        }
        Harness::ClaudeCode => {
            config["claudeCode"]["binary"].is_string() && config["claudeCode"]["home"].is_string()
        }
    }
}

/// The same identity rule the runner uses to pair a profile with an offering.
pub fn matches(profile: &Value, harness: &Harness, provider: &str, selector: &str) -> bool {
    profile["provider"] == provider
        && profile["id"] == selector
        && crewroom_core::runner_kind(&json!({ "resolved": profile }))
            .is_ok_and(|k| k == harness.as_str())
}

/// Derives the execution profile for `offering` (carrying its advertised
/// capabilities) with the operator's `efforts`. `providers` is the runner's
/// `models.providers`, which names the API an OMP provider speaks.
pub fn derive(
    offering: &Offering,
    efforts: &[String],
    providers: &Value,
) -> std::result::Result<Value, String> {
    let advertised = offering
        .capabilities
        .efforts
        .as_ref()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            format!(
                "{} did not advertise reasoning efforts for {}, so no effort can be approved. Refresh models, or add the profile under Advanced → catalog.",
                offering.harness.label(),
                offering.display_name
            )
        })?;
    if efforts.is_empty() {
        return Err("Choose at least one effort to approve.".into());
    }
    if let Some(effort) = efforts.iter().find(|e| !advertised.contains(e)) {
        return Err(format!(
            "Effort {effort} is not advertised for {}. Choose from: {}.",
            offering.display_name,
            advertised.join(", ")
        ));
    }
    // Keep the advertised order; drop repeats.
    let chosen: Vec<&String> = advertised.iter().filter(|a| efforts.contains(a)).collect();
    let mut profile = match offering.harness {
        Harness::Codex => {
            if offering.provider != "codex" {
                return Err("Codex offerings must come from the codex provider.".into());
            }
            json!({"adapter":"codex","provider":"codex","id":offering.selector,"api":"codex-app-server"})
        }
        Harness::ClaudeCode => {
            if offering.provider != "claude-code" {
                return Err(
                    "Claude Code offerings must come from the claude-code provider.".into(),
                );
            }
            let observed = offering.observed_model.as_ref().ok_or_else(|| {
                format!(
                    "Claude Code did not report the model behind {}, so the profile cannot name it. Refresh models.",
                    offering.display_name
                )
            })?;
            json!({"adapter":"claude-code","provider":"claude-code","id":offering.selector,
                "api":"claude-code-stream","resolvedModel":observed.model})
        }
        Harness::Omp => {
            let provider = &providers[offering.provider.as_str()];
            if !provider.is_object() {
                return Err(format!(
                    "Provider {} is not defined under Advanced → models.providers, so OMP cannot launch this model.",
                    offering.provider
                ));
            }
            let api = provider["api"]
                .as_str()
                .filter(|a| OMP_APIS.contains(a))
                .ok_or_else(|| {
                    format!(
                        "Set api for provider {} under Advanced → models.providers to one of: {}.",
                        offering.provider,
                        OMP_APIS.join(", ")
                    )
                })?;
            json!({"provider":offering.provider,"id":offering.selector,"api":api})
        }
    };
    profile["inputs"] = json!(["text"]);
    profile["efforts"] = json!(chosen);
    profile["tools"] = json!(true);
    Ok(profile)
}

/// Discovered offerings, one per harness, provider and selector, with the
/// capabilities discovery advertised (not those narrowed by a profile).
pub(super) fn discovered(catalog: &Value) -> Vec<(Offering, bool)> {
    let mut found: Vec<(Offering, bool)> = Vec::new();
    for route in array(&catalog["offerings"]) {
        let Ok(mut offering) = serde_json::from_value::<Offering>(route["offering"].clone()) else {
            continue;
        };
        if let Ok(advertised) =
            serde_json::from_value::<Capabilities>(route["advertisedCapabilities"].clone())
        {
            offering.capabilities = advertised;
        }
        let fresh = route["fresh"] == true;
        match found.iter_mut().find(|(o, _)| {
            o.harness == offering.harness
                && o.provider == offering.provider
                && o.selector == offering.selector
        }) {
            Some((_, seen)) => *seen = *seen && fresh,
            None => {
                offering.billing = None;
                offering.key = offering.legacy_key();
                found.push((offering, fresh));
            }
        }
    }
    found
}

/// The Settings → Runner list: discovered offerings per configured harness,
/// whether each has a profile, and profiles no discovery listed.
pub(super) fn listing(config: Option<&Value>, catalog: &Value) -> Value {
    let empty = json!({});
    let config = config.unwrap_or(&empty);
    let profiles = array(&config["catalog"]);
    let offerings = discovered(catalog);
    let mut claimed = vec![false; profiles.len()];
    let mut harnesses = Vec::new();
    for harness in [Harness::Omp, Harness::Codex, Harness::ClaudeCode] {
        let configured = harness_configured(config, &harness);
        let mut rows = Vec::new();
        for (offering, fresh) in offerings.iter().filter(|(o, _)| o.harness == harness) {
            let profile = profiles
                .iter()
                .enumerate()
                .find(|(_, p)| matches(p, &harness, &offering.provider, &offering.selector));
            if let Some((index, _)) = profile {
                claimed[index] = true;
            }
            if !configured {
                continue;
            }
            let advertised = offering.capabilities.efforts.clone().unwrap_or_default();
            let refusal = derive(offering, &advertised, &config["models"]["providers"]).err();
            rows.push(json!({
                "harness": harness.as_str(),
                "provider": offering.provider,
                "selector": offering.selector,
                "displayName": offering.display_name,
                "resolvedModel": offering.observed_model.as_ref().map(|o| &o.model),
                "advertisedEfforts": offering.capabilities.efforts,
                "fresh": fresh,
                "profile": profile.map(|(_, p)| json!({"efforts": p["efforts"], "api": p["api"]})),
                "approvable": refusal.is_none(),
                "reason": refusal,
            }));
        }
        let note = if !configured {
            Some(format!(
                "{} is not configured for the runner; its models are not listed here.",
                harness.label()
            ))
        } else if rows.is_empty() {
            Some(format!(
                "Discovery has not listed {} models for this service yet. Apply the saved settings, then refresh models in Settings → Models & budget.",
                harness.label()
            ))
        } else {
            None
        };
        harnesses.push(json!({
            "harness": harness.as_str(),
            "label": harness.label(),
            "configured": configured,
            "note": note,
            "offerings": rows,
        }));
    }
    let unmatched: Vec<Value> = profiles
        .iter()
        .zip(&claimed)
        .filter(|(_, claimed)| !**claimed)
        .map(|(p, _)| {
            let kind = crewroom_core::runner_kind(&json!({ "resolved": p })).ok();
            json!({"harness": kind, "provider": p["provider"], "selector": p["id"], "efforts": p["efforts"]})
        })
        .collect();
    json!({
        "discovery": catalog["state"].as_str().map(|s| if s == "unavailable" { "unavailable" } else { "available" }),
        "discoveryReason": catalog["reason"],
        "note": NOTE,
        "harnesses": harnesses,
        "unmatched": unmatched,
    })
}

/// A requested approval or removal.
pub(super) struct Request {
    harness: Harness,
    provider: String,
    selector: String,
    approve: bool,
    efforts: Vec<String>,
}
pub(super) fn request(body: &Value) -> Result<Request> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid("Send the profile change as an object."))?;
    if let Some(key) = object
        .keys()
        .find(|k| !["harness", "provider", "selector", "approve", "efforts"].contains(&k.as_str()))
    {
        return Err(invalid(format!("Unknown profile field: {key}.")));
    }
    let harness = body["harness"]
        .as_str()
        .and_then(Harness::from_kind)
        .ok_or_else(|| invalid("harness must be omp, codex or claude-code."))?;
    let text = |key: &str| {
        body[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| invalid(format!("{key} is required.")))
    };
    let approve = body["approve"]
        .as_bool()
        .ok_or_else(|| invalid("approve must be true or false."))?;
    let efforts: Vec<String> = match &body["efforts"] {
        Value::Null => Vec::new(),
        value => serde_json::from_value(value.clone())
            .map_err(|_| invalid("efforts must list effort names."))?,
    };
    if !approve && !efforts.is_empty() {
        return Err(invalid("Removing a profile takes no efforts."));
    }
    Ok(Request {
        harness,
        provider: text("provider")?,
        selector: text("selector")?,
        approve,
        efforts,
    })
}

/// The saved configuration with this profile approved or removed, as a body
/// for the validated save path. Secrets stay where they are: references are
/// kept by the save path; inline values move to the encrypted store as a
/// form save would move them.
pub(super) fn change(existing: &Value, request: &Request, catalog: &Value) -> Result<Value> {
    let profiles = array(&existing["catalog"]);
    let (kept, matched): (Vec<&Value>, Vec<&Value>) = profiles
        .iter()
        .partition(|p| !matches(p, &request.harness, &request.provider, &request.selector));
    let mut next: Vec<Value> = kept.into_iter().cloned().collect();
    let name = format!(
        "{} {}/{}",
        request.harness.label(),
        request.provider,
        request.selector
    );
    if request.approve {
        let (offering, _) = discovered(catalog)
            .into_iter()
            .find(|(o, _)| {
                o.harness == request.harness
                    && o.provider == request.provider
                    && o.selector == request.selector
            })
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "{name} is not in the latest model discovery. Refresh models, then approve it."
                ))
            })?;
        if !harness_configured(existing, &request.harness) {
            return Err(conflict(format!(
                "{} is not configured for the runner. Save its executable first.",
                request.harness.label()
            )));
        }
        let profile = derive(
            &offering,
            &request.efforts,
            &existing["models"]["providers"],
        )
        .map_err(invalid)?;
        next.push(profile);
    } else if matched.is_empty() {
        return Err(Error::NotFound(format!(
            "No execution profile is saved for {name}."
        )));
    }
    let mut advanced = existing.clone();
    if let Some(object) = advanced.as_object_mut() {
        for key in FORM_KEYS {
            object.remove(key);
        }
    }
    advanced["catalog"] = json!(next);
    if let Some(providers) = advanced["models"]["providers"].as_object_mut() {
        for provider in providers.values_mut() {
            if let Some(provider) = provider.as_object_mut()
                && provider
                    .get("apiKey")
                    .is_some_and(|k| referenced(k).is_some())
            {
                provider.remove("apiKey");
            }
        }
    }
    let mut body = json!({"fields": form(existing), "advanced": advanced});
    if let Some(token) = existing["mail"]["token"].as_str().filter(|s| !s.is_empty()) {
        body["secrets"] = json!({"mail": token});
    }
    Ok(body)
}

#[cfg(test)]
mod tests;
