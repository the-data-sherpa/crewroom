//! Explicit disposable adapter contract driver. Native access is opt-in through
//! a supplied private configuration; no defaults read the operator's login.
use crewroom_runtime::runner;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
    sync::Arc,
};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("config path required")?;
    let mut config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    // FOR-72 recording driver only: a transcript recorder wraps the real CLI in
    // a tee shim, so admission sees the shim, not the probed CLI. Admit the shim
    // as probed for this disposable recording; production admission is unchanged.
    if config["admitRecordingShim"] == true {
        admit_recording_shim(&mut config)?;
    }
    let origin = config["origin"]
        .as_str()
        .unwrap_or("http://127.0.0.1:9")
        .to_owned();
    let output = Arc::new(std::sync::Mutex::new(std::io::stdout()));
    let out = output.clone();
    let emit: runner::Event = Arc::new(move |value| {
        let mut out = out.lock().unwrap();
        writeln!(out, "{value}").unwrap();
        out.flush().unwrap();
    });
    let root = Path::new(config["root"].as_str().unwrap());
    let opened = runner::open(
        Path::new(config["helper"].as_str().unwrap()),
        root,
        &config,
        &origin,
        config["tools"].as_array().unwrap(),
        emit.clone(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;
    let (runner, identity) = match opened {
        Ok(v) => v,
        Err(e) => {
            emit(json!({"type":"setup_failed","reason":e.to_string()}));
            return Err(e.into());
        }
    };
    emit(json!({"type":"ready","identity":identity}));
    let (send, mut commands) = tokio::sync::mpsc::channel(32);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if send.blocking_send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = commands.recv().await {
        let v: Value = serde_json::from_str(&line)?;
        let result = match v["op"].as_str().unwrap_or("") {
            "input" => runner
                .submit(
                    v["id"].as_str().unwrap(),
                    v["message"].as_str().unwrap(),
                    v["steer"] == true,
                )
                .await
                .map(|_| json!({})),
            "tool" => runner.resolve_tool(&v["result"]).await.map(|_| json!({})),
            "interrupt" => runner.interrupt().await.map(|_| json!({})),
            "close" => {
                let r = runner.close().await;
                emit(json!({"type":"closed","ok":r.is_ok()}));
                r?;
                break;
            }
            "state" => runner.state().await,
            _ => Err(crewroom_core::Error::Invalid(
                "Unknown fixture command.".into(),
            )),
        };
        match result {
            Ok(value) => emit(json!({"type":"reply","id":v["id"],"result":value})),
            Err(e) => emit(json!({"type":"reply","id":v["id"],"error":e.to_string()})),
        }
    }
    runner.close().await?;
    Ok(())
}

fn admit_recording_shim(config: &mut Value) -> Result<(), Box<dyn std::error::Error>> {
    use crewroom_runtime::model_catalog::{
        Harness,
        compatibility::{Basis, Compatibility, ProbeReceipt},
        discovery::Identity,
    };
    use sha2::{Digest, Sha256};
    let (harness, binary) = match config["model"]["resolved"]["adapter"].as_str() {
        Some("codex") => (Harness::Codex, &config["codex"]["binary"]),
        Some("claude-code") => (Harness::ClaudeCode, &config["claudeCode"]["binary"]),
        _ => return Ok(()),
    };
    let binary = std::path::PathBuf::from(binary.as_str().ok_or("binary required")?);
    let canonical = binary.canonicalize()?;
    let sha = format!("{:x}", Sha256::digest(std::fs::read(&canonical)?));
    let version = config["recordingVersion"]
        .as_str()
        .ok_or("recordingVersion required")?
        .to_owned();
    let receipt = ProbeReceipt::new(&harness, &version, &canonical, &sha, 1);
    config["model"]["selection"]["identity"] = serde_json::to_value(Identity {
        binary: binary.clone(),
        canonical_binary: canonical,
        binary_sha256: sha,
        binary_stamp: "recording".into(),
        version,
        profile: "recording".into(),
        compatibility: Some(Compatibility {
            basis: Basis::Probed,
            probe: Some(receipt),
        }),
    })?;
    Ok(())
}
