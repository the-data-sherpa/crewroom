use crewroom_runtime::model_catalog::store::{Catalog, Source};
use crewroom_runtime::model_catalog::{
    Harness,
    discovery::{Config, discover},
};
use crewroom_test_support::install_executable;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

fn config(root: &Path, harness: Harness, body: &str) -> Config {
    let binary = root.join("harness.py");
    let version = match harness {
        Harness::Codex => "codex-cli 0.162.0",
        Harness::ClaudeCode => "2.1.296 (Claude Code)",
        Harness::Omp => "omp/18.4.12",
    };
    install_executable(
        &binary,
        format!(
            "#!/usr/bin/python3\nimport json,sys,os,time\nif '--version' in sys.argv:\n print({version:?});sys.exit(0)\n{body}\n"
        ),
    );
    Config {
        harness,
        binary,
        helper: env!("CARGO_BIN_EXE_crewroom-process").into(),
        path: "/usr/bin:/bin".into(),
        omp_models: json!({"providers":{}}),
    }
}

#[tokio::test]
async fn codex_pages_are_bounded_metadata_only_and_unknown_fields_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let c = config(
        root.path(),
        Harness::Codex,
        r#"
assert not os.path.exists(os.path.join(os.environ['HOME'],'.codex','auth.json'))
for line in sys.stdin:
 v=json.loads(line)
 assert v['method'] in ['initialize','initialized','model/list']
 if v['method']=='initialized': continue
 result={}
 if v['method']=='model/list':
  next=v['params']['cursor'] is None
  result={'data':[{'model':'one' if next else 'two','displayName':'Duplicate label','inputModalities':['text'],'supportedReasoningEfforts':[{'reasoningEffort':'high'}],'apiKey':'secret-canary'}],'nextCursor':'next' if next else None}
 print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#,
    );
    let receipt = discover(
        &c,
        &root.path().join("discovery"),
        100,
        &CancellationToken::new(),
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(receipt.offerings.len(), 2);
    assert!(receipt.cleanup_verified);
    assert!(!receipt.inference_requested);
    assert_eq!(receipt.offerings[0].capabilities.tools, None);
    assert!(
        !serde_json::to_string(&receipt)
            .unwrap()
            .contains("secret-canary")
    );
}

#[tokio::test]
async fn claude_aliases_and_omp_providers_stay_distinct() {
    for harness in [Harness::ClaudeCode, Harness::Omp] {
        let root = tempfile::tempdir().unwrap();
        let body = if harness == Harness::ClaudeCode {
            r#"
assert '--bare' in sys.argv and '--no-session-persistence' in sys.argv
assert sys.argv[sys.argv.index('--tools')+1]==''
v=json.loads(sys.stdin.readline()); assert v['request']['subtype']=='initialize'
print(json.dumps({'type':'control_response','response':{'request_id':v['request_id'],'subtype':'success','response':{'models':[{'value':a,'displayName':'Opus','resolvedModel':'observed','supportedEffortLevels':['high']} for a in ['default','opus']],'account':'secret-canary'}}}),flush=True)
time.sleep(30)
"#
        } else {
            r#"
assert sys.argv[1:]==['models','--json','--no-extensions']
print(json.dumps({'models':[{'provider':p,'id':'same','selector':p+'/same','name':'Same label','thinking':None,'input':['text'],'headers':{'Authorization':'secret-canary'}} for p in ['a','b']]}))
"#
        };
        let c = config(root.path(), harness, body);
        let receipt = discover(
            &c,
            &root.path().join("discovery"),
            100,
            &CancellationToken::new(),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(receipt.offerings.len(), 2);
        assert_ne!(receipt.offerings[0].key, receipt.offerings[1].key);
        assert!(
            !serde_json::to_string(&receipt)
                .unwrap()
                .contains("secret-canary")
        );
    }
}

#[tokio::test]
async fn failures_are_sanitized_and_children_are_reaped() {
    // Only the hung harness needs a short bound. The others fail on their own;
    // their long bound keeps a loaded machine from turning them into timeouts
    // before the metadata helper has published its owner receipt.
    for (body, bound) in [
        ("time.sleep(30)", 5),
        ("print('x'*(9*1024*1024),flush=True)", 20),
        ("print('not json')", 20),
        ("print(json.dumps({'models':[]}));sys.exit(1)", 20),
    ] {
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path(), Harness::Omp, body);
        let dir = root.path().join("discovery");
        let error = discover(
            &c,
            &dir,
            100,
            &CancellationToken::new(),
            Duration::from_secs(bound),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("secret-canary"));
        let owner: Value =
            serde_json::from_slice(&std::fs::read(dir.join("metadata/owner.json")).unwrap())
                .unwrap();
        assert_eq!(
            unsafe { libc::kill(owner["pid"].as_i64().unwrap() as i32, 0) },
            -1
        );
        assert_eq!(
            crewroom_runtime::inspect_settled(&dir.join("metadata"))
                .await
                .state,
            "stopped"
        );
    }
}

