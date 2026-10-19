//! Credential-free model identity. Discovery, access, execution support and
//! inference observations are independent facts, never inferred from labels.
use crate::conflict;
use crewroom_core::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

pub use billing::BillingRoute;

pub mod billing;
pub mod bundled;
pub mod capabilities;
pub mod codex_protocol;
pub mod compatibility;
pub mod discovery;
pub mod known_broken;
pub mod login;
pub mod policy;
mod probe;
pub mod store;
pub mod unconfigured;

pub const SCHEMA_VERSION: u32 = 1;
pub const FRESH_FOR_SECONDS: u64 = 3600;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Omp,
    Codex,
    ClaudeCode,
}
impl Harness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Omp => "omp",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }
    /// The runner kind a frozen model names (`omp`, `codex`, `claude-code`).
    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "omp" => Some(Self::Omp),
            "codex" => Some(Self::Codex),
            "claude-code" => Some(Self::ClaudeCode),
            _ => None,
        }
    }
    /// Product name for operator-facing explanations.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Omp => "OMP",
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
        }
    }
}

/// Length-delimited through percent encoding; provider and selector cannot
/// collide with separators. Display names and observed aliases are not keys.
pub fn offering_key(harness: &Harness, provider: &str, selector: &str) -> String {
    let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!(
        "offering:{}:{}:{}",
        harness.as_str(),
        encode(provider),
        encode(selector)
    )
}

/// An offering bound to one billing route. The route is part of the identity,
/// so a selection can never fall back to the same model on another route.
pub fn routed_key(
    harness: &Harness,
    provider: &str,
    selector: &str,
    billing: BillingRoute,
) -> String {
    format!(
        "{}:{}",
        offering_key(harness, provider, selector),
        billing.as_str()
    )
}

pub fn is_selection(value: &str) -> bool {
    value.starts_with("offering:") || value.starts_with("equivalent:")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Unknown,
    Configured,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub efforts: Option<Vec<String>>,
    pub inputs: Option<Vec<String>>,
    pub tools: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub model: String,
    pub source: String,
    pub observed_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Offering {
    pub key: String,
    pub harness: Harness,
    pub provider: String,
    /// The exact harness argument, including an alias such as `default`.
    pub selector: String,
    pub display_name: String,
    pub api: Option<String>,
    pub observed_model: Option<Observation>,
    pub capabilities: Capabilities,
    pub access: Access,
    /// Requires an accepted execution profile, separately from discovery.
    pub execution_profile: Option<String>,
    pub inference: Option<Observation>,
    /// Detected from the harness's authentication configuration. Discovery
    /// metadata alone carries none; catalog routes and frozen selections do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<BillingRoute>,
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 1000 && !value.chars().any(char::is_control)
}
impl Offering {
    /// Pre-billing identity: harness, provider and selector only.
    pub fn legacy_key(&self) -> String {
        offering_key(&self.harness, &self.provider, &self.selector)
    }
    fn expected_key(&self) -> String {
        match self.billing {
            Some(billing) => routed_key(&self.harness, &self.provider, &self.selector, billing),
            None => self.legacy_key(),
        }
    }
    /// This discovered offering bound to one detected billing route.
    pub fn with_billing(&self, billing: BillingRoute) -> Self {
        let mut routed = self.clone();
        routed.billing = Some(billing);
        routed.key = routed.expected_key();
        routed
    }
    /// Picker label: harness, model and billing route.
    pub fn label(&self) -> String {
        format!(
            "{} · {} · {}",
            self.harness.label(),
            self.display_name,
            self.billing.unwrap_or(BillingRoute::Unknown).label()
        )
    }
    pub fn validate(&self) -> Result<()> {
        if self.key != self.expected_key()
            || !valid_text(&self.key)
            || ![&self.provider, &self.selector, &self.display_name]
                .into_iter()
                .all(|s| valid_text(s))
            || self.api.as_ref().is_some_and(|s| !valid_text(s))
            || self
                .execution_profile
                .as_ref()
                .is_some_and(|s| !valid_text(s))
            || [&self.observed_model, &self.inference]
                .into_iter()
                .flatten()
                .any(|o| !valid_text(&o.model) || !valid_text(&o.source))
        {
            return Err(conflict("Invalid model offering identity."));
        }
        for values in [&self.capabilities.efforts, &self.capabilities.inputs]
            .into_iter()
            .flatten()
        {
            let mut seen = HashSet::new();
            if values.len() > 100 || values.iter().any(|s| !valid_text(s) || !seen.insert(s)) {
                return Err(conflict("Invalid model capabilities."));
            }
        }
        Ok(())
    }

    /// Report all unmet requirements without claiming configured access is
    /// proven inference. Freshness is supplied by the durable harness snapshot.
    pub fn blockers(&self, fresh: bool, effort: &str, requirements: &Value) -> Vec<String> {
        let mut reasons = Vec::new();
        if !fresh {
            reasons.push("Refresh this harness before admission.".into());
        }
        if self.access != Access::Configured {
            reasons.push("Model access is unavailable or unverified.".into());
        }
        if self.execution_profile.is_none() || self.api.is_none() {
            reasons.push("No verified adapter profile supports this offering.".into());
        }
        if self.harness == Harness::Codex && self.billing == Some(BillingRoute::Api) {
            reasons.push(
                "Crewroom starts Codex only with its ChatGPT subscription login; the API key route cannot start agents.".into(),
            );
        }
        if self
            .capabilities
            .efforts
            .as_ref()
            .is_none_or(|v| !v.iter().any(|e| e == effort))
        {
            reasons.push(format!("Effort {effort} is unsupported or unknown."));
        }
        if requirements["tools"] == true && self.capabilities.tools != Some(true) {
            reasons.push("Required tool capability is unavailable or unverified.".into());
        }
        if let Some(inputs) = requirements["inputs"].as_array() {
            for input in inputs {
                if self
                    .capabilities
                    .inputs
                    .as_ref()
                    .is_none_or(|v| !v.iter().any(|i| input == i))
                {
                    reasons.push("Required input capability is unavailable or unknown.".into());
                    break;
                }
            }
        }
        if let Some(allowed) = requirements.get("allowedModels") {
            let legacy = format!("{}/{}", self.provider, self.selector);
            if !allowed.as_array().is_some_and(|a| {
                a.iter()
                    .any(|m| m == &self.key || m == &self.legacy_key() || m == &legacy)
            }) {
                reasons.push("Project disallows this model.".into());
            }
        }
        reasons
    }

    /// Existing adapters consume this exact shape. Claude's requested alias
    /// and discovery observation remain separate; runtime readback verifies it.
    pub fn resolved(&self, effort: &str) -> Value {
        json!({"adapter":self.harness.as_str(),"provider":self.provider,"id":self.selector,
            "api":self.api,"selector":format!("{}/{}",self.provider,self.selector),
            "effort":effort,"resolvedModel":self.observed_model.as_ref().map(|o| &o.model),
            "efforts":self.capabilities.efforts,"inputs":self.capabilities.inputs,"tools":self.capabilities.tools})
    }
}

/// Flat, explicit equivalence sets. References may only name offerings, so
/// recursive groups/cycles cannot be represented. Preference is a strict total
/// order of the members; no implicit tie-breaking or label-based equivalence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Equivalence {
    pub key: String,
    pub display_name: String,
    pub members: Vec<String>,
    pub preference: Vec<String>,
}

