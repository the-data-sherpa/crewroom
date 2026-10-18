//! Disposable test HTTP surface around the production gateway, controlled over stdin.
use axum::{
    Router,
    body::Body,
    http::{Request, Response},
};
use crewroom_core::Result;
use crewroom_runtime::provider_gateway::ProviderGateway;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
    sync::{Arc, Mutex},
};
fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
}
fn create(config: &Value) -> Result<ProviderGateway> {
    ProviderGateway::new(
        Path::new(config["root"].as_str().unwrap()),
        config["plan"].clone(),
        config["provider"].clone(),
        "host-token".into(),
        config["budgetRoot"].as_str().map(Path::new),
        |event| emit(json!({"event":event})),
    )
}
#[tokio::main]
async fn main() -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    });
    let initial: Value = serde_json::from_str(&rx.recv().await.unwrap()?)?;
    let current = Arc::new(Mutex::new(create(&initial)?));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let state = current.clone();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stopped = shutdown.clone();
    let server = tokio::spawn(async move {
        let router = Router::new().fallback(move |request: Request<Body>| {
            let gateway = state.lock().unwrap().clone();
            async move {
                match gateway.serve(request).await {
                    Ok(r) => r,
                    Err(e) => Response::builder()
                        .status(409)
                        .header("Content-Type", "application/json")
                        .body(Body::from(json!({"error":e.to_string()}).to_string()))
                        .unwrap(),
                }
            }
        });
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
    });
    emit(json!({"ready":true,"origin":origin}));
    while let Some(line) = rx.recv().await {
        let command: Value = serde_json::from_str(&line?)?;
        let result = match command["op"].as_str().unwrap() {
            "state" => json!({"failure":current.lock().unwrap().failure()}),
            "replace" => {
                let replacement = create(&command["config"])?;
                let mut gateway = current.lock().unwrap();
                gateway.close();
                *gateway = replacement;
                Value::Null
            }
            "stop" => {
                current.lock().unwrap().close();
                Value::Null
            }
            "close" => {
                current.lock().unwrap().close();
                shutdown.cancel();
                Value::Null
            }
            _ => panic!("unknown fixture command"),
        };
        emit(json!({"id":command["id"],"result":result}));
        if command["op"] == "close" {
            break;
        }
    }
    current.lock().unwrap().close();
    shutdown.cancel();
    server.await.unwrap()?;
    Ok(())
}
