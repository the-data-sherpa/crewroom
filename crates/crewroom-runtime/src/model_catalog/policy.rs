//! One selection rule for preview and new admissions. Route facts are supplied
//! by discovery plus the existing verified runtime configuration.
use super::*;
use sha2::{Digest, Sha256};

pub fn digest(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(crewroom_core::serialized::stringify(value).as_bytes())
    )
}

#[derive(Clone, Debug)]
pub struct Route {
    pub offering: Offering,
    pub advertised: Capabilities,
    pub fresh: bool,
    pub identity: Option<discovery::Identity>,
    pub configuration_revision: Option<String>,
}

/// Profile capabilities can supply known adapter tools/inputs when metadata
/// does not advertise them. When both describe a capability, take intersection.
pub fn with_profile(discovered: &Offering, verified: Option<&Value>, access: Access) -> Offering {
    let mut result = discovered.clone();
    result.access = access;
    let Some(profile) = verified else {
        return result;
    };
    if profile["provider"] != discovered.provider
        || profile["id"] != discovered.selector
        || !crate::runner::kind(&json!({"resolved":profile}))
            .is_ok_and(|k| k == discovered.harness.as_str())
    {
        return result;
    }
    let accepted = match discovered.harness {
        Harness::Omp => matches!(
            profile["api"].as_str(),
            Some("openai-responses" | "anthropic-messages" | "google-generative-ai")
        ),
        Harness::Codex => profile["api"] == "codex-app-server" && profile["provider"] == "codex",
        Harness::ClaudeCode => {
            profile["api"] == "claude-code-stream"
                && profile["provider"] == "claude-code"
                && discovered
                    .observed_model
                    .as_ref()
                    .is_some_and(|o| profile["resolvedModel"] == o.model)
        }
    };
    if !accepted {
        return result;
    }
    let intersect = |advertised: &Option<Vec<String>>, configured: &Value| -> Option<Vec<String>> {
        let values: Vec<String> = configured
            .as_array()?
            .iter()
            .map(|v| v.as_str().map(String::from))
            .collect::<Option<_>>()?;
        Some(
            values
                .into_iter()
                .filter(|v| advertised.as_ref().is_none_or(|a| a.contains(v)))
                .collect(),
        )
    };
    result.api = profile["api"].as_str().map(String::from);
    result.execution_profile = Some(
        match discovered.harness {
            Harness::Omp => "omp-18.4.12-host-tools-v2",
            Harness::Codex => "codex-0.162.0-dynamic-tools-v1",
            Harness::ClaudeCode => "claude-2.1.296-safe-sdk-tools-v1",
        }
        .into(),
    );
    result.capabilities = Capabilities {
        efforts: discovered
            .capabilities
            .efforts
            .as_ref()
            .and_then(|_| intersect(&discovered.capabilities.efforts, &profile["efforts"])),
        inputs: intersect(&discovered.capabilities.inputs, &profile["inputs"]),
        tools: profile["tools"]
            .as_bool()
            .map(|v| v && discovered.capabilities.tools != Some(false)),
    };
    result
}

