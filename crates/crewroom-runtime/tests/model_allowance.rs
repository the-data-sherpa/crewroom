//! FOR-74: allowance reads against simulated harnesses. Each fake asserts it
//! receives only control frames (no prompt), and the reading carries no
//! credential, account or balance value.
use crewroom_core::authority::write_private;
use crewroom_runtime::model_catalog::{
    Harness,
    allowance::{Allowance, State, Target, read},
    billing::Sources,
    discovery::Config,
};
use crewroom_test_support::install_executable;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

const CANARY: &str = "allowance-secret-canary";

fn target(root: &Path, harness: Harness, body: &str) -> Target {
    let binary = root.join("harness.py");
    install_executable(
        &binary,
        format!("#!/usr/bin/python3\nimport json,sys,os,time\n{body}\n"),
    );
    let auth = root.join("codex/auth.json");
    std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
    write_private(
        &auth,
        json!({"auth_mode":"chatgpt","tokens":{"access_token":CANARY,"account_id":CANARY}})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    let home = root.join("claude-home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    write_private(
        &home.join(".claude/.credentials.json"),
        json!({"claudeAiOauth":{"accessToken":CANARY,"scopes":["user:inference"],"subscriptionType":"max"}})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    Target {
        config: Config {
            harness,
            binary,
            helper: env!("CARGO_BIN_EXE_crewroom-process").into(),
            path: "/usr/bin:/bin".into(),
            omp_models: json!({}),
        },
        sources: Sources {
            codex_auth: Some(auth),
            claude_home: Some(home),
            omp_providers: json!({}),
        },
    }
}

const CODEX: &str = r#"
home=os.environ['CODEX_HOME']
assert os.path.islink(os.path.join(home,'auth.json'))
for line in sys.stdin:
 v=json.loads(line)
 assert v['method'] in ['initialize','initialized','account/rateLimits/read'], v
 if v['method']=='initialized': continue
 if v['method']=='initialize': print(json.dumps({'id':v['id'],'result':{}}),flush=True); continue
 print(json.dumps({'id':v['id'],'result':RESULT}),flush=True)
"#;

const CLAUDE: &str = r#"
assert '--no-session-persistence' in sys.argv and sys.argv[sys.argv.index('--tools')+1]==''
for line in sys.stdin:
 v=json.loads(line)
 assert v['type']=='control_request' and v['request']['subtype'] in ['initialize','get_usage'], v
 if v['request']['subtype']=='initialize':
  print(json.dumps({'type':'control_response','response':{'request_id':v['request_id'],'subtype':'success','response':{'models':[]}}}),flush=True)
  continue
 assert v['request']['skip_behaviors'] is True
 print(json.dumps({'type':'control_response','response':{'request_id':v['request_id'],'subtype':RESULT_KIND,'response':RESULT,'error':'Unsupported control request subtype'}}),flush=True)
"#;

async fn run(target: &Target, root: &Path) -> Value {
    let reading = read(
        target,
        &root.join("reads").join(target.config.harness.as_str()),
        100,
        &CancellationToken::new(),
        Duration::from_secs(20),
    )
    .await;
    let text = serde_json::to_string(&reading).unwrap();
    assert!(!text.contains(CANARY), "{text}");
    assert!(!reading.inference_requested);
    assert_eq!(reading.observed_at, Some(100));
    // The login link never outlives the read.
    assert!(
        !root
            .join("reads/codex/home/auth.json")
            .symlink_metadata()
            .is_ok()
    );
    serde_json::to_value(reading).unwrap()
}

#[tokio::test]
async fn codex_available_partial_and_unsupported() {
    for (result, state, check) in [
        (
            json!({"accountId":CANARY,"rateLimits":{"limitId":"codex","planType":"plus","primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1000},
                "secondary":{"usedPercent":60,"windowDurationMins":10080,"resetsAt":2000},"credits":{"balance":CANARY}}}),
            "available",
            json!([75.0, 40.0]),
        ),
        (
            json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":null},"secondary":null}}),
            "partial",
            json!([75.0]),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let body = CODEX.replace("RESULT", &format!("json.loads({:?})", result.to_string()));
        let t = target(root.path(), Harness::Codex, &body);
        let reading = run(&t, root.path()).await;
        assert_eq!(reading["state"], state, "{reading}");
        let remaining: Vec<_> = reading["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["remainingPercent"].clone())
            .collect();
        assert_eq!(json!(remaining), check);
    }
    let root = tempfile::tempdir().unwrap();
    let body = r#"
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 if v['method']=='initialize': print(json.dumps({'id':v['id'],'result':{}}),flush=True)
 else: print(json.dumps({'id':v['id'],'error':{'code':-32600,'message':'Invalid request: unknown variant `account/rateLimits/read`'}}),flush=True)
"#;
    let t = target(root.path(), Harness::Codex, body);
    let reading = run(&t, root.path()).await;
    assert_eq!(reading["state"], "unavailable");
    assert_eq!(
        reading["reason"],
        "The installed Codex does not support account/rateLimits/read."
    );
    assert_eq!(reading["windows"], json!([]));
}

#[tokio::test]
async fn claude_available_and_unsupported_without_a_prompt() {
    let root = tempfile::tempdir().unwrap();
    let result = json!({"subscription_type":"max","rate_limits_available":true,"session":{"total_cost_usd":0},
        "rate_limits":{"five_hour":{"utilization":10,"resets_at":"2026-10-19T09:00:00+00:00"},"seven_day":{"utilization":50,"resets_at":"2026-10-23T21:00:00+00:00"}}});
    let body = CLAUDE
        .replace("RESULT_KIND", "'success'")
        .replace("RESULT", &format!("json.loads({:?})", result.to_string()));
    let t = target(root.path(), Harness::ClaudeCode, &body);
    let reading = run(&t, root.path()).await;
    assert_eq!(reading["state"], "available", "{reading}");
    assert_eq!(reading["plan"], "max");
    assert_eq!(reading["windows"][0]["remainingPercent"], 90.0);
    assert_eq!(reading["windows"][1]["label"], "Weekly");

    let root = tempfile::tempdir().unwrap();
    let body = CLAUDE
        .replace("RESULT_KIND", "'error'")
        .replace("RESULT", "None");
    let t = target(root.path(), Harness::ClaudeCode, &body);
    let reading = run(&t, root.path()).await;
    assert_eq!(reading["state"], "unavailable");
    assert!(
        reading["reason"]
            .as_str()
            .unwrap()
            .contains("does not answer the experimental get_usage")
    );
}

#[tokio::test]
async fn hung_harness_times_out_as_unavailable_and_is_stopped() {
    let root = tempfile::tempdir().unwrap();
    let t = target(root.path(), Harness::ClaudeCode, "time.sleep(60)");
    let reading = read(
        &t,
        &root.path().join("reads/claude-code"),
        5,
        &CancellationToken::new(),
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(reading.state, State::Unavailable);
    assert!(reading.windows.is_empty());
}

#[tokio::test]
async fn cache_reads_each_harness_and_names_missing_ones() {
    let root = tempfile::tempdir().unwrap();
    let allowance = Allowance::open(&root.path().join("allowance"), || {}).unwrap();
    let before = allowance.view();
    assert_eq!(before["harnesses"][0]["state"], "unavailable");
    assert!(
        before["harnesses"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("Not read yet")
    );
    assert_eq!(before["harnesses"][0]["observedAt"], Value::Null);
    let result = json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":5,"windowDurationMins":300,"resetsAt":1000}}});
    let body = CODEX.replace("RESULT", &format!("json.loads({:?})", result.to_string()));
    let codex = target(root.path(), Harness::Codex, &body);
    allowance
        .refresh_with(
            vec![codex],
            "{harness} is not installed for this service.",
            Duration::from_secs(20),
        )
        .unwrap();
    allowance.settle().await;
    let view = allowance.view();
    assert_eq!(view["state"], "idle");
    assert_eq!(view["harnesses"][0]["harness"], "codex");
    assert_eq!(view["harnesses"][0]["state"], "available");
    assert_eq!(view["harnesses"][1]["harness"], "claude-code");
    assert_eq!(view["harnesses"][1]["state"], "unavailable");
    assert_eq!(
        view["harnesses"][1]["reason"],
        "Claude Code is not installed for this service."
    );
    assert!(view["harnesses"][1]["observedAt"].is_u64());
}
