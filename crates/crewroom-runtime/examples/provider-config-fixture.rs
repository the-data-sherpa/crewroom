//! All credential commands and environment values are synthetic JSONL callbacks.
use crewroom_runtime::providers::{
    Error,
    bitbucket::BitbucketClient,
    config::{self, Clock, Command, TokenEnvironment},
    jira::JiraClient,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
    task::JoinSet,
};
type Output = Arc<tokio::sync::Mutex<tokio::io::Stdout>>;
async fn emit(output: &Output, value: Value) {
    let mut s = output.lock().await;
    s.write_all(format!("{value}\n").as_bytes()).await.unwrap();
    s.flush().await.unwrap();
}
fn error(error: Error) -> Value {
    match error {
        Error::Unavailable(v) => {
            json!({"error":v.message,"kind":"unavailable","requestSent":v.request_sent,"status":v.status})
        }
        Error::Invalid(v) => json!({"error":v,"kind":"invalid"}),
    }
}
#[derive(Clone)]
enum Connections {
    Jira(Option<config::JiraConnection>),
    Bitbucket(Vec<config::PublicationConnection>),
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let input: Value = serde_json::from_str(&lines.next_line().await?.ok_or("Missing config")?)?;
    let output = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let pending = Arc::new(Mutex::new(HashMap::<String, oneshot::Sender<Value>>::new()));
    let command: Command = {
        let pending = pending.clone();
        let output = output.clone();
        Arc::new(move |spec| {
            let pending = pending.clone();
            let output = output.clone();
            Box::pin(async move {
                let id = uuid::Uuid::new_v4().to_string();
                let (tx, rx) = oneshot::channel();
                pending.lock().unwrap().insert(id.clone(), tx);
                emit(&output,json!({"callback":id,"command":{"file":spec.file,"args":spec.args,"timeout":spec.timeout.as_millis(),"maxBuffer":spec.max_buffer}})).await;
                let value = rx
                    .await
                    .map_err(|_| Error::Invalid("Fixture command closed".into()))?;
                if value.get("error").is_some() {
                    return Err(Error::Invalid(
                        "Fixture command failed with synthetic private output".into(),
                    ));
                }
                value["stdout"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Error::Invalid("Fixture stdout missing".into()))
            })
        })
    };
    let time = Arc::new(AtomicI64::new(input["now"].as_i64().unwrap()));
    let clock: Clock = {
        let time = time.clone();
        Arc::new(move || time.load(Ordering::Acquire))
    };
    let environment = Arc::new(Mutex::new(input["environment"].as_str().map(str::to_owned)));
    let token: TokenEnvironment = {
        let environment = environment.clone();
        Arc::new(move || environment.lock().unwrap().clone())
    };
    let path = Path::new(input["path"].as_str().unwrap());
    let connections = if input["provider"] == "jira" {
        config::load_jira_connection_with(path, command, clock).map(Connections::Jira)
    } else {
        config::load_publication_connections_with(path, command, clock, token)
            .map(Connections::Bitbucket)
    };
    let connections = match connections {
        Ok(c) => c,
        Err(e) => {
            let mut v = error(e);
            v["ready"] = json!(true);
            emit(&output, v).await;
            return Ok(());
        }
    };
    let metadata=match &connections{Connections::Jira(c)=>c.as_ref().map(|c|c.source.clone()).unwrap_or(Value::Null),Connections::Bitbucket(c)=>json!(c.iter().map(|c|json!({"repositoryPath":c.repository_path,"repository":c.repository,"policy":c.policy})).collect::<Vec<_>>())};
    emit(&output, json!({"ready":true,"metadata":metadata})).await;
    let api = input["apiUrl"].as_str().unwrap().to_owned();
    let mut jobs = JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if let Some(id) = request["callback"].as_str() {
            if let Some(tx) = pending.lock().unwrap().remove(id) {
                let _ = tx.send(request);
            }
            continue;
        }
        if request["op"] == "close" {
            break;
        }
        if request["op"] == "clock" {
            time.store(request["value"].as_i64().unwrap(), Ordering::Release);
            emit(&output, json!({"id":request["id"],"value":null})).await;
            continue;
        }
        if request["op"] == "environment" {
            *environment.lock().unwrap() = request["value"].as_str().map(str::to_owned);
            emit(&output, json!({"id":request["id"],"value":null})).await;
            continue;
        }
        let connections = connections.clone();
        let output = output.clone();
        let api = api.clone();
        jobs.spawn(async move {
            let result = async {
                match connections {
                    Connections::Jira(Some(c)) => {
                        Ok(JiraClient::new(&c.source, c.credentials, Some(&api))?
                            .request("probe", reqwest::Method::GET, None, None)
                            .await?
                            .unwrap_or(Value::Null))
                    }
                    Connections::Bitbucket(c) => {
                        let c = c
                            .get(request["index"].as_u64().unwrap_or(0) as usize)
                            .ok_or_else(|| Error::Invalid("Fixture index unavailable".into()))?;
                        BitbucketClient::new(&c.repository, c.credentials.clone(), Some(&api))?
                            .verify_repository()
                            .await
                    }
                    _ => Err(Error::Invalid("Fixture connection unavailable".into())),
                }
            }
            .await;
            let mut value = match result {
                Ok(value) => json!({"value":value}),
                Err(e) => error(e),
            };
            value["id"] = request["id"].clone();
            emit(&output, value).await;
        });
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    pending.lock().unwrap().clear();
    Ok(())
}
