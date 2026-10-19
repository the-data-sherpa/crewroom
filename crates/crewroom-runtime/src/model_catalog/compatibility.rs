//! FOR-69 runtime-version contract. The exact verified list stays the
//! baseline; any newer release (including a new major) may run once a
//! metadata-only compatibility probe passes for that exact executable.
//! FOR-71 adds the Codex protocol diff, the bundled known-broken list and a
//! probe isolation rule: a health probe never triggers login, hooks, MCP
//! servers or IDE setup.
use super::*;
use std::path::PathBuf;

/// Names of the probe checks, in the order they run. The probe never requests
/// model inference: every check is a version, metadata or protocol exchange.
pub const VERSION: &str = "version";
pub const MODEL_DISCOVERY: &str = "model-discovery";
pub const CODEX_CATALOG: &str = "bundled-model-catalog";
pub const CODEX_SCHEMA: &str = "protocol-schema";
pub const CODEX_CONTROL: &str = "control-protocol";
pub const CLAUDE_STREAM: &str = "stream-protocol";
pub const OMP_RPC: &str = "rpc-frame-contract";
/// The bundled known-broken list (`known_broken::LIST`); checked with the
/// version, before any other probe process starts.
pub const KNOWN_BROKEN: &str = "known-broken";
/// v2 (FOR-71): Codex protocol diff and Claude Code probe isolation. A v1
/// receipt no longer admits a version; it is probed again.
pub const CONTRACT: &str = "harness-compatibility-probe-v2";
/// Retained receipts across harnesses; oldest are dropped first.
pub const MAX_RECEIPTS: usize = 32;

pub fn handshake_checks(harness: &Harness) -> &'static [&'static str] {
    match harness {
        Harness::Codex => &[CODEX_CATALOG, CODEX_SCHEMA, CODEX_CONTROL],
        Harness::ClaudeCode => &[CLAUDE_STREAM],
        Harness::Omp => &[OMP_RPC],
    }
}

/// The baseline this build verified with live launches. Discovery and launch
/// use the same list, so a version is either verified for both or neither.
pub fn verified(harness: &Harness) -> &'static [&'static str] {
    discovery::verified_versions(harness)
}

/// `--version` output reduced to its numeric release. Pre-release or
/// decorated outputs are not parsed; they are refused, never guessed.
pub fn parse(harness: &Harness, reported: &str) -> Option<[u64; 3]> {
    let number = match harness {
        Harness::Codex => reported.strip_prefix("codex-cli "),
        Harness::ClaudeCode => reported.strip_suffix(" (Claude Code)"),
        Harness::Omp => reported.strip_prefix("omp/"),
    }?;
    let parts: Vec<_> = number.split('.').collect();
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// On this build's exact verified list.
    Verified,
    /// Newer than the oldest verified version; admitted by a passed probe.
    Probed,
}

