//! FOR-69: newer unlisted harness versions run after a metadata-only
//! compatibility probe. Simulated harnesses below speak each launch protocol;
//! the opt-in live test probes the CLIs installed on this machine.
//! FOR-71: the Codex fixture also describes its protocol from synthetic
//! schemas, and the Claude Code fixture records its isolation environment.
use crewroom_runtime::model_catalog::{
    Harness, codex_protocol,
    compatibility::{Basis, ProbeReceipt},
    discovery::{Config, Receipt, discover_with, probe_installed},
};
use crewroom_test_support::install_executable;
use serde_json::json;
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

/// A Python harness reporting `version`. Every launch-protocol handshake
/// appends one line to `handshakes` beside the executable.
fn fixture(root: &Path, harness: &Harness, version: &str, fail: bool) -> Config {
    let binary = root.join("harness.py");
    let body = match harness {
        Harness::Codex => {
            r#"
args=sys.argv[1:]
if args[:3]==['debug','models','--bundled']:
 print(json.dumps({'models':[{'slug':'fixture-model','supported_reasoning_levels':[{'effort':'low'}]}]}));sys.exit(0)
if args[:2]==['app-server','generate-json-schema']:
 schema=json.load(open(os.path.join(HERE,'protocol.json')))
 if schema is None: print('unrecognized subcommand',file=sys.stderr);sys.exit(2)
 assert '--experimental' in args
 out=args[args.index('--out')+1]
 for name,methods in [('ServerRequest.json',schema['requests']),('ServerNotification.json',schema['notifications'])]:
  json.dump({'oneOf':[{'properties':{'method':{'enum':[m]}}} for m in methods]},open(os.path.join(out,name),'w'))
 sys.exit(0)
if '--strict-config' in args:
 cfg=open(os.path.join(os.environ['CODEX_HOME'],'config.toml')).read()
 assert 'model = "fixture-model"' in cfg and 'plugins = false' in cfg and 'model_catalog_json' in cfg
 mark()
 if FAIL: print('unknown configuration field',file=sys.stderr);sys.exit(1)
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 assert v['method'] in ['initialize','model/list']
 result={'data':[{'model':'fixture-model','displayName':'Fixture','inputModalities':['text'],'supportedReasoningEfforts':[{'reasoningEffort':'low'}]}],'nextCursor':None} if v['method']=='model/list' else {}
 print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#
        }
        Harness::ClaudeCode => {
            r#"
v=json.loads(sys.stdin.readline())
assert v['request']['subtype']=='initialize'
assert os.environ['ENABLE_CLAUDEAI_MCP_SERVERS']=='false' and os.environ['CLAUDE_CODE_AUTO_CONNECT_IDE']=='0' and os.environ['CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL']=='1'
ok=True
if '--safe-mode' in sys.argv:
 assert '--bare' not in sys.argv and sys.argv[sys.argv.index('--permission-mode')+1]=='dontAsk'
 assert v['request']['sdkMcpServers']==['crewroom']
 mark(); ok=not FAIL
print(json.dumps({'type':'control_request','request_id':'mcp','request':{'subtype':'mcp_message','server_name':'crewroom'}}),flush=True)
print(json.dumps({'type':'control_response','response':{'request_id':v['request_id'],'subtype':'success' if ok else 'error','response':{'models':[{'value':'default','displayName':'Default','resolvedModel':'observed','supportedEffortLevels':['high']}]}}}),flush=True)
time.sleep(30)
"#
        }
        Harness::Omp => {
            r#"
if sys.argv[1:]==['models','--json','--no-extensions'] and os.path.exists(os.path.join(os.path.dirname(os.path.realpath(__file__)),'no-providers')):
 print(json.dumps({'models':[]}));sys.exit(0)
if sys.argv[1:]==['models','--json','--no-extensions']:
 print(json.dumps({'models':[{'provider':'fixture','id':'m','selector':'fixture/m','name':'Model','thinking':['low'],'input':['text']}]}));sys.exit(0)
assert sys.argv[1:3]==['--mode','rpc'] and '--no-tools' in sys.argv
assert sys.argv[sys.argv.index('--model')+1]=='crewroom-probe/probe'
assert 'crewroom-probe' in open(os.path.join(os.environ['PI_CODING_AGENT_DIR'],'models.yml')).read()
mark()
print(json.dumps({'type':'ready','protocolVersion':1,'supportedProtocolVersions':[1] if FAIL else [1,2]}),flush=True)
sessions=sys.argv[sys.argv.index('--session-dir')+1]
for line in sys.stdin:
 v=json.loads(line)
 data={'negotiate_protocol':{'protocolVersion':2},'get_state':{'sessionFile':os.path.join(sessions,'s.jsonl')},'set_host_tools':{'toolNames':[]}}[v['type']]
 print(json.dumps({'id':v['id'],'type':'response','command':v['type'],'success':True,'data':data}),flush=True)
"#
        }
    };
    install_executable(
        &binary,
        format!(
            "#!/usr/bin/python3\nimport json,sys,os,time\nFAIL={}\nHERE=os.path.dirname(os.path.realpath(__file__))\ndef mark():\n open(os.path.join(HERE,'handshakes'),'a').write('x\\n')\nif '--version' in sys.argv:\n print({version:?});sys.exit(0)\n{body}\n",
            if fail { "True" } else { "False" }
        ),
    );
    if !root.join("protocol.json").exists() {
        protocol(root, Some((&[], &[])));
    }
    Config {
        harness: harness.clone(),
        binary,
        helper: env!("CARGO_BIN_EXE_crewroom-process").into(),
        path: "/usr/bin:/bin".into(),
        omp_models: json!({"providers":{}}),
    }
}
/// The synthetic protocol the Codex fixture describes: the reviewed baseline
/// plus `extra` server requests and notifications, or (`None`) no schema
/// subcommand at all.
fn protocol(root: &Path, extra: Option<(&[&str], &[&str])>) {
    let value = extra.map(|(requests, notifications)| {
        json!({
            "requests": codex_protocol::BASELINE_SERVER_REQUESTS.iter().chain(requests).collect::<Vec<_>>(),
            "notifications": codex_protocol::BASELINE_NOTIFICATIONS.iter().chain(notifications).collect::<Vec<_>>(),
        })
    });
    std::fs::write(
        root.join("protocol.json"),
        value.unwrap_or(json!(null)).to_string(),
    )
    .unwrap();
}
fn handshakes(root: &Path) -> usize {
    std::fs::read_to_string(root.join("handshakes"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}
async fn run(
    c: &Config,
    root: &Path,
    name: &str,
    probes: &[ProbeReceipt],
) -> Result<Receipt, String> {
    discover_with(
        c,
        &root.join(name),
        100,
        &CancellationToken::new(),
        Duration::from_secs(30),
        probes,
    )
    .await
    .map_err(|e| e.to_string())
}

const NEWER: [(Harness, &str, &str, &str); 3] = [
    (
        Harness::Codex,
        "codex-cli 0.164.0",
        "codex-cli 1.0.0",
        "1.0.0",
    ),
    (
        Harness::ClaudeCode,
        "2.1.303 (Claude Code)",
        "3.0.0 (Claude Code)",
        "3.0.0",
    ),
    (Harness::Omp, "omp/18.11.4", "omp/19.0.0", "19.0.0"),
];

#[tokio::test]
async fn newer_and_new_major_versions_pass_the_probe_for_each_harness() {
    for (harness, newer, major, number) in NEWER {
        for version in [newer, major] {
            let root = tempfile::tempdir().unwrap();
            let c = fixture(root.path(), &harness, version, false);
            let receipt = run(&c, root.path(), "discovery", &[])
                .await
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            assert_eq!(handshakes(root.path()), 1, "{version} ran its handshake");
            assert!(!receipt.inference_requested && receipt.cleanup_verified);
            assert!(!receipt.offerings.is_empty());
            let compatibility = receipt.identity.compatibility.clone().unwrap();
            assert_eq!(compatibility.basis, Basis::Probed);
            let probe = compatibility.probe.clone().unwrap();
            assert!(probe.covers(
                &harness,
                version,
                &receipt.identity.canonical_binary,
                &receipt.identity.binary_sha256
            ));
            assert!(!probe.inference_requested);
            assert!(crewroom_runtime::model_catalog::compatibility::launchable(
                &harness,
                &receipt.identity
            ));
            if version == major {
                assert_eq!(
                    compatibility.label(&harness, version),
                    format!("compatible (probed {number})")
                );
            }
        }
    }
}

#[tokio::test]
async fn verified_versions_skip_the_probe() {
    for (harness, version) in [
        (Harness::Codex, "codex-cli 0.163.0"),
        (Harness::ClaudeCode, "2.1.302 (Claude Code)"),
        (Harness::Omp, "omp/18.11.3"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let c = fixture(root.path(), &harness, version, true);
        let receipt = run(&c, root.path(), "discovery", &[]).await.unwrap();
        assert_eq!(handshakes(root.path()), 0);
        let compatibility = receipt.identity.compatibility.unwrap();
        assert_eq!(compatibility.basis, Basis::Verified);
        assert_eq!(compatibility.probe, None);
    }
}

#[tokio::test]
async fn older_unparseable_and_probe_failing_versions_are_refused_with_the_check() {
    for ((harness, newer, _, _), (older, garbage, check)) in NEWER.into_iter().zip([
        (
            "codex-cli 0.161.0",
            "codex-cli 0.164.0-beta",
            "control-protocol",
        ),
        ("2.1.200 (Claude Code)", "2.1.303", "stream-protocol"),
        ("omp/18.4.11", "omp 19", "rpc-frame-contract"),
    ]) {
        let root = tempfile::tempdir().unwrap();
        for (name, version, expected) in [
            ("older", older, "older than the oldest version"),
            ("garbage", garbage, "cannot parse"),
        ] {
            let c = fixture(root.path(), &harness, version, false);
            let error = run(&c, root.path(), name, &[]).await.unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("failed check: version"), "{error}");
            assert!(error.contains(harness.label()), "{error}");
            assert!(error.contains("then refresh"), "{error}");
        }
        assert_eq!(handshakes(root.path()), 0, "refused before any handshake");
        let c = fixture(root.path(), &harness, newer, true);
        let error = run(&c, root.path(), "failing", &[]).await.unwrap_err();
        assert!(error.contains(&format!("failed check: {check}")), "{error}");
        assert!(error.contains(newer), "{error}");
        assert!(error.contains("compatibility probe"), "{error}");
        assert!(error.contains("then refresh"), "{error}");
        assert_eq!(handshakes(root.path()), 1);
    }
}

#[tokio::test]
async fn retained_receipts_skip_the_handshake_until_the_executable_changes() {
    for (harness, newer, _, _) in NEWER {
        let root = tempfile::tempdir().unwrap();
        let c = fixture(root.path(), &harness, newer, false);
        let first = run(&c, root.path(), "first", &[]).await.unwrap();
        let probe = first.identity.compatibility.unwrap().probe.unwrap();
        let again = run(&c, root.path(), "again", std::slice::from_ref(&probe))
            .await
            .unwrap();
        assert_eq!(handshakes(root.path()), 1, "retained receipt reused");
        assert_eq!(again.identity.compatibility.unwrap().probe.unwrap(), probe);
        // Same version, different content: a new executable identity.
        let mut content = std::fs::read(&c.binary).unwrap();
        content.extend_from_slice(b"# rebuilt\n");
        std::fs::write(&c.binary, content).unwrap();
        let changed = run(&c, root.path(), "changed", std::slice::from_ref(&probe))
            .await
            .unwrap();
        assert_eq!(handshakes(root.path()), 2, "changed binary re-probed");
        let reprobed = changed.identity.compatibility.unwrap().probe.unwrap();
        assert_ne!(reprobed.binary_sha256, probe.binary_sha256);
        assert_ne!(reprobed.id, probe.id);
    }
}

/// OMP lists models only from configured providers. A newer OMP on a runner
/// without provider configuration passes the probe with zero offerings; the
/// refusal is never blamed on a probe failure.
#[tokio::test]
async fn newer_omp_without_configured_providers_is_compatible_with_no_offerings() {
    use crewroom_runtime::model_catalog::store::{Catalog, Source};
    let root = tempfile::tempdir().unwrap();
    let c = fixture(root.path(), &Harness::Omp, "omp/18.11.4", false);
    std::fs::write(root.path().join("no-providers"), "").unwrap();
    let receipt = run(&c, root.path(), "discovery", &[]).await.unwrap();
    assert!(receipt.offerings.is_empty());
    assert_eq!(handshakes(root.path()), 1, "the handshake still ran");
    let compatibility = receipt.identity.compatibility.clone().unwrap();
    assert_eq!(compatibility.basis, Basis::Probed);
    assert_eq!(
        compatibility.label(&Harness::Omp, "omp/18.11.4"),
        "compatible (probed 18.11.4)"
    );
    assert!(crewroom_runtime::model_catalog::compatibility::launchable(
        &Harness::Omp,
        &receipt.identity
    ));
    // Through the catalog: labelled compatible, no error, zero offerings.
    let catalog = Catalog::open(
        &root.path().join("catalog"),
        vec![Source {
            config: c.clone(),
            configuration_revision: "one".into(),
        }],
        || {},
    )
    .unwrap();
    catalog.refresh().unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        while catalog.snapshot().unwrap().state == "refreshing" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let entry = &snapshot.harnesses[0];
    assert_eq!(
        entry.compatibility().as_deref(),
        Some("compatible (probed 18.11.4)")
    );
    assert!(entry.offerings.is_empty());
    assert!(entry.error.is_none(), "{:?}", entry.error);
    catalog.close().await.unwrap();
}

#[tokio::test]
async fn refresh_and_launch_reprobe_retain_receipts_and_report_failures() {
    use crewroom_runtime::model_catalog::store::{Catalog, Source};
    let root = tempfile::tempdir().unwrap();
    let c = fixture(root.path(), &Harness::Omp, "omp/19.0.0", false);
    let catalog = Catalog::open(
        &root.path().join("catalog"),
        vec![Source {
            config: c.clone(),
            configuration_revision: "one".into(),
        }],
        || {},
    )
    .unwrap();
    catalog.refresh().unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        while catalog.snapshot().unwrap().state == "refreshing" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    assert_eq!(
        snapshot.harnesses[0].compatibility().as_deref(),
        Some("compatible (probed 19.0.0)")
    );
    assert_eq!(snapshot.harnesses[0].execution_note(), None);
    assert_eq!(snapshot.probes.len(), 1);
    assert_eq!(handshakes(root.path()), 1);
    // Launch-time re-probe of the unchanged executable reuses the receipt.
    let identity = catalog.probe_now(&Harness::Omp).await.unwrap().identity;
    assert_eq!(
        identity.compatibility.unwrap().probe.unwrap(),
        snapshot.probes[0]
    );
    assert_eq!(handshakes(root.path()), 1);
    // A replaced executable that fails its handshake is reported with the
    // failed check; no receipt is retained for it.
    fixture(root.path(), &Harness::Omp, "omp/19.0.1", true);
    let error = catalog.probe_now(&Harness::Omp).await.unwrap_err();
    assert!(
        error.contains("failed check: rpc-frame-contract"),
        "{error}"
    );
    assert!(error.contains("omp/19.0.1"), "{error}");
    assert_eq!(handshakes(root.path()), 2);
    assert_eq!(catalog.snapshot().unwrap().probes, snapshot.probes);
    catalog.close().await.unwrap();
    // Receipts survive a service restart.
    drop(catalog);
    let reopened = Catalog::open(&root.path().join("catalog"), vec![], || {}).unwrap();
    assert_eq!(reopened.snapshot().unwrap().probes, snapshot.probes);
}

/// Opt-in compatibility probe of the CLIs installed on this machine, with
/// isolated HOME and configuration. No prompt is sent, so no inference runs.
/// Each CLI is probed as installed, then behind a wrapper that reports a new
/// major version and delegates every other invocation to the real binary.
/// Set CREWROOM_LIVE_DISCOVERY=1 and keep codex, claude and omp on PATH.
#[tokio::test]
#[ignore = "Requires CREWROOM_LIVE_DISCOVERY=1 and installed codex, claude and omp"]
async fn installed_harnesses_pass_the_compatibility_probe() {
    assert_eq!(
        std::env::var("CREWROOM_LIVE_DISCOVERY").as_deref(),
        Ok("1"),
        "explicit opt-in required"
    );
    let path = std::env::var("PATH").unwrap();
    let root = tempfile::tempdir().unwrap();
    for (harness, name, major) in [
        (Harness::Codex, "codex", "codex-cli 99.0.0"),
        (Harness::ClaudeCode, "claude", "99.0.0 (Claude Code)"),
        (Harness::Omp, "omp", "omp/99.0.0"),
    ] {
        let binary = std::env::split_paths(&path)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("{name} is not on PATH"))
            .canonicalize()
            .unwrap();
        let installed = Config {
            harness: harness.clone(),
            binary: binary.clone(),
            helper: env!("CARGO_BIN_EXE_crewroom-process").into(),
            path: path.clone(),
            omp_models: json!({"providers":{"crewroom-fixture":{"api":"openai-responses","apiKey":"synthetic-offline-key",
                "baseUrl":"http://127.0.0.1:9/v1","models":[{"id":"synthetic-model","name":"Synthetic model",
                "reasoning":true,"input":["text"],"contextWindow":128000,"maxTokens":8192}]}}}),
        };
        let receipt = probe_installed(
            &installed,
            &root.path().join(name),
            100,
            &CancellationToken::new(),
            Duration::from_secs(90),
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let probe = receipt.identity.compatibility.as_ref().unwrap();
        println!(
            "LIVE-PROBE {} version={:?} basis={:?} contract={} checks={:?} protocol={:?} offerings={} inference={}",
            harness.as_str(),
            receipt.identity.version,
            probe.basis,
            probe.probe.as_ref().unwrap().contract,
            probe.probe.as_ref().unwrap().checks,
            probe.probe.as_ref().unwrap().protocol,
            receipt.offerings.len(),
            receipt.inference_requested
        );
        if harness == Harness::Codex {
            assert!(probe.probe.as_ref().unwrap().protocol.is_some());
        }
        assert!(receipt.cleanup_verified && !receipt.inference_requested);
        let wrapper = root.path().join(format!("{name}-major"));
        install_executable(
            &wrapper,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{major}'; exit 0; fi\nexec '{}' \"$@\"\n",
                binary.display()
            ),
        );
        let wrapped = Config {
            binary: wrapper,
            ..installed
        };
        let receipt = discover_with(
            &wrapped,
            &root.path().join(format!("{name}-major-root")),
            100,
            &CancellationToken::new(),
            Duration::from_secs(90),
            &[],
        )
        .await
        .unwrap_or_else(|e| panic!("{name} as {major}: {e}"));
        let compatibility = receipt.identity.compatibility.unwrap();
        assert_eq!(compatibility.basis, Basis::Probed);
        println!(
            "LIVE-PROBE {} wrapped-version={:?} label={:?}",
            harness.as_str(),
            major,
            compatibility.label(&harness, major)
        );
    }
}

/// FOR-71: the Codex probe diffs the protocol Codex describes offline with
/// the reviewed baseline. An unhandled server request fails, naming it; a
/// missing schema subcommand fails closed; new notifications are recorded.
#[tokio::test]
async fn codex_protocol_diff_names_unhandled_requests_and_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let c = fixture(root.path(), &Harness::Codex, "codex-cli 0.164.0", false);
    protocol(root.path(), Some((&[], &["thread/memory/updated"])));
    let receipt = run(&c, root.path(), "baseline", &[]).await.unwrap();
    let probe = receipt.identity.compatibility.unwrap().probe.unwrap();
    assert!(probe.checks.contains(&"protocol-schema".to_owned()));
    let diff = probe.protocol.clone().unwrap();
    assert_eq!(diff.added_notifications, ["thread/memory/updated"]);
    assert!(diff.removed_notifications.is_empty());
    assert!(probe.valid());
    assert_eq!(handshakes(root.path()), 1);

    protocol(
        root.path(),
        Some((&["item/browser/requestNavigation"], &[])),
    );
    let error = run(&c, root.path(), "unhandled", &[]).await.unwrap_err();
    assert!(error.contains("failed check: protocol-schema"), "{error}");
    assert!(
        error.contains("does not handle: item/browser/requestNavigation"),
        "{error}"
    );
    assert!(error.contains("codex-cli 0.164.0"), "{error}");
    assert_eq!(
        handshakes(root.path()),
        1,
        "refused before the app-server ran"
    );

    protocol(root.path(), None);
    let error = run(&c, root.path(), "missing", &[]).await.unwrap_err();
    assert!(error.contains("failed check: protocol-schema"), "{error}");
    assert!(
        error.contains("generate-json-schema is unavailable or failed"),
        "{error}"
    );
    assert_eq!(handshakes(root.path()), 1);
    // A retained receipt from the passing run does not cover a v1 contract.
    let mut stale = probe.clone();
    stale.contract = "harness-compatibility-probe-v1".into();
    assert!(!stale.valid());
}