pub fn validate_contract(offerings: &[Offering], groups: &[Equivalence]) -> Result<()> {
    let mut keys = HashSet::new();
    for offering in offerings {
        offering.validate()?;
        if !keys.insert(offering.key.as_str()) {
            return Err(conflict("Duplicate model offering identity."));
        }
    }
    let mut group_keys = HashSet::new();
    let mut grouped: HashSet<String> = HashSet::new();
    for group in groups {
        let members: HashSet<_> = group.members.iter().collect();
        let preferences: HashSet<_> = group.preference.iter().collect();
        if !group.key.starts_with("equivalent:")
            || group.key.len() <= 11
            || !valid_text(&group.key)
            || !valid_text(&group.display_name)
            || !group_keys.insert(&group.key)
            || members.len() < 2
            || members.len() != group.members.len()
            || preferences != members
            || group.preference.len() != members.len()
            || group.members.iter().any(|m| {
                !find(m, offerings).is_ok_and(|o| o.is_some_and(|o| grouped.insert(o.key.clone())))
            })
        {
            return Err(conflict(
                "Invalid equivalence mapping or unresolved route preference.",
            ));
        }
    }
    Ok(())
}

pub fn candidates<'a>(
    selection: &str,
    offerings: &'a [Offering],
    groups: &[Equivalence],
) -> Result<Vec<&'a Offering>> {
    validate_contract(offerings, groups)?;
    if let Some(offering) = find(selection, offerings)? {
        return Ok(vec![offering]);
    }
    if let Some(group) = groups.iter().find(|g| g.key == selection) {
        return group
            .preference
            .iter()
            .map(|k| find(k, offerings).map(Option::unwrap))
            .collect();
    }
    Err(conflict(
        "Saved model selection is unavailable. Refresh or choose an available model.",
    ))
}

/// Resolve a selection to one offering. A selection saved before billing
/// routes existed names harness, provider and model only; it resolves when
/// exactly one route offers that model, and otherwise asks for an explicit
/// choice instead of picking a route.
pub fn find<'a>(selection: &str, offerings: &'a [Offering]) -> Result<Option<&'a Offering>> {
    if let Some(offering) = offerings.iter().find(|o| o.key == selection) {
        return Ok(Some(offering));
    }
    let legacy: Vec<_> = offerings
        .iter()
        .filter(|o| o.billing.is_some() && o.legacy_key() == selection)
        .collect();
    match legacy.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        many => Err(conflict(&format!(
            "{} is reachable through {} billing. Choose one billing route explicitly.",
            many[0].display_name,
            many.iter()
                .map(|o| o.billing.unwrap().label())
                .collect::<Vec<_>>()
                .join(" and ")
        ))),
    }
}

/// FOR-73 facts for one harness row of a catalog view: the capability record
/// of its discovered version with the fallbacks in operator words, bundled
/// models the installed version predates, and (OMP) a summary of the login
/// listing in place of the raw listing.
pub fn harness_facts(entry: &mut Value, saved: &store::HarnessSnapshot, runner_providers: &Value) {
    let record = saved
        .identity
        .as_ref()
        .and_then(|i| capabilities::HarnessCapabilities::for_identity(&saved.harness, i));
    entry["capabilities"] = json!(record);
    entry["refusals"] = json!(record.as_ref().map(|r| r.refusals()).unwrap_or_default());
    entry["degradedModes"] = json!(record.as_ref().map(|r| r.notes()).unwrap_or_default());
    entry["needsUpdate"] = json!(
        saved
            .identity
            .as_ref()
            .map(|i| bundled::needs_update(&saved.harness, &i.version, &saved.offerings))
            .unwrap_or_default()
    );
    entry["login"] = login::summary(saved.login.as_ref(), runner_providers);
}
