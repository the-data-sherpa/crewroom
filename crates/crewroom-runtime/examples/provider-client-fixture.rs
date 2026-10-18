//! Synthetic provider callbacks over JSONL. Never reads operator credentials.
use crewroom_runtime::providers::{
    BeforeDispatch, Credential, Credentials, Error, Unavailable, bitbucket::BitbucketClient,
    jira::JiraClient,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
    task::JoinSet,
};
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;
type Output = Arc<tokio::sync::Mutex<tokio::io::Stdout>>;
async fn emit(output: &Output, value: Value) {
    let mut output = output.lock().await;
    output
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
    output.flush().await.unwrap();
}
async fn callback(
    pending: &Pending,
    output: &Output,
    kind: &str,
    request: Option<String>,
) -> Result<Value, Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    pending.lock().unwrap().insert(id.clone(), tx);
    emit(output, json!({"callback":id,"kind":kind,"request":request})).await;
    let value = rx
        .await
        .map_err(|_| Error::Invalid("Fixture callback closed.".into()))?;
    if value.get("error").is_some() {
        return Err(Error::Unavailable(Unavailable {
            message: if kind == "credentials" {
                "Fixture credentials unavailable."
            } else {
                "Fixture dispatch refused."
            }
            .into(),
            status: None,
            request_sent: false,
        }));
    }
    Ok(value["value"].clone())
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
enum Client {
    Jira(JiraClient),
    Bitbucket(BitbucketClient),
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let pending: Pending = Arc::default();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let config: Value =
        serde_json::from_str(&lines.next_line().await?.ok_or("Missing fixture config")?)?;
    let credentials: Credentials = {
        let pending = pending.clone();
        let output = output.clone();
        Arc::new(move || {
            let pending = pending.clone();
            let output = output.clone();
            Box::pin(async move {
                let value = callback(&pending, &output, "credentials", None).await?;
                if let Some(token) = value.as_str() {
                    return Ok(Credential::Bearer(token.into()));
                }
                if value["scheme"] != "basic" {
                    return Err(Error::Unavailable(Unavailable {
                        message: "Fixture credentials unavailable.".into(),
                        status: None,
                        request_sent: false,
                    }));
                }
                Ok(Credential::Basic {
                    username: value["username"].as_str().unwrap_or("").into(),
                    token: value["token"].as_str().unwrap_or("").into(),
                })
            })
        })
    };
    let client = if config["provider"] == "jira" {
        JiraClient::new(&config["source"], credentials, config["apiUrl"].as_str()).map(Client::Jira)
    } else {
        BitbucketClient::new(
            &config["repository"],
            credentials,
            config["apiUrl"].as_str(),
        )
        .map(Client::Bitbucket)
    };
    let client = match client {
        Ok(c) => c,
        Err(e) => {
            let mut e = error(e);
            e["ready"] = json!(true);
            emit(&output, e).await;
            return Ok(());
        }
    };
    emit(&output, json!({"ready":true})).await;
    let mut jobs = JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if let Some(id) = request["callback"].as_str() {
            let sender = pending.lock().unwrap().remove(id);
            if let Some(sender) = sender {
                let _ = sender.send(request);
            }
            continue;
        }
        if request["op"] == "close" {
            break;
        }
        let client = client.clone();
        let output = output.clone();
        let pending = pending.clone();
        jobs.spawn(async move {
            let before: Option<BeforeDispatch> = (request["before"] == true).then(|| {
                let output = output.clone();
                let id = request["id"].as_str().unwrap().to_owned();
                Arc::new(move || {
                    let output = output.clone();
                    let pending = pending.clone();
                    let id = id.clone();
                    Box::pin(async move {
                        callback(&pending, &output, "before", Some(id)).await?;
                        Ok(())
                    })
                        as futures_util::future::BoxFuture<'static, Result<(), Error>>
                }) as BeforeDispatch
            });
            let input = &request["input"];
            let id = input["id"].as_str().unwrap_or("");
            let number = input["id"].as_i64().unwrap_or(0);
            let result = async {
                match &client {
                    Client::Jira(c) => match request["op"].as_str().unwrap() {
                        "source" => Ok(c.source.clone()),
                        // An explicit `jql`, or the query the source's saved
                        // filter and assignee imply (FOR-53).
                        "search" => {
                            let jql = input["jql"].as_str().map(str::to_owned).or_else(|| {
                                crewroom_core::jira_projects::workspace_source(&c.source)
                                    .and_then(|s| crewroom_core::jira_projects::jql(&s))
                            });
                            Ok(json!(c.search(&jql.unwrap_or_default()).await?))
                        }
                        "issue" => c.issue(id).await,
                        "comments" => Ok(json!(c.comments(id).await?)),
                        "transitions" => Ok(json!(c.transitions(id).await?)),
                        "addComment" => {
                            c.add_comment(id, input["body"].clone(), before.as_ref())
                                .await
                        }
                        "transition" => {
                            c.transition(
                                id,
                                input["transitionId"].as_str().unwrap(),
                                before.as_ref(),
                            )
                            .await?;
                            Ok(Value::Null)
                        }
                        "request" => {
                            let value = c
                                .request(
                                    input["path"].as_str().unwrap(),
                                    input["method"].as_str().unwrap_or("GET").parse().unwrap(),
                                    input.get("body").cloned(),
                                    before.as_ref(),
                                )
                                .await?;
                            Ok(json!({"present":value.is_some(),"value":value}))
                        }
                        _ => Err(Error::Invalid("Unknown Jira fixture operation.".into())),
                    },
                    Client::Bitbucket(c) => match request["op"].as_str().unwrap() {
                        "repository" => Ok(c.repository.clone()),
                        "verifyRepository" => c.verify_repository().await,
                        "pullRequest" => c.pull_request(number).await,
                        "pullRequests" => Ok(json!(
                            c.pull_requests(
                                input["branch"].as_str().unwrap(),
                                input["target"].as_str().unwrap()
                            )
                            .await?
                        )),
                        "comments" => c.comments(number).await,
                        "observe" => c.observe(number).await,
                        "createPullRequest" => c.create_pull_request(input, before.as_ref()).await,
                        "updatePullRequest" => {
                            c.update_pull_request(number, input, before.as_ref()).await
                        }
                        _ => Err(Error::Invalid(
                            "Unknown Bitbucket fixture operation.".into(),
                        )),
                    },
                }
            }
            .await;
            let mut response = match result {
                Ok(value) => json!({"value":value}),
                Err(e) => error(e),
            };
            response["id"] = request["id"].clone();
            emit(&output, response).await;
        });
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    pending.lock().unwrap().clear();
    Ok(())
}