#[tokio::test]
async fn cancellation_unavailable_version_drift_and_cursor_loops_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let c = config(
        root.path(),
        Harness::Codex,
        r#"
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 result={} if v['method']=='initialize' else {'data':[],'nextCursor':'repeat'}
 print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#,
    );
    // The bound only guards a hung fixture; two Python starts can exceed a
    // few seconds on a loaded machine before the repeated cursor arrives.
    let error = discover(
        &c,
        &root.path().join("loop"),
        100,
        &CancellationToken::new(),
        Duration::from_secs(20),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("cursor"), "{error}");
    let c = config(root.path(), Harness::Omp, "time.sleep(30)");
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });
    // The cancellation can land before either helper starts, before it
    // publishes its owner receipt or during the exchange. Each must report
    // cancellation; the long bound keeps the timeout from winning on a loaded
    // machine. The deterministic cases are covered separately below.
    let cancelled = discover(
        &c,
        &root.path().join("cancel"),
        100,
        &token,
        Duration::from_secs(20),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(cancelled.contains("cancelled"), "{cancelled}");
    let mut missing = c.clone();
    missing.binary = root.path().join("missing");
    assert!(
        discover(
            &missing,
            &root.path().join("missing-root"),
            100,
            &token,
            Duration::from_secs(3)
        )
        .await
        .is_err()
    );
    std::fs::write(&c.binary, "#!/bin/sh\necho 1.2.3\n").unwrap();
    let drift = discover(
        &c,
        &root.path().join("version"),
        100,
        &CancellationToken::new(),
        Duration::from_secs(3),
    )
    .await
    .unwrap_err()
    .to_string();
    // The operator sees which harness drifted, what it reported, what is
    // verified and how to recover.
    assert!(drift.contains("OMP reported version \"1.2.3\""), "{drift}");
    assert!(drift.contains("omp/18.4.12"), "{drift}");
    assert!(drift.contains("then refresh"), "{drift}");
}

#[tokio::test]
async fn cancellation_before_start_launches_nothing() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path(), Harness::Omp, "time.sleep(30)");
    let token = CancellationToken::new();
    token.cancel();
    let dir = root.path().join("discovery");
    let error = discover(&c, &dir, 100, &token, Duration::from_secs(20))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!dir.exists());
}

