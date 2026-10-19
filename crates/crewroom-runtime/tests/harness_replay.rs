//! FOR-72 SIMULATED replay of recorded harness sessions. Each transcript under
//! `tests/fixtures/harness-transcripts` was recorded from a real Codex, Claude
//! Code or OMP binary with a synthetic provider (`scripts/harness_transcripts.py`).
//! A replay peer stands in for that binary and Crewroom's production adapter
//! drives it through the recorded scenario. The adapter must send the recorded
//! frame kinds and reproduce the host-visible events of the recording. These
//! are simulated checks: they never count as live delivery or a live harness run.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use crewroom_runtime::{
    model_catalog::{
        Harness,
        compatibility::{Basis, Compatibility, ProbeReceipt},
        discovery::Identity,
    },
    runner,
};
use crewroom_test_support::install_executable;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::mpsc, time::timeout};

const PEER: &str = include_str!("fixtures/harness-replay.py");
/// Adapter events whose per-type sequence must match the recording.
const EXPECTED: &[&str] = &[
    "opened",
    "ready",
    "setup_failed",
    "agent_start",
    "agent_end",
    "host_tool_call",
    "host_tool_cancel",
    "prompt_result",
    "model_policy_rejected",
    "harness_compaction",
    "message_end",
];

fn transcripts(harness: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/harness-transcripts")
        .join(harness);
    let mut found: Vec<_> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    found.retain(|p| p.extension().is_some_and(|e| e == "jsonl"));
    found.sort();
    found
}

