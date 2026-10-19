//! FOR-71 bundled known-broken harness versions. A reviewed list shipped with
//! Crewroom refuses a release even when its compatibility probe would pass,
//! for defects the probe cannot observe without inference. The list changes
//! only with a Crewroom revision: it is never fetched from the network,
//! configured by the operator, or reduced to a warning.
use super::*;

/// One refused, inclusive release range of a harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownBroken {
    pub harness: Harness,
    pub from: [u64; 3],
    pub through: [u64; 3],
    /// Operator-facing reason, ending with a full stop.
    pub reason: &'static str,
    /// Where the entry was reviewed (Linear issue or evidence section).
    pub reviewed: &'static str,
}

/// The shipped list. To add an entry, record the failing evidence (the
/// revision, version and observed defect) and review it like any code change;
/// a test keeps every verified baseline version off this list.
pub const LIST: &[KnownBroken] = &[];

/// The entry, if any, that refuses this exact release.
pub fn find<'a>(
    list: &'a [KnownBroken],
    harness: &Harness,
    version: [u64; 3],
) -> Option<&'a KnownBroken> {
    list.iter()
        .find(|e| &e.harness == harness && e.from <= version && version <= e.through)
}

/// The refusal message: harness, version, the recorded reason and a remedy.
pub fn refusal(entry: &KnownBroken, reported: &str) -> String {
    let [a, b, c] = entry.from;
    let [x, y, z] = entry.through;
    let range = if entry.from == entry.through {
        format!("{a}.{b}.{c}")
    } else {
        format!("{a}.{b}.{c} through {x}.{y}.{z}")
    };
    format!(
        "{} {} is on Crewroom's known-broken list ({range}; reviewed in {}) and is refused even when its compatibility probe passes (failed check: {}): {} Install another release, then refresh.",
        entry.harness.label(),
        reported
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect::<String>(),
        entry.reviewed,
        compatibility::KNOWN_BROKEN,
        entry.reason
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_verified_baseline_version_is_on_the_shipped_list() {
        for harness in [Harness::Codex, Harness::ClaudeCode, Harness::Omp] {
            for version in compatibility::verified(&harness) {
                let parsed = compatibility::parse(&harness, version).unwrap();
                assert_eq!(find(LIST, &harness, parsed), None, "{version}");
            }
        }
        for entry in LIST {
            assert!(entry.from <= entry.through);
            assert!(entry.reason.ends_with('.') && !entry.reviewed.is_empty());
        }
    }
    #[test]
    fn ranges_are_inclusive_and_per_harness() {
        let list = [KnownBroken {
            harness: Harness::Codex,
            from: [0, 170, 0],
            through: [0, 170, 2],
            reason: "Synthetic: tool results are dropped after compaction.",
            reviewed: "test fixture",
        }];
        for (version, refused) in [
            ([0, 169, 9], false),
            ([0, 170, 0], true),
            ([0, 170, 2], true),
            ([0, 170, 3], false),
        ] {
            assert_eq!(
                find(&list, &Harness::Codex, version).is_some(),
                refused,
                "{version:?}"
            );
        }
        assert_eq!(find(&list, &Harness::Omp, [0, 170, 1]), None);
        let message = refusal(&list[0], "codex-cli 0.170.1");
        assert!(message.contains("Codex codex-cli 0.170.1"), "{message}");
        assert!(message.contains("0.170.0 through 0.170.2"), "{message}");
        assert!(message.contains("failed check: known-broken"), "{message}");
        assert!(message.contains("tool results are dropped"), "{message}");
        assert!(message.contains("then refresh"), "{message}");
    }

    /// A listed version is refused with its recorded reason although the same
    /// executable passes the full probe when the list does not name it.
    #[tokio::test]
    async fn a_listed_version_is_refused_even_when_its_probe_passes() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("omp");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/python3
import json,sys,os
if '--version' in sys.argv:
 print('omp/19.0.0');sys.exit(0)
if sys.argv[1:]==['models','--json','--no-extensions']:
 print(json.dumps({'models':[{'provider':'fixture','id':'m','selector':'fixture/m','name':'Model','thinking':['low'],'input':['text']}]}));sys.exit(0)
open(os.path.join(os.path.dirname(os.path.realpath(__file__)),'handshakes'),'a').write('x\n')
print(json.dumps({'type':'ready','protocolVersion':1,'supportedProtocolVersions':[1,2]}),flush=True)
sessions=sys.argv[sys.argv.index('--session-dir')+1]
for line in sys.stdin:
 v=json.loads(line)
 data={'negotiate_protocol':{'protocolVersion':2},'get_state':{'sessionFile':os.path.join(sessions,'s.jsonl')},'set_host_tools':{'toolNames':[]}}[v['type']]
 print(json.dumps({'id':v['id'],'type':'response','command':v['type'],'success':True,'data':data}),flush=True)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let helpers =
            crate::test_binaries::ensure(&crate::test_binaries::profile_dir_of_current_test());
        let config = discovery::Config {
            harness: Harness::Omp,
            binary,
            helper: helpers.process,
            path: "/usr/bin:/bin".into(),
            omp_models: json!({"providers":{}}),
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let passed =
            discovery::discover_with_known_broken(&config, &root.path().join("pass"), &cancel, &[])
                .await
                .unwrap();
        assert!(compatibility::launchable(&Harness::Omp, &passed.identity));
        let handshakes = || {
            std::fs::read_to_string(root.path().join("handshakes"))
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        assert_eq!(handshakes(), 1, "the probe passes for this executable");
        let list = [KnownBroken {
            harness: Harness::Omp,
            from: [19, 0, 0],
            through: [19, 0, 0],
            reason: "Synthetic: session files are truncated on resume.",
            reviewed: "test fixture",
        }];
        let refused = discovery::discover_with_known_broken(
            &config,
            &root.path().join("refused"),
            &cancel,
            &list,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(refused.contains("OMP omp/19.0.0"), "{refused}");
        assert!(refused.contains("known-broken list"), "{refused}");
        assert!(refused.contains("session files are truncated"), "{refused}");
        assert_eq!(handshakes(), 1, "refused before any probe process");
    }
}