pub fn resolve(
    role: &Value,
    policy: &Value,
    requirements: &Value,
    routes: &[Route],
    groups: &[Equivalence],
    catalog_revision: u64,
) -> Result<Value> {
    if role["enabled"] != true {
        return Err(conflict("Role is disabled."));
    }
    let intent = crate::model_policy::intent(role, policy)?;
    let offerings: Vec<_> = routes.iter().map(|r| r.offering.clone()).collect();
    let choices = candidates(intent.model, &offerings, groups)?;
    let mut blockers = Vec::new();
    for choice in choices {
        let route = routes
            .iter()
            .find(|r| r.offering.key == choice.key)
            .unwrap();
        let reasons = choice.blockers(route.fresh, intent.effort, requirements);
        if !reasons.is_empty() {
            blockers.push(format!("{}: {}", choice.display_name, reasons.join(" ")));
            continue;
        }
        let policy_revision =
            digest(&json!({"policy":policy,"requirements":requirements,"equivalence":groups}));
        let role_revision = digest(role);
        let reason = if intent.model == choice.key {
            "Exact offering selected."
        } else {
            "First eligible route in the explicit equivalence preference order."
        };
        let mut selection = json!({"schemaVersion":SCHEMA_VERSION,"intendedModel":intent.model,
            "modelSource":if role["model"].as_str().is_some_and(|s|!s.is_empty()) {"role_override"}else{"job"},
            "effortSource":if role["effort"].as_str().is_some_and(|s|!["","default"].contains(&s)){"role_override"}else{"job_budget"},
            "requestedEffort":intent.effort,"billingRoute":choice.billing,"offering":choice,"advertisedCapabilities":route.advertised,
            "identity":route.identity,"configurationRevision":route.configuration_revision,"catalogRevision":catalog_revision,
            "policyRevision":policy_revision,"roleRevision":role_revision,"reason":reason});
        selection["previewRevision"] = json!(digest(&selection));
        return Ok(
            json!({"requested":{"model":role["model"],"effort":role["effort"]},"resolved":choice.resolved(intent.effort),
            "selection":selection,"job":intent.job,"budget":intent.budget,"warning":intent.warning}),
        );
    }
    Err(conflict(&format!(
        "No eligible route. {}",
        blockers.join(" ")
    )))
}

pub fn verify_preview(model: &Value, expected: &Value) -> Result<()> {
    if (model["selection"].is_object() || !expected.is_null())
        && expected != &model["selection"]["previewRevision"]
    {
        return Err(conflict(
            "Model selection, policy or catalog changed. Refresh the preview before admission.",
        ));
    }
    Ok(())
}

/// Picker facts use the admission capability predicate. UI clients do not
/// infer support from display names or implement their own eligibility policy.
pub fn choices(routes: &[Route], groups: &[Equivalence], requirements: &Value) -> Result<Value> {
    let offerings: Vec<_> = routes.iter().map(|r| r.offering.clone()).collect();
    validate_contract(&offerings, groups)?;
    let route_for = |key: &String| -> Result<&Route> {
        let offering = find(key, &offerings)?.unwrap();
        Ok(routes
            .iter()
            .find(|r| r.offering.key == offering.key)
            .unwrap())
    };
    // Each selection: its public header plus routes in preference order.
    let mut selections: Vec<(Value, Vec<&Route>)> = routes
        .iter()
        .map(|r| {
            let billing = r.offering.billing.unwrap_or(BillingRoute::Unknown);
            (
                json!({"key":r.offering.key,"displayName":r.offering.display_name,
                    "label":r.offering.label(),"billingRoute":billing}),
                vec![r],
            )
        })
        .collect();
    for g in groups {
        selections.push((
            json!({"key":g.key,"displayName":g.display_name,"label":g.display_name,"billingRoute":null}),
            g.preference.iter().map(route_for).collect::<Result<_>>()?,
        ));
    }
    Ok(json!(selections.into_iter().map(|(mut choice, candidates)| {
        let mut eligibility = json!({});
        for effort in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
            let mut reasons = Vec::new();
            let mut selected = None;
            for route in &candidates {
                let blocked = route.offering.blockers(route.fresh, effort, requirements);
                if blocked.is_empty() { selected = Some(route.offering.key.clone()); break; }
                reasons.extend(blocked.into_iter().map(|reason| format!("{} / {} / {}: {reason}", route.offering.harness.as_str(), route.offering.provider, route.offering.billing.unwrap_or(BillingRoute::Unknown).as_str())));
            }
            eligibility[effort] = json!({"eligible":selected.is_some(),"routeKey":selected,"reasons":if selected.is_some() { vec![] } else { reasons }});
        }
        choice["routes"] = json!(candidates.iter().map(|r| &r.offering).collect::<Vec<_>>());
        choice["eligibility"] = eligibility;
        choice
    }).collect::<Vec<_>>()))
}
