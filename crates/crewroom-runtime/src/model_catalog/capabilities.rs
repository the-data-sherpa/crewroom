//! FOR-73: what one harness version supports, as a typed record. Admission,
//! launch and the workbench read this record; the MH2/MH3 runtime contracts
//! reference it instead of restating capabilities in prose.
//!
//! A verified version (on this build's exact list) carries the baseline facts
//! established by live acceptance. A newer version admitted by a FOR-69 probe
//! receipt carries only what the probe exercised: host tool registration and
//! model discovery (with its effort levels) are `probed`; behaviour the probe
//! never exercised is `unverified`; facts a harness never offers stay
//! `unavailable`. Every capability that is not `verified` or `probed` has an
//! explicit fallback — hold, a stated degraded mode, or refusal — so nothing
//! is silently downgraded.
use super::{compatibility, discovery, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Crewroom's addressed host tools register and gate every effect.
    HostTools,
    /// Operator steering reaches a running turn.
    SteerMidTurn,
    /// A running turn stops on request.
    Interrupt,
    /// A replacement attempt resumes the harness's own session history.
    SessionResume,
    /// The requested reasoning effort is applied.
    EffortControl,
    TokenAccounting,
    RequestCounts,
    Cost,
    /// Spend is checked before each provider request.
    PreRequestBudget,
}
pub const ALL: [Capability; 9] = [
    Capability::HostTools,
    Capability::SteerMidTurn,
    Capability::Interrupt,
    Capability::SessionResume,
    Capability::EffortControl,
    Capability::TokenAccounting,
    Capability::RequestCounts,
    Capability::Cost,
    Capability::PreRequestBudget,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    /// Exercised by live acceptance on this exact version.
    Verified,
    /// Exercised by the FOR-69 compatibility probe of this exact executable.
    Probed,
    /// Available as an estimate, never as a measured or billed value.
    Estimated,
    /// Not exercised on this version; the fallback applies.
    Unverified,
    /// The harness route does not provide it; the fallback applies.
    Unavailable,
}
impl Support {
    pub fn usable(self) -> bool {
        matches!(self, Self::Verified | Self::Probed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fallback {
    /// The action waits until it is safe (for example, until a turn ends).
    Hold,
    /// The work runs in a stated reduced mode.
    Degraded,
    /// The work is not admitted.
    Refuse,
}
/// One fallback per capability, whatever the reason it is missing.
pub fn fallback(capability: Capability) -> Fallback {
    match capability {
        Capability::HostTools | Capability::EffortControl => Fallback::Refuse,
        Capability::SteerMidTurn => Fallback::Hold,
        _ => Fallback::Degraded,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub capability: Capability,
    pub support: Support,
    /// Present when `support` is not usable: what Crewroom does instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Fallback>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessCapabilities {
    pub harness: Harness,
    /// The `--version` output this record describes.
    pub version: String,
    pub basis: compatibility::Basis,
    /// `verified-list`, or `probe:<receipt id>` for a probed executable.
    pub source: String,
    pub capabilities: Vec<Entry>,
}

/// Baseline facts for verified versions. Source: live acceptance recorded in
/// the MH2 (Codex), MH3 (Claude Code) and OMP gateway runtime contracts.
fn baseline(harness: &Harness, capability: Capability) -> Support {
    use Capability::*;
    use Support::*;
    match (harness, capability) {
        (_, HostTools | SteerMidTurn | Interrupt | SessionResume | EffortControl) => Verified,
        (_, TokenAccounting) => Verified,
        // The OMP provider gateway sees and checks every request.
        (Harness::Omp, RequestCounts | PreRequestBudget) => Verified,
        (Harness::Omp, Cost) => Unavailable,
        (Harness::Codex, RequestCounts | Cost | PreRequestBudget) => Unavailable,
        (Harness::ClaudeCode, Cost) => Estimated,
        (Harness::ClaudeCode, RequestCounts | PreRequestBudget) => Unavailable,
    }
}

fn entries(support: impl Fn(Capability) -> Support) -> Vec<Entry> {
    ALL.iter()
        .map(|&capability| {
            let support = support(capability);
            Entry {
                capability,
                support,
                fallback: (!support.usable()).then(|| fallback(capability)),
            }
        })
        .collect()
}

impl HarnessCapabilities {
    /// The baseline record for a verified version.
    pub fn verified(harness: &Harness, version: &str) -> Self {
        Self {
            harness: harness.clone(),
            version: version.into(),
            basis: compatibility::Basis::Verified,
            source: "verified-list".into(),
            capabilities: entries(|c| baseline(harness, c)),
        }
    }
    /// The record for a newer version admitted by a probe receipt: only the
    /// checks the receipt names are claimed.
    pub fn probed(receipt: &compatibility::ProbeReceipt) -> Self {
        let harness = &receipt.harness;
        let checked = |check: &str| receipt.checks.iter().any(|c| c == check);
        let handshake = compatibility::handshake_checks(harness)
            .iter()
            .all(|c| checked(c));
        Self {
            harness: harness.clone(),
            version: receipt.version.clone(),
            basis: compatibility::Basis::Probed,
            source: format!("probe:{}", receipt.id),
            capabilities: entries(
                |capability| match (capability, baseline(harness, capability)) {
                    (_, other @ (Support::Unavailable | Support::Estimated)) => other,
                    (Capability::HostTools, _) if handshake => Support::Probed,
                    (Capability::EffortControl, _) if checked(compatibility::MODEL_DISCOVERY) => {
                        Support::Probed
                    }
                    _ => Support::Unverified,
                },
            ),
        }
    }
    /// The record for a discovered identity that may launch; none otherwise.
    pub fn for_identity(harness: &Harness, identity: &discovery::Identity) -> Option<Self> {
        if !compatibility::launchable(harness, identity) {
            return None;
        }
        if compatibility::verified(harness).contains(&identity.version.as_str()) {
            return Some(Self::verified(harness, &identity.version));
        }
        identity
            .compatibility
            .as_ref()
            .and_then(|c| c.probe.as_ref())
            .map(Self::probed)
    }
    /// The record a frozen model was admitted with. Routes admitted before
    /// FOR-73 (and the retained pre-catalog OMP route) ran with the verified
    /// baseline, so they keep it rather than gaining a new restriction.
    /// A record that is present but unreadable claims nothing beyond the
    /// facts a harness never provides.
    pub fn for_model(model: &Value) -> Self {
        let harness = model["resolved"]["adapter"]
            .as_str()
            .map_or(Some(Harness::Omp), Harness::from_kind)
            .unwrap_or(Harness::Omp);
        let version = model["selection"]["identity"]["version"]
            .as_str()
            .unwrap_or("pre-catalog route");
        let frozen = &model["selection"]["harnessCapabilities"];
        if frozen.is_null() {
            return Self::verified(&harness, version);
        }
        match serde_json::from_value::<Self>(frozen.clone()) {
            Ok(record) if record.valid() && record.harness == harness => record,
            _ => Self::unverified(&harness, version),
        }
    }
    fn unverified(harness: &Harness, version: &str) -> Self {
        Self {
            harness: harness.clone(),
            version: version.into(),
            basis: compatibility::Basis::Probed,
            source: "unreadable-record".into(),
            capabilities: entries(|c| match baseline(harness, c) {
                other @ (Support::Unavailable | Support::Estimated) => other,
                _ => Support::Unverified,
            }),
        }
    }
    /// Shape check for a record read back from storage.
    pub fn valid(&self) -> bool {
        self.capabilities.len() == ALL.len()
            && self.capabilities.iter().zip(ALL).all(|(e, c)| {
                e.capability == c && e.fallback == (!e.support.usable()).then(|| fallback(c))
            })
            && valid_text(&self.version)
            && valid_text(&self.source)
    }
    pub fn support(&self, capability: Capability) -> Support {
        self.capabilities
            .iter()
            .find(|e| e.capability == capability)
            .map(|e| e.support)
            .unwrap_or(Support::Unverified)
    }
    pub fn allows(&self, capability: Capability) -> bool {
        self.support(capability).usable()
    }
    fn subject(&self) -> String {
        let label = self.harness.label();
        match self.basis {
            compatibility::Basis::Verified => format!("{label} {}", self.version),
            compatibility::Basis::Probed => format!(
                "{label} {} ({})",
                self.version,
                compatibility::Compatibility {
                    basis: compatibility::Basis::Probed,
                    probe: None
                }
                .label(&self.harness, &self.version)
            ),
        }
    }
    /// Operator wording for one capability that is not usable.
    pub fn explain(&self, capability: Capability) -> Option<String> {
        let support = self.support(capability);
        if support.usable() {
            return None;
        }
        let why = match support {
            Support::Unavailable => "does not provide",
            Support::Estimated => "only estimates",
            _ => "has not been verified for",
        };
        let subject = self.subject();
        Some(match capability {
            Capability::HostTools => format!(
                "{subject} {why} Crewroom's gated host tools, so agents cannot be admitted on it. Install a verified version or one that passes the compatibility probe, then refresh models."
            ),
            Capability::EffortControl => format!(
                "{subject} {why} effort selection, so agents cannot be admitted on it. Refresh models so discovery can report its effort levels."
            ),
            Capability::SteerMidTurn => format!(
                "Steering waits until the current turn ends: {subject} {why} steering a running turn."
            ),
            Capability::Interrupt => format!(
                "Pause and stop end the agent's process without relying on an interrupted turn: {subject} {why} turn interruption."
            ),
            Capability::SessionResume => format!(
                "A replacement agent starts a fresh session from Crewroom's saved record instead of resuming its harness session: {subject} {why} session resume."
            ),
            Capability::TokenAccounting => {
                format!("Token usage may be missing: {subject} {why} token accounting.")
            }
            Capability::RequestCounts => {
                format!("Request counts are unavailable: {subject} {why} per-request counts.")
            }
            Capability::Cost => match support {
                Support::Estimated => format!(
                    "Cost is a list-price estimate, not a billed amount: {subject} {why} cost."
                ),
                _ => format!("Cost is unavailable: {subject} {why} cost."),
            },
            Capability::PreRequestBudget => format!(
                "Crewroom cannot stop spend before each request: {subject} {why} pre-request budget checks. Active-time limits, pause and cancel still apply."
            ),
        })
    }
    fn explained(&self, wanted: Fallback) -> Vec<String> {
        ALL.iter()
            .filter(|c| fallback(**c) == wanted)
            .filter_map(|c| self.explain(*c))
            .collect()
    }
    /// Missing capabilities that refuse admission.
    pub fn refusals(&self) -> Vec<String> {
        self.explained(Fallback::Refuse)
    }
    /// Missing capabilities whose actions wait.
    pub fn holds(&self) -> Vec<String> {
        self.explained(Fallback::Hold)
    }
    /// Missing capabilities that run in a stated reduced mode.
    pub fn degraded(&self) -> Vec<String> {
        self.explained(Fallback::Degraded)
    }
    /// Operator notes for every fallback that applies (holds, then degraded).
    pub fn notes(&self) -> Vec<String> {
        let mut notes = self.holds();
        notes.extend(self.degraded());
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn probed(harness: Harness, version: &str) -> HarnessCapabilities {
        HarnessCapabilities::probed(&compatibility::ProbeReceipt::new(
            &harness,
            version,
            Path::new("/fixture/bin"),
            "abc",
            1,
        ))
    }

    #[test]
    fn verified_versions_carry_the_runtime_contract_baseline() {
        let codex = HarnessCapabilities::verified(&Harness::Codex, "codex-cli 0.163.0");
        assert!(codex.valid());
        for c in [
            Capability::HostTools,
            Capability::SteerMidTurn,
            Capability::Interrupt,
            Capability::SessionResume,
            Capability::EffortControl,
            Capability::TokenAccounting,
        ] {
            assert_eq!(codex.support(c), Support::Verified, "{c:?}");
        }
        for c in [
            Capability::RequestCounts,
            Capability::Cost,
            Capability::PreRequestBudget,
        ] {
            assert_eq!(codex.support(c), Support::Unavailable, "{c:?}");
        }
        assert!(codex.refusals().is_empty() && codex.holds().is_empty());
        let degraded = codex.degraded();
        assert_eq!(degraded.len(), 3);
        assert!(
            degraded
                .iter()
                .any(|n| n.starts_with("Crewroom cannot stop spend"))
        );
        let claude = HarnessCapabilities::verified(&Harness::ClaudeCode, "2.1.302 (Claude Code)");
        assert_eq!(claude.support(Capability::Cost), Support::Estimated);
        assert!(
            claude
                .explain(Capability::Cost)
                .unwrap()
                .starts_with("Cost is a list-price estimate")
        );
        let omp = HarnessCapabilities::verified(&Harness::Omp, "omp/18.11.3");
        assert!(omp.allows(Capability::PreRequestBudget));
        assert!(omp.allows(Capability::RequestCounts));
        assert_eq!(
            omp.notes(),
            ["Cost is unavailable: OMP omp/18.11.3 does not provide cost."]
        );
    }

    #[test]
    fn probed_versions_claim_only_what_the_receipt_checked() {
        for harness in [Harness::Omp, Harness::Codex, Harness::ClaudeCode] {
            let version = match harness {
                Harness::Omp => "omp/19.0.0",
                Harness::Codex => "codex-cli 1.0.0",
                Harness::ClaudeCode => "3.0.0 (Claude Code)",
            };
            let record = probed(harness.clone(), version);
            assert!(record.valid());
            assert!(record.source.starts_with("probe:"));
            assert_eq!(record.support(Capability::HostTools), Support::Probed);
            assert_eq!(record.support(Capability::EffortControl), Support::Probed);
            for c in [
                Capability::SteerMidTurn,
                Capability::Interrupt,
                Capability::SessionResume,
                Capability::TokenAccounting,
            ] {
                assert_eq!(record.support(c), Support::Unverified, "{c:?}");
            }
            assert!(record.refusals().is_empty(), "probed versions still run");
            let holds = record.holds();
            assert_eq!(holds.len(), 1);
            assert!(holds[0].starts_with("Steering waits until the current turn ends"));
            assert!(holds[0].contains("compatible (probed"), "{}", holds[0]);
            assert!(
                record
                    .degraded()
                    .iter()
                    .any(|n| n.starts_with("A replacement agent starts a fresh session"))
            );
            // Facts the harness never provides are not upgraded by a probe.
            assert_eq!(
                record.support(Capability::Cost),
                HarnessCapabilities::verified(&harness, "x").support(Capability::Cost)
            );
        }
    }

    #[test]
    fn a_receipt_without_the_handshake_refuses_admission() {
        let mut receipt = compatibility::ProbeReceipt::new(
            &Harness::Codex,
            "codex-cli 1.0.0",
            Path::new("/fixture/bin"),
            "abc",
            1,
        );
        receipt.checks.truncate(2);
        let record = HarnessCapabilities::probed(&receipt);
        assert_eq!(record.support(Capability::HostTools), Support::Unverified);
        let refusals = record.refusals();
        assert_eq!(refusals.len(), 1);
        assert!(
            refusals[0].contains("cannot be admitted"),
            "{}",
            refusals[0]
        );
    }

    #[test]
    fn frozen_models_keep_their_record_and_older_routes_keep_the_baseline() {
        let record = probed(Harness::ClaudeCode, "3.0.0 (Claude Code)");
        let model = json!({"resolved":{"adapter":"claude-code"},"selection":{"harnessCapabilities":record}});
        assert_eq!(HarnessCapabilities::for_model(&model), record);
        let legacy = json!({"resolved":{"adapter":"codex"},"selection":{"identity":{"version":"codex-cli 0.163.0"}}});
        let baseline = HarnessCapabilities::for_model(&legacy);
        assert_eq!(baseline.harness, Harness::Codex);
        assert!(baseline.allows(Capability::SteerMidTurn));
        let omp = HarnessCapabilities::for_model(&json!({"provider":"p","id":"m"}));
        assert_eq!(omp.harness, Harness::Omp);
        assert!(omp.allows(Capability::SessionResume));
        // A tampered record is not trusted, and it never widens to the
        // verified baseline: nothing beyond unavailable facts is claimed.
        let mut forged = serde_json::to_value(&record).unwrap();
        forged["capabilities"][1]["fallback"] = Value::Null;
        let model = json!({"resolved":{"adapter":"claude-code"},"selection":{"harnessCapabilities":forged}});
        let distrusted = HarnessCapabilities::for_model(&model);
        assert!(ALL.iter().all(|c| !distrusted.allows(*c)));
        assert!(!distrusted.refusals().is_empty());
    }
}
