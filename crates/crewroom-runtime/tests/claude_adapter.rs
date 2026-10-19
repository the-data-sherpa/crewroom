//! Deterministic production-adapter faults. The subprocess is explicitly a fake
//! protocol peer and all account metadata is fake; live checks are separate.
use crewroom_runtime::runner;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};
use tokio::time::{Duration, timeout};

/// Writes a fake CLI without this process ever holding a writer to it (FOR-86).
/// Every fork by a sibling test thread copies all open descriptors until its
/// child execs. If one copy is a writer of the script, exec of the script fails
/// with `Text file busy` (ETXTBSY), so admission failed for a load-dependent
/// reason. The only writer now lives in a separate `cp`, which exits first.
fn install_executable(path: &Path, content: &str) {
    let staged = path.with_extension("source");
    std::fs::write(&staged, content).unwrap();
    let copied = std::process::Command::new("cp")
        .arg(&staged)
        .arg(path)
        .status()
        .unwrap();
    assert!(copied.success(), "cp {}: {copied}", path.display());
    std::fs::remove_file(&staged).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    config: Value,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let binary = root.join("claude");
        install_executable(&binary, include_str!("fixtures/claude-rpc.py"));
        std::fs::write(root.join("mode"), mode).unwrap();
        let account = root.join(".claude.json");
        std::fs::write(
            &account,
            r#"{"oauthAccount":{"accountUuid":"fake","organizationUuid":"fake"}}"#,
        )
        .unwrap();
        std::fs::set_permissions(account, std::fs::Permissions::from_mode(0o600)).unwrap();
        let config = json!({"assignmentId":"fixture","runnerCwd":root,"prompt":"Fixture only","claudeCode":{"binary":binary,"home":root},"model":{"resolved":{"adapter":"claude-code","provider":"claude-code","api":"claude-code-stream","id":"sonnet","resolvedModel":"claude-sonnet-5-5","effort":"low"}}});
        Self {
            _temp: temp,
            root,
            config,
        }
    }
    async fn open(
        &self,
    ) -> (
        crewroom_core::Result<(runner::Runner, Value)>,
        tokio::sync::mpsc::UnboundedReceiver<Value>,
    ) {
        let (send, receive) = tokio::sync::mpsc::unbounded_channel();
        let tools = vec![
            json!({"name":"echo","description":"fixture","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}}),
        ];
        let attempt_root = self.root.join(format!("attempt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&attempt_root).unwrap();
        std::fs::set_permissions(&attempt_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = runner::open(
            Path::new(env!("CARGO_BIN_EXE_crewroom-process")),
            &attempt_root,
            &self.config,
            "http://127.0.0.1:9",
            &tools,
            Arc::new(move |v| {
                let _ = send.send(v);
            }),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
        (result, receive)
    }
}

#[tokio::test]
async fn malformed_or_unaddressed_calls_never_reach_the_host() {
    for mode in [
        "wrong-session",
        "wrong-turn",
        "wrong-server",
        "stale-call",
        "changed-arguments",
        "schema",
        "native-tool",
        "skip-gate",
        "effort-change",
        "model-change",
        "wrong-usage-session",
        "malformed",
        "oversized",
        "eof",
        "partial-eof",
    ] {
        let f = Fixture::new(mode);
        let (opened, mut events) = f.open().await;
        let (r, _) = opened.unwrap();
        r.submit("input", "Fixture", false).await.unwrap();
        timeout(Duration::from_secs(5), async {
            while let Some(v) = events.recv().await {
                assert_ne!(v["type"], "host_tool_call", "{mode}: {v}");
                assert_ne!(v["type"], "provider_usage", "{mode}: {v}");
                if v["type"] == "rpc_stream_closed" || v["type"] == "model_policy_rejected" {
                    return;
                }
            }
            panic!("Missing failure for {mode}")
        })
        .await
        .unwrap();
        r.close().await.unwrap();
    }
}

#[tokio::test]
async fn alias_drift_and_changed_account_block_admission() {
    // Each block must be refused for its own reason; an unrelated launch error
    // (such as ETXTBSY under load, FOR-86) must not satisfy the assertion.
    let f = Fixture::new("alias-change");
    let (r, _) = f.open().await;
    let refused = r.err().expect("alias drift was admitted").to_string();
    assert!(
        refused.contains("Claude alias or effort differs from the frozen assignment"),
        "{refused}"
    );
    let f = Fixture::new("normal");
    let (r, _) = f.open().await;
    let (r, _) = r.unwrap();
    std::fs::write(
        f.root.join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"changed","organizationUuid":"fake"}}"#,
    )
    .unwrap();
    let refused = r
        .submit("input", "Fixture", false)
        .await
        .err()
        .expect("changed account was admitted")
        .to_string();
    assert!(
        refused.contains("Claude authority or native account changed"),
        "{refused}"
    );
    r.close().await.unwrap();
}

#[tokio::test]
async fn unconfirmed_input_survives_cancellation_and_cannot_be_replayed() {
    let f = Fixture::new("unconfirmed");
    let (r, _) = f.open().await;
    let (r, _) = r.unwrap();
    assert!(
        timeout(
            Duration::from_millis(100),
            r.submit("input", "Fixture", false)
        )
        .await
        .is_err()
    );
    r.close().await.unwrap();
    drop(r);
    let (reopened, _) = f.open().await;
    assert!(reopened.is_err());
    assert!(
        std::fs::read_to_string(f.root.join("claude-session/crewroom.jsonl"))
            .unwrap()
            .contains("\"kind\":\"input\"")
    );
}

#[tokio::test]
async fn duplicate_terminal_does_not_repeat_usage_and_provider_error_is_not_success() {
    for mode in [
        "duplicate-result",
        "provider-error",
        "duplicate-request",
        "text-interrupted",
    ] {
        let f = Fixture::new(mode);
        let (opened, mut events) = f.open().await;
        let (r, _) = opened.unwrap();
        r.submit("input", "Fixture", false).await.unwrap();
        let mut calls = 0;
        let mut terminal = 0;
        let mut usage = 0;
        timeout(Duration::from_secs(5),async {
            while let Some(v)=events.recv().await {
                match v["type"].as_str(){
                    Some("host_tool_call")=>{calls+=1;assert_eq!(calls,1);if mode!="duplicate-request" {r.resolve_tool(&json!({"id":v["id"],"outcome":"confirmed","isError":false,"result":{"content":[{"type":"text","text":"receipt"}]}})).await.unwrap();}},
                    Some("provider_usage")=>{usage+=1;assert_eq!(usage,1);assert!(v["usage"].get("reasoning").is_none());},
                    Some("prompt_result")=>{terminal+=1;assert_eq!(terminal,1);assert_eq!(v["status"],if mode=="provider-error"{"error"}else if mode=="text-interrupted"{"aborted"}else{"completed"});},
                    Some("session_settled")=>break,
                    Some("model_policy_rejected") if mode=="duplicate-request"=>break,
                    _=>(),
                }
            }
        }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        while let Ok(v) = events.try_recv() {
            assert!(
                !["host_tool_call", "prompt_result", "provider_usage"]
                    .iter()
                    .any(|k| v["type"] == *k)
            );
        }
        assert_eq!(calls, 1);
        r.close().await.unwrap();
    }
}

async fn event(events: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, kind: &str) -> Value {
    timeout(Duration::from_secs(15), async {
        while let Some(v) = events.recv().await {
            if v["type"] == kind {
                return v;
            }
            if kind != "rpc_stream_closed" {
                assert_ne!(v["type"], "model_policy_rejected", "{v}");
            }
        }
        panic!("Missing {kind}")
    })
    .await
    .unwrap()
}
fn receipt(call: &Value) -> Value {
    json!({"id":call["id"],"outcome":"confirmed","isError":false,"result":{"content":[{"type":"text","text":"receipt"}]}})
}

#[tokio::test]
async fn absorbed_steering_and_terminal_before_queued_settle_original_inputs() {
    for mode in ["folded", "terminal-before-queued"] {
        let f = Fixture::new(mode);
        let (opened, mut events) = f.open().await;
        let (runner, _) = opened.unwrap();
        runner.submit("first", "Initial", false).await.unwrap();
        if mode == "folded" {
            runner.submit("second", "Steering", true).await.unwrap();
        }
        let mut completed = std::collections::BTreeSet::new();
        loop {
            let v = event(&mut events, "prompt_result").await;
            assert_eq!(v["status"], "completed");
            assert!(completed.insert(v["id"].as_str().unwrap().to_owned()));
            if completed.len() == if mode == "folded" { 2 } else { 1 } {
                break;
            }
        }
        assert!(completed.contains("first"));
        if mode == "folded" {
            assert!(completed.contains("second"));
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(runner.state().await.unwrap()["isStreaming"], false);
        runner.close().await.unwrap();
    }
}

#[tokio::test]
async fn partial_frame_deadline_preserves_unknown_dispatch_and_no_late_success() {
    for mode in ["partial-stall", "wrong-usage-session"] {
        let f = Fixture::new(mode);
        let (opened, mut events) = f.open().await;
        let (runner, _) = opened.unwrap();
        runner.submit("first", "Initial", false).await.unwrap();
        event(&mut events, "rpc_stream_closed").await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        while let Ok(v) = events.try_recv() {
            assert!(
                !["prompt_result", "provider_usage", "host_tool_call"]
                    .iter()
                    .any(|k| v["type"] == *k),
                "{v}"
            );
        }
        assert!(runner.state().await.is_err());
        runner.close().await.unwrap();
        let journal =
            std::fs::read_to_string(f.root.join("claude-session/crewroom.jsonl")).unwrap();
        assert!(journal.contains("\"kind\":\"input\""));
        assert!(!journal.contains("\"kind\":\"terminal\""));
    }
}

#[tokio::test]
async fn recovery_holds_every_unknown_boundary_and_preserves_delivered_receipts() {
    for phase in [
        "intent-only",
        "receipt-not-delivered",
        "delivered",
        "absent",
        "corrupt",
        "wrong-session",
        "off-chain",
        "old-checkpoint",
        "compacted",
        "content-replacement",
        "changed-binding",
    ] {
        let mut f = Fixture::new(if phase == "receipt-not-delivered" {
            phase
        } else {
            "normal"
        });
        let (opened, mut events) = f.open().await;
        let (runner, identity) = opened.unwrap();
        // A concurrent owner cannot even inspect/start a second native process.
        if phase == "delivered" {
            assert!(timeout(Duration::from_millis(100), f.open()).await.is_err());
        }
        runner.submit("first", "Initial", false).await.unwrap();
        let call = event(&mut events, "host_tool_call").await;
        if phase != "intent-only" {
            runner.resolve_tool(&receipt(&call)).await.unwrap();
            event(&mut events, "prompt_result").await;
        }
        runner.close().await.unwrap();
        drop(runner);
        tokio::task::yield_now().await;
        let path = std::path::PathBuf::from(identity["sessionDirectory"].as_str().unwrap())
            .join(format!("{}.jsonl", identity["sessionId"].as_str().unwrap()));
        let original = std::fs::read_to_string(&path).unwrap();
        match phase {
            "absent"=>std::fs::remove_file(&path).unwrap(),
            "corrupt"=>std::fs::write(&path,format!("{original}{{unfinished")).unwrap(),
            "wrong-session"=>std::fs::write(&path,original.replace(identity["sessionId"].as_str().unwrap(),"another")).unwrap(),
            "off-chain"=>{
                let mut rows:Vec<Value>=original.lines().map(|l|serde_json::from_str(l).unwrap()).collect();
                rows.last_mut().unwrap()["parentUuid"]=Value::Null;
                std::fs::write(&path,rows.iter().map(|v|format!("{v}\n")).collect::<String>()).unwrap();
            }
            "old-checkpoint"=>{
                let first:Value=serde_json::from_str(original.lines().next().unwrap()).unwrap();
                std::fs::write(&path,format!("{original}{}\n",json!({"type":"last-prompt","leafUuid":first["uuid"],"explicit":true}))).unwrap();
            }
            "compacted"=>std::fs::write(&path,format!("{original}{}\n",json!({"type":"system","subtype":"compact_boundary","parentUuid":null,"uuid":"compact"}))).unwrap(),
            "content-replacement"=>std::fs::write(&path,format!("{original}{}\n",json!({"type":"content-replacement","uuid":"replacement"}))).unwrap(),
            "changed-binding"=>f.config["assignmentId"]=json!("another"),
            _=>(),
        }
        let (reopened, mut fresh_events) = f.open().await;
        if phase == "delivered" {
            let (replacement, recovered) = reopened.unwrap();
            assert_eq!(recovered["sessionId"], identity["sessionId"]);
            assert_ne!(recovered["runnerPid"], identity["runnerPid"]);
            assert_eq!(recovered["recoveryMode"], "session");
            while let Ok(v) = fresh_events.try_recv() {
                assert_ne!(v["type"], "host_tool_call");
            }
            replacement.close().await.unwrap();
        } else {
            assert!(reopened.is_err(), "{phase} unexpectedly resumed");
        }
        let journal =
            std::fs::read_to_string(f.root.join("claude-session/crewroom.jsonl")).unwrap();
        assert_eq!(
            journal
                .lines()
                .filter(|l| l.contains("\"kind\":\"call\""))
                .count(),
            1,
            "{phase}"
        );
    }
}

#[tokio::test]
async fn missing_usage_remains_unknown_and_changed_terminal_is_rejected() {
    for mode in ["lost-usage", "changed-result"] {
        let f = Fixture::new(mode);
        let (opened, mut events) = f.open().await;
        let (runner, _) = opened.unwrap();
        runner.submit("input", "Initial", false).await.unwrap();
        let call = event(&mut events, "host_tool_call").await;
        runner.resolve_tool(&receipt(&call)).await.unwrap();
        let mut usage = 0;
        let mut terminal = 0;
        timeout(Duration::from_secs(5), async {
            while let Some(v) = events.recv().await {
                match v["type"].as_str() {
                    Some("provider_usage") => usage += 1,
                    Some("prompt_result") => terminal += 1,
                    Some("session_settled") if mode == "lost-usage" => break,
                    Some("model_policy_rejected") if mode == "changed-result" => break,
                    _ => (),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(terminal, 1);
        assert_eq!(usage, usize::from(mode != "lost-usage"));
        runner.close().await.unwrap();
    }
}

#[tokio::test]
async fn aborted_tool_holds_same_process_input_and_suppresses_late_delivery() {
    let f = Fixture::new("tool-unresolved");
    let (opened, mut events) = f.open().await;
    let (runner, _) = opened.unwrap();
    runner.submit("first", "Initial", false).await.unwrap();
    let call = event(&mut events, "host_tool_call").await;
    assert_eq!(
        event(&mut events, "prompt_result").await["status"],
        "aborted"
    );
    assert!(runner.submit("second", "Continue", false).await.is_err());
    runner.resolve_tool(&receipt(&call)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(runner.state().await.is_err());
    while let Ok(v) = events.try_recv() {
        assert_ne!(v["type"], "prompt_result");
    }
    runner.close().await.unwrap();
    drop(runner);
    tokio::task::yield_now().await;
    assert!(f.open().await.0.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fast_terminal_waits_for_result_delivery_and_unknown_effect_never_completes() {
    for mode in ["normal", "normal", "normal", "premature-success"] {
        let f = Fixture::new(mode);
        let (opened, mut events) = f.open().await;
        let (runner, _) = opened.unwrap();
        runner.submit("first", "Initial", false).await.unwrap();
        let call = event(&mut events, "host_tool_call").await;
        if mode == "premature-success" {
            event(&mut events, "rpc_stream_closed").await;
            assert!(runner.state().await.is_err());
            while let Ok(v) = events.try_recv() {
                assert_ne!(v["type"], "prompt_result");
            }
        } else {
            // The peer writes its terminal immediately after reading the result.
            runner.resolve_tool(&receipt(&call)).await.unwrap();
            assert_eq!(
                event(&mut events, "prompt_result").await["status"],
                "completed"
            );
            assert_eq!(runner.state().await.unwrap()["isStreaming"], false);
        }
        runner.close().await.unwrap();
    }
}

/// FOR-86: while sibling threads keep spawning children, every installed
/// fixture must start. A script written with `std::fs::write` in this process
/// fails here with `Text file busy` within a few hundred installs.
#[test]
fn fixture_executables_start_while_sibling_threads_spawn() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let spawners: Vec<_> = (0..4)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("/bin/true").status();
                }
            })
        })
        .collect();
    let installers: Vec<_> = (0..4)
        .map(|w| {
            let root = temp.path().to_owned();
            std::thread::spawn(move || {
                for i in 0..150 {
                    let path = root.join(format!("fixture-{w}-{i}"));
                    install_executable(&path, "#!/bin/sh\nexit 0\n");
                    let status = std::process::Command::new(&path).status();
                    assert!(
                        status.as_ref().is_ok_and(|s| s.success()),
                        "{i}: {status:?}"
                    );
                }
            })
        })
        .collect();
    let installed: Vec<_> = installers.into_iter().map(|t| t.join()).collect();
    stop.store(true, Ordering::Relaxed);
    for spawner in spawners {
        spawner.join().unwrap();
    }
    for result in installed {
        result.unwrap();
    }
}
