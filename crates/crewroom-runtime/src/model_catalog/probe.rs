//! FOR-69 launch-protocol handshakes. Each starts the harness with the same
//! launch-shaped arguments an attempt uses, exchanges only initialization and
//! state frames, and stops it. No prompt is sent, so no inference can run.
use super::{
    compatibility::{CLAUDE_STREAM, CODEX_CATALOG, CODEX_CONTROL, OMP_RPC},
    discovery::{Config, Process, bounded, private_dir, request},
    *,
};
use crate::durable_new;
use crewroom_core::Error;
use std::{collections::BTreeMap, path::Path};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

type Failure = (&'static str, Error);

/// Runs one bounded exchange in its own owned process and always verifies
/// cleanup, so a failed check never leaves a harness running.
#[allow(clippy::too_many_arguments)] // Process identity plus the shared bounds.
async fn exchange<T>(
    config: &Config,
    root: &Path,
    name: &str,
    args: Vec<String>,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    limit: Duration,
    decode: bool,
    f: impl for<'a> FnOnce(
        &'a mut Process,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>,
    >,
) -> Result<T> {
    let mut process = Process::start(config, root, name, args, env).await?;
    if decode {
        process.decoder = Some(crate::rpc_frame::Decoder::default());
    }
    let result = bounded(&mut process, cancel, limit, f).await;
    process.close().await?;
    result
}

pub(super) async fn handshake(
    config: &Config,
    root: &Path,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    limit: Duration,
) -> std::result::Result<(), Failure> {
    match config.harness {
        Harness::Codex => codex(config, root, env, cancel, limit).await,
        Harness::ClaudeCode => claude(config, root, env, cancel, limit)
            .await
            .map_err(|e| (CLAUDE_STREAM, e)),
        Harness::Omp => omp(config, root, env, cancel, limit)
            .await
            .map_err(|e| (OMP_RPC, e)),
    }
}

/// The bundled catalog `prepare` pins, then the app-server under
/// `--strict-config` with the attempt's generated configuration.
async fn codex(
    config: &Config,
    root: &Path,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    limit: Duration,
) -> std::result::Result<(), Failure> {
    let state = root.join("probe-codex");
    private_dir(&state).map_err(|e| (CODEX_CATALOG, e))?;
    let mut env = env.clone();
    env.insert("CODEX_HOME".into(), state.to_string_lossy().into());
    let bytes = exchange(
        config,
        root,
        "probe-catalog",
        ["debug", "models", "--bundled"].map(String::from).to_vec(),
        &env,
        cancel,
        limit,
        false,
        |p| Box::pin(p.collect()),
    )
    .await
    .map_err(|e| (CODEX_CATALOG, e))?;
    let raw: Value = serde_json::from_slice(&bytes).map_err(|_| {
        (
            CODEX_CATALOG,
            conflict("Bundled model catalog is not JSON."),
        )
    })?;
    let rows = raw["models"]
        .as_array()
        .filter(|rows| !rows.is_empty())
        .ok_or_else(|| {
            (
                CODEX_CATALOG,
                conflict("Bundled model catalog lists no models."),
            )
        })?;
    let pinned = rows
        .iter()
        .find_map(|row| {
            let effort = row["supported_reasoning_levels"]
                .as_array()?
                .iter()
                .find_map(|l| l["effort"].as_str().map(|_| l["effort"].clone()))?;
            row["slug"].as_str()?;
            crate::runner::codex_pinned(&raw, &row["slug"], &effort)
                .ok()
                .map(|row| (row, effort))
        })
        .ok_or_else(|| {
            (
                CODEX_CATALOG,
                conflict("No bundled model has the slug and reasoning levels launch pins."),
            )
        })?;
    let slug = pinned.0["slug"].clone();
    let catalog_path = state.join("models.json");
    let (catalog, settings) =
        crate::runner::codex_launch_files(pinned.0, &slug, &pinned.1, &catalog_path);
    durable_new(&catalog_path, catalog.as_bytes()).map_err(|e| (CODEX_CONTROL, e))?;
    durable_new(&state.join("config.toml"), settings.as_bytes()).map_err(|e| (CODEX_CONTROL, e))?;
    exchange(
        config,
        root,
        "probe-handshake",
        ["app-server", "--listen", "stdio://", "--strict-config"]
            .map(String::from)
            .to_vec(),
        &env,
        cancel,
        limit,
        false,
        |p| {
            Box::pin(async move {
                request(
                    p,
                    1,
                    "initialize",
                    json!({"clientInfo":{"name":"crewroom_compatibility_probe","version":"0.1.0"},
                        "capabilities":{"experimentalApi":true}}),
                )
                .await?;
                p.send(&json!({"method":"initialized"})).await
            })
        },
    )
    .await
    .map_err(|e| (CODEX_CONTROL, e))
}

/// The stream-json control handshake with the launch flags, the SDK tool
/// server and the tool gate hook an attempt registers.
async fn claude(
    config: &Config,
    root: &Path,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    limit: Duration,
) -> Result<()> {
    let mut args: Vec<String> = crate::runner::CLAUDE_LAUNCH_FLAGS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    args.push("--no-session-persistence".into());
    exchange(
        config,
        root,
        "probe-handshake",
        args,
        env,
        cancel,
        limit,
        false,
        |p| {
            Box::pin(async move {
                p.send(
                    &json!({"type":"control_request","request_id":"crewroom-probe","request":{
                "subtype":"initialize","sdkMcpServers":["crewroom"],
                "hooks":{"PreToolUse":[{"hookCallbackIds":["crewroom-gate"]}]}}}),
                )
                .await?;
                let frame = p
                    .receive(|v| {
                        v["type"] == "control_response"
                            && v["response"]["request_id"] == "crewroom-probe"
                    })
                    .await?;
                if frame["response"]["subtype"] != "success" {
                    return Err(conflict("Harness rejected the launch initialization."));
                }
                if discovery::normalize(
                    &Harness::ClaudeCode,
                    &frame["response"]["response"]["models"],
                    0,
                )?
                .is_empty()
                {
                    return Err(conflict("Launch initialization offered no models."));
                }
                Ok(())
            })
        },
    )
    .await
}

/// RPC mode with the launch flags and a synthetic loopback provider that is
/// never contacted: readiness, protocol-v2 negotiation, session state and
/// host tool registration, decoded with the runtime's frame decoder.
async fn omp(
    config: &Config,
    root: &Path,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    limit: Duration,
) -> Result<()> {
    let private = root.join("probe-config");
    let sessions = root.join("probe-sessions");
    let cwd = root.join("probe-cwd");
    for dir in [&private, &sessions, &cwd] {
        private_dir(dir)?;
    }
    let plan = json!({"resolved":{"provider":"crewroom-probe","selector":"crewroom-probe/probe","effort":"low"}});
    for (name, value) in [
        (
            "models.yml",
            json!({"providers":{"crewroom-probe":{"api":"openai-responses","apiKey":"crewroom-probe-offline",
                "baseUrl":"http://127.0.0.1:9/v1","models":[{"id":"probe","name":"Crewroom probe",
                "reasoning":true,"input":["text"],"contextWindow":128000,"maxTokens":8192}]}}}),
        ),
        (
            "config.yml",
            crate::model_policy::isolated_model_settings(&plan),
        ),
        ("mcp.json", json!({"mcpServers":{}})),
    ] {
        durable_new(&private.join(name), &serde_json::to_vec(&value)?)?;
    }
    let mut env = env.clone();
    env.insert(
        "PI_CODING_AGENT_DIR".into(),
        private.to_string_lossy().into(),
    );
    env.insert(
        "PI_CODING_AGENT_SESSION_DIR".into(),
        sessions.to_string_lossy().into(),
    );
    let mut args: Vec<String> = crate::runner::OMP_RPC_FLAGS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    args.extend([
        "--cwd".into(),
        cwd.to_string_lossy().into(),
        "--session-dir".into(),
        sessions.to_string_lossy().into(),
        "--model".into(),
        "crewroom-probe/probe".into(),
        "--thinking".into(),
        "low".into(),
        "--system-prompt".into(),
        "Crewroom compatibility probe.".into(),
    ]);
    exchange(
        config,
        root,
        "probe-handshake",
        args,
        &env,
        cancel,
        limit,
        true,
        |p| {
            Box::pin(async move {
                let ready = p.receive(|v| v["type"] == "ready").await?;
                if !ready["supportedProtocolVersions"]
                    .as_array()
                    .is_some_and(|v| v.contains(&json!(2)))
                {
                    return Err(conflict("Harness does not offer RPC protocol v2."));
                }
                let negotiated = omp_command(
                    p,
                    "crewroom-probe-1",
                    "negotiate_protocol",
                    json!({"protocolVersion":2}),
                )
                .await?;
                if negotiated["protocolVersion"] != 2 {
                    return Err(conflict("RPC protocol v2 negotiation failed."));
                }
                let state = omp_command(p, "crewroom-probe-2", "get_state", json!({})).await?;
                let expected = sessions.canonicalize()?;
                if !state["sessionFile"].as_str().is_some_and(|f| {
                    Path::new(f)
                        .parent()
                        .and_then(|d| d.canonicalize().ok())
                        .is_some_and(|d| d == expected)
                }) {
                    return Err(conflict("Session state does not match its directory."));
                }
                let registered =
                    omp_command(p, "crewroom-probe-3", "set_host_tools", json!({"tools":[]}))
                        .await?;
                if registered["toolNames"] != json!([]) {
                    return Err(conflict("Host tool registration did not match."));
                }
                Ok(())
            })
        },
    )
    .await
}

async fn omp_command(p: &mut Process, id: &str, kind: &str, args: Value) -> Result<Value> {
    let mut frame = args;
    frame["id"] = json!(id);
    frame["type"] = json!(kind);
    p.send(&frame).await?;
    let reply = p
        .receive(|v| v["type"] == "response" && v["id"] == id)
        .await?;
    if reply["success"] != true {
        return Err(conflict(&format!("Harness rejected {kind}.")));
    }
    Ok(reply.get("data").cloned().unwrap_or(Value::Null))
}
