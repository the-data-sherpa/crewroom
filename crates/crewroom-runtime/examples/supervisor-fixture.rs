//! Disposable lifecycle driver. Assignment/model inputs are explicit fixtures;
//! native processes, SQLite identities, tmux and cleanup are real.
use crewroom_core::{Result, authority::Authority, store::Store};
use crewroom_runtime::{
    messaging::StoreAccess,
    supervisor::{HostManager, HostOptions, client::HostClient, tasks::Reply},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task::{AbortHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;
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
    let store = Arc::new(Mutex::new(Store::open(Path::new(
        config["database"].as_str().unwrap(),
    ))?));
    let runtime = Path::new(config["root"].as_str().unwrap());
    let manager = HostManager::new(
        store.clone(),
        authority,
        HostOptions {
            root: runtime.into(),
            helper: std::fs::canonicalize("target/debug/crewroom-process")?,
            host: std::fs::canonicalize("target/debug/crewroom-host")?,
            tmux: std::fs::canonicalize(config["tmux"].as_str().unwrap_or(
                if cfg!(target_os = "linux") {
                    "/usr/bin/tmux"
                } else {
                    "/opt/homebrew/bin/tmux"
                },
            ))?,
            socket: runtime.join("tmux.sock"),
            backend: config["backend"].as_str().unwrap().into(),
            command_engine: config["socket"].as_str().map(Into::into),
        },
        |_| {},
    )?;
    output(json!({"ready":true}));
    let mut jobs = JoinSet::new();
    let mut cancellations: HashMap<String, AbortHandle> = HashMap::new();
    while let Some(line) = lines.next_line().await? {
        let r: Value = serde_json::from_str(&line)?;
        if r["op"] == "close" {
            break;
        }
        if r["op"] == "cancel_waiter" {
            if let Some(id) = r["requestId"].as_str()
                && let Some(job) = cancellations.get(id)
            {
                job.abort();
                output(json!({"id":id,"error":"Fixture waiter cancelled."}));
            }
            output(json!({"id":r["id"],"value":null}));
            continue;
        }
        let manager = manager.clone();
        let store = store.clone();
        let id = r["id"].as_str().unwrap().to_owned();
        let job=jobs.spawn(async move {
            let target=r["target"].as_str().unwrap_or("");
            let result:Reply=async {Ok(match r["op"].as_str().unwrap_or("") {
                "seed"=>store.with_store(|s| {
                    let task=s.create_task(&json!({"title":r["title"],"description":""}))?;
                    let task_id=task["id"].as_str().unwrap();
                    s.refine(task_id,&json!({"revision":1,"sourceRevision":1,"brief":"Supervise the assigned offline runner","criteria":["Retain independent process identity"]}))?;
                    let run=s.queue_research(task_id,&json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()}))?;
                    let run_id=run["id"].as_str().unwrap();let run=s.set_desired_state(run_id,"running")?;
                    let admitted=s.admit_assignment(run_id,&r["input"])?;
                    Ok(json!({"task":task,"run":run,"assignment":admitted["assignment"],"attempt":admitted["attempt"]}))
                })?,
                "launch"=>manager.launch(target,r["revision"].as_u64().unwrap(),r["config"].clone())?.wait().await?,
                "reattach"=>manager.reattach(target)?.wait().await?,
                "stop"=>manager.stop_run(target)?.wait().await?,
                "close_runtime"=>manager.close()?.wait().await?,
                "request"=>manager.request(target,&r["input"]).await?,
                "owned"=>manager.owned_state(&store.with_store(|s|s.get_attempt(target))?).await?,
                "terminal"=>manager.terminal(r["runId"].as_str().unwrap_or(""), target).await?,
                "observation"=>manager.observation(target),
                "auth"=>json!({"authorized":manager.authenticates(target,r["authorization"].as_str().unwrap_or("")),"observed":manager.observes(target)}),
                "busy"=>json!(manager.busy(target)),
                "pane_exit"=>{manager.pane_exited(target)?;Value::Null},
                "runtime"=>store.with_store(|s|s.runtime(target))?,
                "snapshot"=>store.with_store(|s|s.snapshot())?,
                "attempt"=>store.with_store(|s|s.get_attempt(target))?,
                "activate"=>store.with_store(|s|s.start_execution(target))?,
                "desired"=>store.with_store(|s|s.set_desired_state(target,r["state"].as_str().unwrap()))?,
                "pause"=>store.with_store(|s|s.begin_pause(target,None))?,
                "finish_pause"=>store.with_store(|s|s.finish_pause(target))?,
                "events"=>json!(store.with_store(|s|s.agent_events(target))?),
                "eventpage"=>store.with_store(|s|s.agent_event_page(target,r["after"].as_u64().unwrap_or(0),200))?,
                "replace"=>store.with_store(|s|s.replace_attempt(target,&json!({"automatic":false})))?,
                "continue"=>store.with_store(|s|s.prepare_continuation(target,s.get_run(target)?["controlRevision"].as_u64().unwrap()))?,
                "client"=>HostClient::new(r["url"].as_str().unwrap(),r["token"].as_str().unwrap())?.request(&r["input"],&CancellationToken::new()).await?,
                _=>return Err(Arc::new(crewroom_core::Error::Invalid("Unknown fixture operation.".into()))),
            })}.await;
            output(match result {Ok(value)=>json!({"id":r["id"],"value":value}),Err(error)=>json!({"id":r["id"],"error":error.to_string()})});
        });
        cancellations.insert(id, job);
    }
    let result = manager
        .close()?
        .wait()
        .await
        .map_err(|e| crewroom_core::Error::Conflict(e.to_string()))?;
    if result["stopped"] != true {
        return Err(crewroom_core::Error::Conflict(
            "Fixture retained unresolved process ownership.".into(),
        ));
    }
    while let Some(result) = jobs.join_next().await {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            return Err(std::io::Error::other(error).into());
        }
    }
    Ok(())
}