/// Probe evidence for one exact executable. Keyed by harness, version,
/// canonical path and content hash; any change to these needs a new probe.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeReceipt {
    pub id: String,
    pub contract: String,
    pub harness: Harness,
    pub version: String,
    pub canonical_binary: PathBuf,
    pub binary_sha256: String,
    pub checks: Vec<String>,
    pub inference_requested: bool,
    pub probed_at: u64,
    /// Codex: notification differences from the reviewed protocol baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<super::codex_protocol::ProtocolDiff>,
}
impl ProbeReceipt {
    pub fn new(
        harness: &Harness,
        version: &str,
        canonical_binary: &std::path::Path,
        binary_sha256: &str,
        probed_at: u64,
    ) -> Self {
        let mut checks = vec![VERSION.to_owned(), MODEL_DISCOVERY.to_owned()];
        checks.extend(handshake_checks(harness).iter().map(|c| (*c).to_owned()));
        let mut receipt = Self {
            id: String::new(),
            contract: CONTRACT.into(),
            harness: harness.clone(),
            version: version.into(),
            canonical_binary: canonical_binary.into(),
            binary_sha256: binary_sha256.into(),
            checks,
            inference_requested: false,
            probed_at,
            protocol: None,
        };
        receipt.id = policy::digest(&json!(receipt));
        receipt
    }
    /// Record the protocol differences the probe observed; re-signs the id.
    pub fn with_protocol(mut self, protocol: Option<super::codex_protocol::ProtocolDiff>) -> Self {
        self.protocol = protocol;
        self.id = String::new();
        self.id = policy::digest(&json!(self));
        self
    }
    /// A receipt is reusable only for the identical executable and contract.
    pub fn covers(
        &self,
        harness: &Harness,
        version: &str,
        canonical_binary: &std::path::Path,
        binary_sha256: &str,
    ) -> bool {
        self.valid()
            && &self.harness == harness
            && self.version == version
            && self.canonical_binary == canonical_binary
            && self.binary_sha256 == binary_sha256
    }
    pub fn valid(&self) -> bool {
        let mut unsigned = self.clone();
        unsigned.id = String::new();
        self.contract == CONTRACT
            && !self.inference_requested
            && self.id == policy::digest(&json!(unsigned))
            && self.checks
                == [VERSION, MODEL_DISCOVERY]
                    .iter()
                    .chain(handshake_checks(&self.harness))
                    .map(|c| (*c).to_owned())
                    .collect::<Vec<_>>()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Compatibility {
    pub basis: Basis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbeReceipt>,
}
impl Compatibility {
    /// Operator wording: `verified` versus `compatible (probed <version>)`.
    pub fn label(&self, harness: &Harness, version: &str) -> String {
        match self.basis {
            Basis::Verified => "verified".into(),
            Basis::Probed => format!(
                "compatible (probed {})",
                parse(harness, version)
                    .map(|[a, b, c]| format!("{a}.{b}.{c}"))
                    .unwrap_or_else(|| printable(version))
            ),
        }
    }
}

/// Whether this exact discovered identity may launch agents: listed, or
/// probed with a receipt bound to the same version and executable content.
pub fn launchable(harness: &Harness, identity: &discovery::Identity) -> bool {
    if broken(harness, &identity.version).is_some() {
        return false;
    }
    if verified(harness).contains(&identity.version.as_str()) {
        return true;
    }
    identity.compatibility.as_ref().is_some_and(|c| {
        c.basis == Basis::Probed
            && c.probe.as_ref().is_some_and(|p| {
                p.covers(
                    harness,
                    &identity.version,
                    &identity.canonical_binary,
                    &identity.binary_sha256,
                )
            })
    })
}

fn printable(reported: &str) -> String {
    let reported: String = reported
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    if reported.is_empty() {
        "unknown".into()
    } else {
        reported
    }
}

/// Decide what a reported version needs. Refusals name the failing check and
/// a remedy; nothing falls back to another harness or to free text.
pub fn classify(harness: &Harness, reported: &str) -> std::result::Result<Basis, String> {
    classify_with(harness, reported, known_broken::LIST)
}

/// The shipped list; the only list discovery and launch consult.
pub fn known_broken_list() -> &'static [known_broken::KnownBroken] {
    known_broken::LIST
}

/// The shipped known-broken entry refusing this reported version, if any.
pub fn broken(harness: &Harness, reported: &str) -> Option<&'static known_broken::KnownBroken> {
    known_broken::find(known_broken::LIST, harness, parse(harness, reported)?)
}

/// `classify` against an explicit known-broken list (tests use synthetic
/// entries; production always passes the shipped `known_broken::LIST`).
pub fn classify_with(
    harness: &Harness,
    reported: &str,
    broken: &[known_broken::KnownBroken],
) -> std::result::Result<Basis, String> {
    if let Some(entry) =
        parse(harness, reported).and_then(|v| known_broken::find(broken, harness, v))
    {
        return Err(known_broken::refusal(entry, reported));
    }
    let list = verified(harness);
    if list.contains(&reported) {
        return Ok(Basis::Verified);
    }
    let label = harness.label();
    let lowest = list
        .iter()
        .filter_map(|v| parse(harness, v))
        .min()
        .expect("verified versions parse");
    let Some(version) = parse(harness, reported) else {
        return Err(format!(
            "{label} reported version \"{}\", which Crewroom has not verified for model discovery and cannot parse as a {label} release (failed check: {VERSION}; verified: {}). Install a verified or newer release version, then refresh.",
            printable(reported),
            list.join(", ")
        ));
    };
    if version <= lowest {
        return Err(format!(
            "{label} reported version \"{}\", which is older than the oldest version Crewroom has verified (failed check: {VERSION}; verified: {}). Update {label} to a verified or newer version, then refresh.",
            printable(reported),
            list.join(", ")
        ));
    }
    Ok(Basis::Probed)
}

