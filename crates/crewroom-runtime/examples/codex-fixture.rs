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
    let config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
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
        "http://127.0.0.1:9",
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
