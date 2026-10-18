//! Test-only JSONL adapter for the native Mail client and disposable schema26 store.
use crewroom_core::{Result, store::Store};
use crewroom_runtime::{
    mail::{MailClient, MailError},
    messaging::{Messaging, StoreAccess},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task::JoinSet,
};
fn output(value: Value) {
    writeln!(std::io::stdout().lock(), "{}", value).unwrap();
}
fn kind(error: &MailError) -> &'static str {
    match error {
        MailError::Timeout => "timeout",
        MailError::Transport => "transport",
        MailError::Http { .. } => "http",
        MailError::Protocol => "protocol",
        MailError::Tool(_) => "tool",
        MailError::Busy => "busy",
        MailError::Configuration => "configuration",
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let config: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    let client = MailClient::with_budget(
        config["url"].as_str().unwrap(),
        config["token"].as_str().unwrap().into(),
        Duration::from_millis(config["budgetMs"].as_u64().unwrap_or(12000)),
    )
    .map_err(std::io::Error::other)?;
    let clock = Arc::new(AtomicI64::new(
        config["now"].as_i64().unwrap_or_else(crewroom_core::now_ms),
    ));
    let store = if let Some(path) = config["database"].as_str() {
        let time = clock.clone();
        Some(Arc::new(Mutex::new(
            Store::open(Path::new(path))?.with_clock(move || time.load(Ordering::SeqCst)),
        )))
    } else {
        None
    };
    let messages = if let Some(store) = &store {
        let time = clock.clone();
        Some(Messaging::with_clock(
            store.clone(),
            client.clone(),
            config["project"].as_str().unwrap().into(),
            Duration::from_secs(config["retentionSeconds"].as_u64().unwrap_or(86400)),
            move || time.load(Ordering::SeqCst),
        )?)
    } else {
        None
    };
    output(json!({"ready":true}));
    let mut jobs = JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["op"] == "close" {
            break;
        }
        let client = client.clone();
        let messages = messages.clone();
        let store = store.clone();
        let clock = clock.clone();
        jobs.spawn(async move {
            let ident = request["id"].clone();
            if request["op"] == "call" {
                let result = client
                    .call(request["name"].as_str().unwrap(), request["args"].clone())
                    .await;
                output(match result {
                    Ok(value) => json!({"id":ident,"value":value}),
                    Err(e) => json!({"id":ident,"error":e.to_string(),"kind":kind(&e)}),
                });
                return;
            }
            let result: Result<Value> = async {
                if request["op"] == "clock" {
                    clock.store(request["now"].as_i64().unwrap(), Ordering::SeqCst);
                    return Ok(Value::Null);
                }
                if request["op"] == "policy" {
                    let values: Vec<_> = request["values"].as_array().unwrap().iter().map(|v| crewroom_runtime::mail::retry_after(v.as_str(), request["now"].as_i64().unwrap()).as_millis()).collect();
                    return Ok(json!({"names":crewroom_runtime::messaging::mailbox_names(),"delays":values}));
                }
                let service = messages.as_ref().unwrap();
                let store = store.as_ref().unwrap();
                let target = request["target"].as_str().unwrap_or("");
                match request["op"].as_str().unwrap() {
                    "register" => {
                        service.register(target).await?;
                        Ok(Value::Null)
                    }
                    "send" => {
                        service
                            .send(target, request["key"].as_str().unwrap(), &request["input"])
                            .await
                    }
                    "reconcile" => service.reconcile(target).await,
                    "accept" => service.accept(target).await,
                    "observe" => Ok(json!(service.observe(target).await?)),
                    "runtime" => store.with_store(|s| s.runtime(target)),
                    "message" => store.with_store(|s| s.get_peer_message(target)),
                    "prepare" => store.with_store(|s| {
                        s.prepare_message(
                            target,
                            request["key"].as_str().unwrap(),
                            &request["input"],
                        )
                    }),
                    "accept_local" => store.with_store(|s| s.accept_message(target)),
                    "dispatch" => {
                        store.with_store(|s| s.record_dispatch(target, true))?;
                        Ok(Value::Null)
                    }
                    "ownership" => {
                        store.with_store(|s| s.record_ownership(target, true, None))?;
                        Ok(Value::Null)
                    }
                    _ => Err(crewroom_core::Error::Invalid(
                        "Unknown fixture operation".into(),
                    )),
                }
            }
            .await;
            output(match result {
                Ok(value) => json!({"id":ident,"value":value}),
                Err(e) => json!({"id":ident,"error":e.to_string(),"kind":"domain"}),
            });
        });
    }
    while let Some(result) = jobs.join_next().await {
        result.map_err(std::io::Error::other)?;
    }
    Ok(())
}