/// A probe check failed for a newer, unlisted version.
pub fn probe_failure(harness: &Harness, reported: &str, check: &str, detail: &str) -> String {
    format!(
        "{} {} failed the Crewroom compatibility probe (failed check: {check}): {} Install a verified version ({}) or a release that passes the probe, then refresh.",
        harness.label(),
        printable(reported),
        detail.trim_end_matches('.').to_owned() + ".",
        verified(harness).join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_parse_per_harness_and_never_guess() {
        assert_eq!(
            parse(&Harness::Codex, "codex-cli 0.163.0"),
            Some([0, 163, 0])
        );
        assert_eq!(
            parse(&Harness::ClaudeCode, "2.1.303 (Claude Code)"),
            Some([2, 1, 303])
        );
        assert_eq!(parse(&Harness::Omp, "omp/19.0.0"), Some([19, 0, 0]));
        for (harness, bad) in [
            (Harness::Codex, "0.163.0"),
            (Harness::Codex, "codex-cli 0.164.0-alpha.1"),
            (Harness::ClaudeCode, "2.1 (Claude Code)"),
            (Harness::ClaudeCode, "2.1.303"),
            (Harness::Omp, "omp/18.11"),
            (Harness::Omp, "omp/18.11.x"),
            (Harness::Omp, "1.2.3"),
        ] {
            assert_eq!(parse(&harness, bad), None, "{bad}");
        }
    }
    #[test]
    fn listed_newer_older_and_unparseable_versions_are_classified_for_each_harness() {
        for (harness, newer, major, older, garbage) in [
            (
                Harness::Codex,
                "codex-cli 0.164.0",
                "codex-cli 1.0.0",
                "codex-cli 0.161.9",
                "codex 0.164",
            ),
            (
                Harness::ClaudeCode,
                "2.1.303 (Claude Code)",
                "3.0.0 (Claude Code)",
                "2.1.295 (Claude Code)",
                "Claude Code next",
            ),
            (
                Harness::Omp,
                "omp/18.11.4",
                "omp/19.0.0",
                "omp/18.4.11",
                "omp/nightly",
            ),
        ] {
            for listed in verified(&harness) {
                assert_eq!(classify(&harness, listed), Ok(Basis::Verified));
            }
            assert_eq!(classify(&harness, newer), Ok(Basis::Probed));
            assert_eq!(classify(&harness, major), Ok(Basis::Probed));
            let old = classify(&harness, older).unwrap_err();
            assert!(old.contains("older than the oldest version"), "{old}");
            assert!(old.contains("failed check: version"), "{old}");
            assert!(old.contains("then refresh"), "{old}");
            let bad = classify(&harness, garbage).unwrap_err();
            assert!(bad.contains("cannot parse"), "{bad}");
            assert!(bad.contains(harness.label()), "{bad}");
            assert!(bad.contains("failed check: version"), "{bad}");
        }
    }
    #[test]
    fn receipts_bind_version_and_executable_content() {
        let path = std::path::Path::new("/fixture/codex");
        let receipt = ProbeReceipt::new(&Harness::Codex, "codex-cli 1.0.0", path, "abc", 1);
        assert!(receipt.valid());
        assert!(!receipt.inference_requested);
        assert_eq!(
            receipt.checks,
            [
                "version",
                "model-discovery",
                "bundled-model-catalog",
                "protocol-schema",
                "control-protocol"
            ]
        );
        assert!(receipt.covers(&Harness::Codex, "codex-cli 1.0.0", path, "abc"));
        assert!(!receipt.covers(&Harness::Codex, "codex-cli 1.0.0", path, "changed"));
        assert!(!receipt.covers(&Harness::Codex, "codex-cli 1.0.1", path, "abc"));
        assert!(!receipt.covers(
            &Harness::Codex,
            "codex-cli 1.0.0",
            std::path::Path::new("/other/codex"),
            "abc"
        ));
        assert!(!receipt.covers(&Harness::Omp, "codex-cli 1.0.0", path, "abc"));
        let mut forged = receipt.clone();
        forged.inference_requested = true;
        assert!(!forged.valid());
        let mut forged = receipt;
        forged.checks.pop();
        assert!(!forged.valid());
        assert_eq!(
            Compatibility {
                basis: Basis::Probed,
                probe: None
            }
            .label(&Harness::ClaudeCode, "2.1.303 (Claude Code)"),
            "compatible (probed 2.1.303)"
        );
    }
}