fn read(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Replace `<<name>>` placeholders in every string with bound values.
fn fill(value: &Value, binds: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(s) => {
            let mut out = s.clone();
            for (name, bound) in binds {
                out = out.replace(name, bound);
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, binds)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), fill(v, binds)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

fn private(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn synthetic_token() -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    format!(
        "synthetic.{}.signature",
        URL_SAFE_NO_PAD.encode(json!({"exp": exp}).to_string())
    )
}

/// The outcome of one simulated replay.
pub struct Replay {
    pub label: String,
    pub sessions: usize,
    pub drift: Vec<String>,
    pub expected: Value,
    pub observed: Value,
    pub errors: Vec<String>,
}

fn expectations(events: &[Value]) -> Value {
    let mut out = Map::new();
    for event in events {
        let kind = event["type"].as_str().unwrap_or("");
        if !EXPECTED.contains(&kind) {
            continue;
        }
        let mut item = json!({"type": kind});
        let keys: &[&str] = match kind {
            "prompt_result" => &["id", "status"],
            "host_tool_call" => &["toolName", "arguments"],
            _ => &[],
        };
        for key in keys {
            if let Some(v) = event.get(*key) {
                item[*key] = v.clone();
            }
        }
        if kind == "opened" {
            item["recoveryMode"] = event["identity"]["recoveryMode"].clone();
        }
        out.entry(kind)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(item);
    }
    Value::Object(out)
}

fn matches(step: &Value, event: &Value) -> bool {
    event["type"] == step["type"]
        && ["id", "status"]
            .iter()
            .all(|k| step.get(*k).is_none() || event[*k] == step[*k])
}

/// Consume the first event at or after `cursor` that satisfies a `wait` step
/// and return the cursor just past it. Like the recorder's per-session queue,
/// a wait never matches an event that an earlier wait or close consumed, so an
/// earlier session's `agent_end` cannot satisfy the reattached turn's wait.
fn take_match(events: &[Value], cursor: usize, step: &Value) -> Option<usize> {
    events[cursor.min(events.len())..]
        .iter()
        .position(|e| matches(step, e))
        .map(|at| cursor + at + 1)
}

/// Drive the production adapter through the transcript's recorded steps
/// against the replay peer. `lines` may be a mutated copy of a transcript.
pub async fn replay(lines: &[Value]) -> Replay {
    replay_within(lines, Duration::from_secs(20)).await
}

/// `replay` with a bound on how long each `wait` step may take.
pub async fn replay_within(lines: &[Value], wait_limit: Duration) -> Replay {
    let header = &lines[0];
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    private(&root);
    for dir in ["assignment", "auth", "harness-home"] {
        private(&root.join(dir));
    }
    let binary = root.join(header["binaryName"].as_str().unwrap());
    install_executable(&binary, PEER);
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let peer = |suffix: &str| PathBuf::from(format!("{}{suffix}", binary.display()));
    std::fs::write(peer(".transcript.jsonl"), body).unwrap();
    let mut binds = BTreeMap::from([
        ("<<root>>".to_owned(), root.to_string_lossy().into_owned()),
        ("<<synthetic-access-token>>".to_owned(), synthetic_token()),
    ]);
    for file in header["setupFiles"].as_array().into_iter().flatten() {
        let path = PathBuf::from(fill(&file["path"], &binds).as_str().unwrap());
        std::fs::write(&path, fill(&file["content"], &binds).as_str().unwrap()).unwrap();
        std::fs::set_permissions(
            &path,
            std::fs::Permissions::from_mode(file["mode"].as_u64().unwrap() as u32),
        )
        .unwrap();
    }
    let harness = match header["harness"].as_str().unwrap() {
        "codex" => Some(Harness::Codex),
        "claude-code" => Some(Harness::ClaudeCode),
        _ => None,
    };
    let content = std::fs::read(&binary).unwrap();
    let sha = format!("{:x}", Sha256::digest(&content));
    let version = header["version"].as_str().unwrap().to_owned();
    let tools: Vec<Value> = header["tools"].as_array().unwrap().clone();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (send, mut receive) = mpsc::unbounded_channel::<Value>();
    let mut current: Option<runner::Runner> = None;
    let mut opens = 0;
    // Index of the first event no `wait` or `close` has consumed yet.
    let mut cursor = 0usize;
    let mut errors = Vec::new();
    let label = format!(
        "{} {} {}",
        header["harness"].as_str().unwrap(),
        version,
        header["scenario"].as_str().unwrap()
    );
    'steps: for step in header["steps"].as_array().unwrap() {
        let op = step["op"].as_str().unwrap();
        let outcome: Result<(), String> = async {
            match op {
                "open" => {
                    opens += 1;
                    let attempt = root.join(format!("attempt-{opens}"));
                    private(&attempt);
                    // Session identifiers the peer bound in earlier processes.
                    if let Ok(state) = std::fs::read_to_string(peer(".state.json")) {
                        let state: Value = serde_json::from_str(&state).unwrap();
                        for (k, v) in state["binds"].as_object().unwrap() {
                            if k.starts_with("<<uuid:") {
                                binds.insert(k.clone(), v.as_str().unwrap().into());
                            }
                        }
                    }
                    let mut config = fill(&step["config"], &binds);
                    if let Some(harness) = &harness {
                        // The peer stands in for the recorded executable; admit
                        // it as probed for the recorded version (simulated).
                        let receipt = ProbeReceipt::new(harness, &version, &binary, &sha, 1);
                        config["model"]["selection"]["identity"] = serde_json::to_value(Identity {
                            binary: binary.clone(),
                            canonical_binary: binary.clone(),
                            binary_sha256: sha.clone(),
                            binary_stamp: "replay".into(),
                            version: version.clone(),
                            profile: "replay".into(),
                            compatibility: Some(Compatibility {
                                basis: Basis::Probed,
                                probe: Some(receipt),
                            }),
                        })
                        .unwrap();
                    }
                    let sink = send.clone();
                    let (runner, identity) = runner::open(
                        Path::new(env!("CARGO_BIN_EXE_crewroom-process")),
                        &attempt,
                        &config,
                        "http://127.0.0.1:9",
                        &tools,
                        Arc::new(move |v| {
                            let _ = sink.send(v);
                        }),
                        &tokio_util::sync::CancellationToken::new(),
                    )
                    .await
                    .map_err(|e| format!("open {opens}: {e}"))?;
                    events
                        .lock()
                        .unwrap()
                        .push(json!({"type":"opened","identity":identity}));
                    current = Some(runner);
                }
                "input" => current
                    .as_ref()
                    .unwrap()
                    .submit(
                        step["id"].as_str().unwrap(),
                        step["message"].as_str().unwrap(),
                        step["steer"] == true,
                    )
                    .await
                    .map_err(|e| format!("input {}: {e}", step["id"]))?,
                "wait" => {
                    let found = take_match(&events.lock().unwrap(), cursor, step);
                    if let Some(next) = found {
                        cursor = next;
                    } else {
                        cursor = timeout(wait_limit, async {
                            while let Some(event) = receive.recv().await {
                                let mut seen = events.lock().unwrap();
                                seen.push(event.clone());
                                if matches(step, &event) {
                                    return Ok(seen.len());
                                }
                                if event["type"] == "model_policy_rejected" {
                                    return Err(format!("adapter rejected: {}", event["reason"]));
                                }
                            }
                            Err("event stream closed".to_owned())
                        })
                        .await
                        .map_err(|_| format!("timed out waiting for {step}"))??;
                    }
                }
                "tool" => {
                    let call = events
                        .lock()
                        .unwrap()
                        .iter()
                        .rev()
                        .find(|e| e["type"] == "host_tool_call")
                        .cloned()
                        .ok_or("no host tool call")?;
                    current
                        .as_ref()
                        .unwrap()
                        .resolve_tool(
                            &json!({"type":"host_tool_result","id":call["id"],"isError":false,
                            "result":{"content":[{"type":"text","text":step["text"]}]}}),
                        )
                        .await
                        .map_err(|e| format!("tool: {e}"))?;
                }
                "interrupt" => current
                    .as_ref()
                    .unwrap()
                    .interrupt()
                    .await
                    .map_err(|e| format!("interrupt: {e}"))?,
                "pause" => {
                    tokio::time::sleep(Duration::from_millis(step["ms"].as_u64().unwrap())).await
                }
                "close" => {
                    let runner = current.take().unwrap();
                    runner.close().await.map_err(|e| format!("close: {e}"))?;
                    // Collect what the adapter emitted before it closed.
                    while let Ok(event) = receive.try_recv() {
                        events.lock().unwrap().push(event);
                    }
                    // As the recorder starts a new queue per process, a later
                    // wait must not match what this session emitted.
                    cursor = events.lock().unwrap().len();
                }
                other => return Err(format!("unknown step {other}")),
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            errors.push(error);
            break 'steps;
        }
    }
    if let Some(runner) = current.take() {
        let _ = runner.close().await;
    }
    while let Ok(event) = receive.try_recv() {
        events.lock().unwrap().push(event);
    }
    let lines_of = |suffix: &str| -> Vec<String> {
        std::fs::read_to_string(peer(suffix))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    };
    errors.extend(lines_of(".errors"));
    let sessions = lines[1..]
        .iter()
        .filter_map(|e| e["session"].as_u64())
        .max()
        .unwrap_or(0) as usize;
    let done = lines_of(".done");
    for session in 1..=sessions {
        if !done.contains(&format!("session {session} complete")) {
            errors.push(format!(
                "session {session} did not reach the end of its transcript"
            ));
        }
    }
    let observed = expectations(&events.lock().unwrap());
    Replay {
        label,
        sessions,
        drift: lines_of(".drift"),
        expected: header["expect"].clone(),
        observed,
        errors,
    }
}

fn report(results: &[Replay]) {
    for r in results {
        println!(
            "SIMULATED replay (not live evidence): {} — {} session(s), {} client drift note(s)",
            r.label,
            r.sessions,
            r.drift.len()
        );
        for d in &r.drift {
            println!("  drift: {d}");
        }
    }
    if let Ok(path) = std::env::var("CREWROOM_REPLAY_REPORT") {
        let rows: Vec<_> = results
            .iter()
            .map(|r| {
                json!({"evidence":"simulated-replay","transcript":r.label,"sessions":r.sessions,
                "drift":r.drift,"passed":r.errors.is_empty() && r.expected == r.observed})
            })
            .collect();
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "{}", json!(rows))
            });
    }
}

