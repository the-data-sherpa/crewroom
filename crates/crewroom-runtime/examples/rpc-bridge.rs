//! Test-only JSONL bridge. The production Rust adapter owns the actual OMP process.
use crewroom_core::Result;
use crewroom_runtime::{CommandSpec, model_policy::configure_assignment, rpc::OmpRpc};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
};
fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
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
    let spec: CommandSpec = serde_json::from_value(initial["spec"].clone())?;
    let helper = PathBuf::from(initial["helper"].as_str().unwrap());
    let rpc = OmpRpc::start(&helper, &spec, emit).await?;
    emit(json!({"type":"native_ready","pid":rpc.pid,"ownershipRoot":rpc.ownership_root}));
    while let Some(line) = rx.recv().await {
        let frame: Value = serde_json::from_str(&line?)?;
        let kind = frame["type"].as_str().unwrap();
        let result = match kind {
            "native_configure" => configure_assignment(&rpc, &frame["plan"]).await,
            "native_send" => rpc.send(&frame["frame"]).await.map(|_| Value::Null),
            "native_close" => rpc.close().await.map(|_| Value::Null),
            _ => {
                rpc.command_with_id(kind, frame.clone(), frame["id"].as_str().unwrap())
                    .await
            }
        };
        match result {
            Ok(value) => emit(
                json!({"type":"response","id":frame["id"],"command":kind,"success":true,"data":value}),
            ),
            Err(error) => emit(
                json!({"type":"response","id":frame["id"],"command":kind,"success":false,"error":error.to_string()}),
            ),
        }
        if kind == "native_close" {
            return Ok(());
        }
    }
    rpc.close().await
}
