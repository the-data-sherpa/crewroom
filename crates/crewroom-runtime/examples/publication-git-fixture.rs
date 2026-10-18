//! Disposable bare-remote driver; command effects never target operator repositories.
use crewroom_core::{Result, authority::Authority};
use crewroom_runtime::publication_git::{PublicationGit, Push, Remote, Target};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task::JoinSet,
};
fn output(value: Value) {
    writeln!(std::io::stdout().lock(), "{value}").unwrap();
}
#[tokio::main]
async fn main() -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let config: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    let authority = Arc::new(Authority::acquire_for_execution(Path::new(
        config["authority"].as_str().unwrap(),
    ))?);
    let create = || {
        PublicationGit::new(
            config["root"].as_str().unwrap().into(),
            Remote::Fixture(config["remote"].as_str().unwrap().into()),
            serde_json::from_value(config["protected"].clone())?,
            std::fs::canonicalize(
                config["helper"]
                    .as_str()
                    .unwrap_or("target/debug/crewroom-process"),
            )?,
            authority.clone(),
        )
    };
    let transport = match create() {
        Ok(git) => git,
        Err(error) => {
            output(json!({"ready":true,"error":error.to_string()}));
            return Ok(());
        }
    };
    let allowed = Arc::new(AtomicBool::new(true));
    let guards = Arc::new(AtomicUsize::new(0));
    let mut jobs = JoinSet::new();
    let mut callers = HashMap::new();
    output(json!({"ready":true}));
    while let Some(line) = lines.next_line().await? {
        let v: Value = serde_json::from_str(&line)?;
        let id = v["id"].as_str().unwrap_or("").to_owned();
        match v["op"].as_str().unwrap_or("") {
            "shutdown" => break,
            "allow" => {
                allowed.store(v["value"].as_bool().unwrap(), Ordering::SeqCst);
                output(json!({"id":id,"value":true}));
                continue;
            }
            "guards" => {
                output(json!({"id":id,"value":guards.load(Ordering::SeqCst)}));
                continue;
            }
            "cancel" => {
                if let Some(job) = callers.get(v["target"].as_str().unwrap()) {
                    let job: &tokio::task::AbortHandle = job;
                    job.abort();
                }
                output(json!({"id":id,"value":true}));
                continue;
            }
            "contend" => {
                let result = create();
                match result {
                    Ok(other) => {
                        other.close().await?;
                        output(json!({"id":id,"value":true}));
                    }
                    Err(e) => output(json!({"id":id,"error":e.to_string()})),
                };
                continue;
            }
            _ => {}
        }
        let git = transport.clone();
        let allowed = allowed.clone();
        let guards = guards.clone();
        let key = id.clone();
        let handle = jobs.spawn(async move {
            let guard = move |launch: &mut dyn FnMut() -> Result<tokio::process::Child>| {
                guards.fetch_add(1, Ordering::SeqCst);
                if !allowed.load(Ordering::SeqCst) {
                    return Err(crewroom_core::Error::Conflict(
                        "Fixture publication authorization revoked.".into(),
                    ));
                }
                launch()
            };
            let result: Result<Value> = async {
                match v["op"].as_str().unwrap_or("") {
                    "ref" => Ok(json!(git.reference(v["name"].as_str().unwrap()).await?)),
                    "push" => {
                        git.push(
                            Push {
                                directory: v["directory"].as_str().unwrap().into(),
                                head: v["head"].as_str().unwrap().into(),
                                branch: v["branch"].as_str().unwrap().into(),
                                expected_head: v["expectedHead"].as_str().map(String::from),
                            },
                            guard,
                        )
                        .await?;
                        Ok(Value::Null)
                    }
                    "fetch" => {
                        git.fetch_target(
                            Target {
                                directory: v["directory"].as_str().unwrap().into(),
                                branch: v["branch"].as_str().unwrap().into(),
                                destination: v["destination"].as_str().unwrap().into(),
                            },
                            guard,
                        )
                        .await?;
                        Ok(Value::Null)
                    }
                    "stop" => {
                        git.close().await?;
                        Ok(Value::Null)
                    }
                    _ => Err(crewroom_core::Error::Invalid(
                        "Unknown publication fixture operation.".into(),
                    )),
                }
            }
            .await;
            match result {
                Ok(value) => output(json!({"id":id,"value":value})),
                Err(error) => output(json!({"id":id,"error":error.to_string()})),
            }
        });
        callers.insert(key, handle);
    }
    let result = transport.close().await;
    while jobs.join_next().await.is_some() {}
    output(json!({"closed":true,"error":result.err().map(|e|e.to_string())}));
    Ok(())
}