async fn replay_all(harness: &str) {
    let files = transcripts(harness);
    assert!(
        !files.is_empty(),
        "no {harness} transcripts are committed; record them with scripts/harness_transcripts.py"
    );
    let mut results = Vec::new();
    for file in files {
        results.push(replay(&read(&file)).await);
    }
    report(&results);
    for r in &results {
        assert!(r.errors.is_empty(), "{}: {:?}", r.label, r.errors);
        assert_eq!(r.observed, r.expected, "{}: adapter events differ", r.label);
    }
}

#[tokio::test]
async fn codex_adapter_replays_recorded_sessions() {
    replay_all("codex").await;
}

#[tokio::test]
async fn claude_adapter_replays_recorded_sessions() {
    replay_all("claude-code").await;
}

#[tokio::test]
async fn omp_adapter_replays_recorded_sessions() {
    replay_all("omp").await;
}

/// A changed harness frame is caught, not silently absorbed: the replay
/// reports the adapter's refusal or a different event sequence.
#[tokio::test]
async fn a_changed_harness_frame_fails_the_replay() {
    let file = transcripts("claude-code")
        .into_iter()
        .find(|p| p.ends_with("tool-completion.jsonl"))
        .expect("claude-code tool-completion transcript");
    let mut lines = read(&file);
    let changed = lines
        .iter_mut()
        .find(|l| l["dir"] == "recv" && l["frame"]["type"] == "assistant")
        .expect("an assistant frame");
    changed["frame"]["message"]["model"] = json!("claude-unadmitted-model");
    let r = replay(&lines).await;
    assert!(
        !r.errors.is_empty() || r.observed != r.expected,
        "a changed model frame must fail the replay"
    );
    // A changed client protocol is a hard peer failure.
    let mut lines = read(&file);
    let sent = lines
        .iter_mut()
        .find(|l| l["dir"] == "send" && l["frame"]["type"] == "user")
        .expect("a user frame");
    sent["frame"]["type"] = json!("renamed_user");
    let r = replay(&lines).await;
    assert!(
        r.errors.iter().any(|e| e.contains("adapter sent 'user'")),
        "{:?}",
        r.errors
    );
}