#[tokio::test]
async fn cancellation_after_the_harness_starts_stops_its_group_and_reports_cancelled() {
    let root = tempfile::tempdir().unwrap();
    // The harness signals readiness only after the helper published its owner
    // receipt and exec'd it, so this cancel always takes the owned-stop path.
    let c = config(
        root.path(),
        Harness::Omp,
        "open(os.path.join(os.environ['HOME'],'ready'),'w').close()\ntime.sleep(30)",
    );
    let dir = root.path().join("discovery");
    let token = CancellationToken::new();
    let task = tokio::spawn({
        let (c, dir, token) = (c.clone(), dir.clone(), token.clone());
        async move { discover(&c, &dir, 100, &token, Duration::from_secs(60)).await }
    });
    let ready = dir.join("home/ready");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !ready.exists() {
        if task.is_finished() {
            panic!("discovery ended first: {:?}", task.await.unwrap().err());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "harness never started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    token.cancel();
    let error = task.await.unwrap().unwrap_err().to_string();
    assert!(error.contains("cancelled"), "{error}");
    let metadata = dir.join("metadata");
    let owner: Value =
        serde_json::from_slice(&std::fs::read(metadata.join("owner.json")).unwrap()).unwrap();
    // No member of the harness's process group survives.
    assert_eq!(
        unsafe { libc::kill(-(owner["pgid"].as_i64().unwrap() as i32), 0) },
        -1
    );
    assert!(!metadata.join("owner.json.partial").exists());
    assert_eq!(
        crewroom_runtime::inspect_settled(&metadata).await.state,
        "stopped"
    );
}

/// Opt-in metadata exchange with the CLIs installed on this machine. It runs
/// the production discovery path with an isolated HOME and requests no
/// inference. Set CREWROOM_LIVE_DISCOVERY=1 and keep the harnesses on PATH.
#[tokio::test]
#[ignore = "Requires CREWROOM_LIVE_DISCOVERY=1 and installed codex, claude and omp"]
async fn installed_harnesses_list_models_without_inference() {
    assert_eq!(
        std::env::var("CREWROOM_LIVE_DISCOVERY").as_deref(),
        Ok("1"),
        "explicit opt-in required"
    );
    let path = std::env::var("PATH").unwrap();
    let root = tempfile::tempdir().unwrap();
    for (harness, name) in [
        (Harness::Codex, "codex"),
        (Harness::ClaudeCode, "claude"),
        (Harness::Omp, "omp"),
    ] {
        let binary = std::env::split_paths(&path)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("{name} is not on PATH"));
        let c = Config {
            harness: harness.clone(),
            binary,
            helper: env!("CARGO_BIN_EXE_crewroom-process").into(),
            path: path.clone(),
            // OMP lists configured providers only. A synthetic loopback
            // provider proves the exchange; it is never contacted.
            omp_models: json!({"providers":{"crewroom-fixture":{"api":"openai-responses","apiKey":"synthetic-offline-key",
                "baseUrl":"http://127.0.0.1:9/v1","models":[{"id":"synthetic-model","name":"Synthetic model",
                "reasoning":true,"input":["text"],"contextWindow":128000,"maxTokens":8192}]}}}),
        };
        let receipt = discover(
            &c,
            &root.path().join(name),
            100,
            &CancellationToken::new(),
            Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(receipt.cleanup_verified && !receipt.inference_requested);
        assert!(!receipt.offerings.is_empty(), "{name} listed no models");
        println!(
            "LIVE {} version={:?} offerings={} sample={:?}",
            harness.as_str(),
            receipt.identity.version,
            receipt.offerings.len(),
            receipt
                .offerings
                .iter()
                .take(3)
                .map(|o| o.key.as_str())
                .collect::<Vec<_>>()
        );
        // FOR-42: billing route from the logins in this machine's HOME (Codex,
        // Claude Code) and the runner provider definition (OMP). Prints kinds
        // and file names only, never credential values.
        let sources = crewroom_runtime::model_catalog::billing::Sources {
            omp_providers: c.omp_models["providers"].clone(),
            ..crewroom_runtime::model_catalog::billing::Sources::native(&json!({
                "HOME": std::env::var("HOME").ok(),
                "CODEX_HOME": std::env::var("CODEX_HOME").ok(),
            }))
        };
        let detection = sources.detect(&harness, &receipt.offerings[0].provider);
        println!(
            "LIVE {} billing={:?} basis={:?}",
            harness.as_str(),
            detection
                .routes
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>(),
            detection.basis
        );
    }
}

#[tokio::test]
async fn durable_refresh_preserves_partial_results_and_detects_configuration_and_binary_drift() {
    let root = tempfile::tempdir().unwrap();
    let config = config(
        root.path(),
        Harness::Omp,
        r#"print(json.dumps({'models':[{'provider':'fixture','id':'m','selector':'fixture/m','name':'Model','thinking':['high'],'input':['text']}]}))"#,
    );
    let source = Source {
        config: config.clone(),
        configuration_revision: "one".into(),
    };
    let catalog = Catalog::open(&root.path().join("catalog"), vec![source.clone()], || {}).unwrap();
    let first = catalog.refresh().unwrap();
    let repeated = catalog.refresh().unwrap();
    assert_eq!(first["generation"], repeated["generation"]);
    tokio::time::timeout(Duration::from_secs(5), async {
        while catalog.snapshot().unwrap().state == "refreshing" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(catalog.snapshot().unwrap().state, "partial");
    assert_eq!(
        catalog.view(crewroom_runtime::model_catalog::store::now())["harnesses"][0]["fresh"],
        true
    );
    let offerings = catalog.snapshot().unwrap().harnesses[0].offerings.clone();
    catalog.close().await.unwrap();
    drop(catalog);
    let mut changed = source;
    changed.configuration_revision = "two".into();
    let catalog = Catalog::open(&root.path().join("catalog"), vec![changed], || {}).unwrap();
    assert_eq!(
        catalog.snapshot().unwrap().harnesses[0].offerings,
        offerings
    );
    assert_eq!(
        catalog.view(crewroom_runtime::model_catalog::store::now())["harnesses"][0]["fresh"],
        false
    );
    std::fs::write(&config.binary, "#!/bin/sh\necho wrong-version\n").unwrap();
    catalog.refresh().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while catalog.snapshot().unwrap().state == "refreshing" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(catalog.snapshot().unwrap().state, "error");
    assert_eq!(
        catalog.snapshot().unwrap().harnesses[0].offerings,
        offerings
    );
    assert_eq!(
        catalog.view(crewroom_runtime::model_catalog::store::now())["harnesses"][0]["fresh"],
        false
    );
    catalog.close().await.unwrap();
}