/// A wait never matches an event an earlier wait or session consumed.
#[test]
fn a_wait_only_matches_unconsumed_events() {
    let step = json!({"op": "wait", "type": "agent_end"});
    let events = vec![
        json!({"type": "agent_start"}),
        json!({"type": "agent_end"}),
        json!({"type": "opened"}),
        json!({"type": "agent_start"}),
    ];
    assert_eq!(take_match(&events, 0, &step), Some(2));
    // After session 1 closed, its agent_end no longer satisfies the wait.
    assert_eq!(take_match(&events, 2, &step), None);
    assert_eq!(take_match(&events, 9, &step), None);
}

/// The reattach replay must wait for the resumed turn itself. With the second
/// session's `agent_end` removed (and the expectation adjusted so only the wait
/// can catch it), the first session's `agent_end` must not satisfy the wait.
#[tokio::test]
async fn the_reattached_turn_must_complete_on_its_own() {
    let file = transcripts("omp")
        .into_iter()
        .find(|p| p.ends_with("reattach.jsonl"))
        .expect("omp reattach transcript");
    let mut lines = read(&file);
    let at = lines
        .iter()
        .position(|l| l["session"] == 2 && l["dir"] == "recv" && l["frame"]["type"] == "agent_end")
        .expect("the second session's agent_end");
    lines.remove(at);
    let ends = lines[0]["expect"]["agent_end"].as_array_mut().unwrap();
    assert_eq!(ends.len(), 2, "both sessions end a turn");
    ends.pop();
    let r = replay_within(&lines, Duration::from_secs(3)).await;
    assert!(
        r.errors.iter().any(|e| e.contains("timed out waiting")),
        "the wait must not be satisfied by session 1: {:?}",
        r.errors
    );
}
