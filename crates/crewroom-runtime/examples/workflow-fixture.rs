//! Disposable full workflow driver. The caller supplies isolated SQLite/config
//! and loopback transports; process ownership and all service logic are native.
use crewroom_core::{Result, authority::Authority, store::Store};
use crewroom_runtime::{
    evidence::{Checks, Evidence, Research},
    github_delivery::GitHubDelivery,
    jira_delivery::JiraDelivery,
    messaging::StoreAccess,
    providers::{Credential, Credentials, bitbucket::BitbucketClient, jira::JiraClient},
    publication::Publication,
    publication_remote::{Connection, PublicationRemote},
    repository::Repository,
    supervisor::{
        tasks::Reply,
        workflow::{Services, Workflow, WorkflowOptions, configuration::RuntimeConfig},
    },
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task::{AbortHandle, JoinSet},
};
fn output(v: Value) {
    writeln!(std::io::stdout().lock(), "{v}").unwrap();
}
fn loopback(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|v| v.starts_with("http://127.0.0.1:"))
        .ok_or_else(|| std::io::Error::other("Fixture provider requires loopback HTTP").into())
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
    let helper = std::fs::canonicalize("target/debug/crewroom-process")?;
    let work = Repository::new(
        store.clone(),
        config["repositories"].as_str().unwrap().into(),
        helper.clone(),
        authority.clone(),
    )?;
    let evidence = Evidence::new(
        work.clone(),
        Path::new(config["authority"].as_str().unwrap()).join("artifacts"),
    )?;
    let research = Research::new(evidence.clone());
    let jira = config
        .get("jira")
        .map(|settings| -> Result<_> {
            let credentials: Credentials = Arc::new(|| {
                Box::pin(async { Ok(Credential::Bearer("synthetic-jira-token".into())) })
            });
            let endpoint = loopback(&settings["apiUrl"])?;
            let client = JiraClient::new(&settings["source"], credentials, Some(endpoint))
                .map_err(std::io::Error::other)?;
            Ok(JiraDelivery::new(
                store.clone(),
                client,
                Some(evidence.clone()),
                Some(research.clone()),
            ))
        })
        .transpose()?;
    // GitHub issue delivery (FOR-48) against a loopback GitHub REST fixture.
    let github = config
        .get("github")
        .map(|settings| -> Result<_> {
            let credentials: Credentials = Arc::new(|| {
                Box::pin(async { Ok(Credential::Bearer("synthetic-github-token".into())) })
            });
            Ok(GitHubDelivery::new(
                store.clone(),
                credentials,
                Some(loopback(&settings["apiUrl"])?.into()),
                Some(evidence.clone()),
                Some(research.clone()),
            ))
        })
        .transpose()?;
    let publication = config
        .get("publicationRemote")
        .map(|settings| -> Result<_> {
            let connections = settings["connections"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| -> Result<_> {
                    let credentials: Credentials = Arc::new(|| {
                        Box::pin(async {
                            Ok(Credential::Bearer("synthetic-bitbucket-token".into()))
                        })
                    });
                    let client = BitbucketClient::new(
                        &item["repository"],
                        credentials,
                        Some(loopback(&item["apiUrl"])?),
                    )
                    .map_err(std::io::Error::other)?;
                    Connection::new(
                        Path::new(item["repositoryPath"].as_str().unwrap()),
                        &item["repository"],
                        client,
                        &item["policy"],
                        Some(Path::new(item["fixtureGitPath"].as_str().unwrap())),
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            let remote = PublicationRemote::new(
                store.clone(),
                Path::new(config["authority"].as_str().unwrap()).join("publication"),
                helper.clone(),
                authority.clone(),
                connections,
            )?;
            remote.install_observers(&work);
            Ok(Publication::new(remote, work.clone(), evidence.clone()))
        })
        .transpose()?;
    let checks = config["runtime"]["commandEngine"]["socket"]
        .as_str()
        .map(|socket| {
            Checks::new(
                evidence.clone(),
                PathBuf::from(config["runtime"]["root"].as_str().unwrap()),
                socket.into(),
            )
        })
        .transpose()?;
    let changed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let notifications = changed.clone();
    let workflow = Workflow::new(
        store.clone(),
        authority,
        WorkflowOptions {
            runtime: RuntimeConfig::parse(&config["runtime"])?,
            helper,
            host: std::fs::canonicalize("target/debug/crewroom-host")?,
            tmux: std::fs::canonicalize(config["tmux"].as_str().unwrap_or(
                if cfg!(target_os = "macos") {
                    "/opt/homebrew/bin/tmux"
                } else {
                    "/usr/bin/tmux"
                },
            ))?,
            backend: config["backend"].as_str().unwrap().into(),
            roles: config["roles"].as_str().unwrap().into(),
            environment: config["environment"].clone(),
        },
        Services {
            repository: work.clone(),
            evidence: evidence.clone(),
            checks: checks.clone(),
            research: research.clone(),
            publication: publication.clone(),
            jira: jira.clone(),
            github: github.clone(),
        },
        move |_| {
            notifications.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
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
            if let Some(handle) = r["requestId"].as_str().and_then(|id| cancellations.get(id)) {
                handle.abort();
                output(json!({"id":r["requestId"],"error":"Fixture waiter cancelled."}));
            }
            output(json!({"id":r["id"],"value":null}));
            continue;
        }
        let github = github.clone();
        let (workflow, store, work, evidence, checks, research, publication, jira, changed) = (
            workflow.clone(),
            store.clone(),
            work.clone(),
            evidence.clone(),
            checks.clone(),
            research.clone(),
            publication.clone(),
            jira.clone(),
            changed.clone(),
        );
        let id = r["id"].as_str().unwrap().to_owned();
        let job = jobs.spawn(async move {
            let target = r["target"].as_str().unwrap_or("");
            let result: Reply = async {
                Ok(match r["op"].as_str().unwrap_or("") {
                    "plan_source" => {
                        let preflight = store.with_store(|s| {
                            s.prepare_plan_source(target, r["input"]["revision"].as_u64().unwrap())
                        })?;
                        let pinned = tokio::task::spawn_blocking(move || preflight.validate())
                            .await
                            .map_err(|_| {
                                crewroom_core::Error::Conflict(
                                    "Source validation interrupted.".into(),
                                )
                            })??;
                        store.with_store(|s| s.save_plan_source(&pinned))?
                    }
                    "plan_save" => {
                        store.with_store(|s| s.save_implementation_plan(target, &r["input"]))?
                    }
                    "plan_decide" => {
                        store.with_store(|s| s.decide_implementation_plan(target, &r["input"]))?
                    }
                    "start" => {
                        workflow
                            .start(
                                target,
                                r["input"].clone(),
                                r["roles"].as_array().unwrap().clone(),
                            )?
                            .wait()
                            .await?
                    }
                    "refine" => workflow.start_refinement(target)?.wait().await?,
                    "pause" => {
                        workflow
                            .pause(target, r["preserve"] == true, r["reason"].as_str())?
                            .wait()
                            .await?
                    }
                    "cancel" => workflow.cancel(target)?.wait().await?,
                    "resume" => workflow.resume(target)?.wait().await?,
                    "initialize" => workflow.initialize()?.wait().await?,
                    "close_runtime" => workflow.close()?.wait().await?,
                    "tool" => {
                        workflow
                            .tool(
                                r["route"].as_str().unwrap(),
                                r["authorization"].as_str().unwrap_or(""),
                                r["input"].clone(),
                            )
                            .await?
                    }
                    "insert" => workflow.insert_specialist(target, &r["input"])?,
                    "snapshot" => workflow.snapshot(target)?,
                    "all" => store.with_store(|s| s.snapshot())?,
                    "runtime" => store.with_store(|s| s.runtime(target))?,
                    "steering" => json!(store.with_store(|s| s.steering(target))?),
                    "events" => json!(store.with_store(|s| s.agent_events(target))?),
                    "run" => store.with_store(|s| s.get_run(target))?,
                    "steer" => {
                        let value = store.with_store(|s| s.steer(target, &r["input"]))?;
                        workflow.notify(target);
                        value
                    }
                    "notify" => {
                        workflow.notify(target);
                        Value::Null
                    }
                    "busy" => json!(workflow.busy(target)),
                    "changes" => json!(changed.load(std::sync::atomic::Ordering::SeqCst)),
                    "prepare" => work.prepare(target, &r["input"]).await?,
                    "configure" => evidence.configure(target, &r["input"])?,
                    "evidence" => evidence.assess(target).await?,
                    "record" => store
                        .with_store(|s| s.coding_record(target))?
                        .unwrap_or(Value::Null),
                    "task" => store.with_store(|s| s.get_task(target))?,
                    "research" => research.read(target)?,
                    "jira_updates" => json!(store.with_store(|s| s.jira_updates(Some(target)))?),
                    // Imports one GitHub issue as the ticket, assigns the saved
                    // repository, accepts the brief and queues pinned research.
                    "github_seed" => store.with_store(|s| {
                        // The project confirms GitHub Issues as its ticket source (FOR-45).
                        let project = r["input"]["repositoryId"].as_str().unwrap_or_default().to_owned();
                        let repository = r["input"]["repository"].as_str().unwrap_or_default();
                        s.confirm_ticket_source(
                            &project,
                            &json!({"tickets":"github","repository":repository}),
                            &crewroom_core::project_source::observe(Some(&format!("git@github.com:{repository}.git"))),
                        )?;
                        let saved = s.github_source(&project)?.ok_or_else(|| crewroom_core::Error::Conflict("GitHub source was not confirmed.".into()))?;
                        s.commit_github_sync(&project, &saved["source"], &json!([r["input"]["issue"]]), &json!([]), &json!([]))?;
                        let task = s.snapshot()?["tasks"]
                            .as_array()
                            .and_then(|tasks| tasks.first())
                            .cloned()
                            .ok_or_else(|| crewroom_core::Error::Conflict("GitHub issue was not imported.".into()))?;
                        let id = task["id"].as_str().unwrap_or_default().to_owned();
                        let revision = |s: &Store| -> Result<Value> { Ok(s.get_task(&id)?["revision"].clone()) };
                        s.assign_repository(&id, &json!({"revision":revision(s)?,"repositoryId":r["input"]["repositoryId"]}))?;
                        s.refine(&id, &json!({"revision":revision(s)?,"sourceRevision":1,"brief":"Preserve workflow boundaries","criteria":["Retain independently verified evidence"]}))?;
                        let run = s.queue_research(&id, &json!({"revision":revision(s)?,"requestId":uuid::Uuid::new_v4().to_string()}))?;
                        Ok(json!({"task":s.get_task(&id)?,"run":run}))
                    })?,
                    "github_updates" => json!(store.with_store(|s| s.github_updates(Some(target)))?),
                    "github_flush" => {
                        github.as_ref().unwrap().flush().await?;
                        workflow.notify(target);
                        Value::Null
                    }
                    "jira_flush" => {
                        jira.as_ref().unwrap().flush().await?;
                        workflow.notify(target);
                        Value::Null
                    }
                    "publication" => store
                        .with_store(|s| s.publication(target))?
                        .unwrap_or(Value::Null),
                    "publication_flush" => {
                        publication.as_ref().unwrap().flush().await?;
                        workflow.notify(target);
                        Value::Null
                    }
                    "checks" => {
                        checks
                            .as_ref()
                            .unwrap()
                            .execute(target, r["key"].as_str().unwrap(), &r["input"])
                            .await?
                    }
                    "limit" => store.with_store(|s| s.set_active_limit(target, &r["input"]))?,
                    "allow" => store.with_store(|s| {
                        s.grant_repair_cycle(target, r["assignment"].as_str().unwrap(), &r["input"])
                    })?,
                    "adopt" => store.with_store(|s| s.adopt_requirements(target, &r["input"]))?,
                    "operator_hold" => {
                        let root = PathBuf::from(r["root"].as_str().unwrap());
                        workflow
                            .operator_transition(target, move || async move {
                                tokio::fs::write(root.join("waiting"), b"waiting").await?;
                                let started = std::time::Instant::now();
                                while !root.join("release").exists() {
                                    if started.elapsed() > std::time::Duration::from_secs(20) {
                                        return Err(crewroom_core::Error::Conflict(
                                            "Fixture lane timed out.".into(),
                                        ));
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                                }
                                Ok(Value::Null)
                            })?
                            .wait()
                            .await?
                    }
                    _ => {
                        return Err(Arc::new(crewroom_core::Error::Invalid(
                            "Unknown workflow fixture operation.".into(),
                        )));
                    }
                })
            }
            .await;
            output(match result {
                Ok(value) => json!({"id":r["id"],"value":value}),
                Err(e) => json!({"id":r["id"],"error":e.to_string()}),
            });
        });
        cancellations.insert(id, job);
    }
    let closed = workflow
        .close()?
        .wait()
        .await
        .map_err(|e| crewroom_core::Error::Conflict(e.to_string()))?;
    while let Some(result) = jobs.join_next().await {
        if let Err(e) = result
            && !e.is_cancelled()
        {
            return Err(std::io::Error::other(e).into());
        }
    }
    if closed["stopped"] != true {
        return Err(crewroom_core::Error::Conflict(format!(
            "Workflow fixture retained unresolved cleanup: {closed}"
        )));
    }
    Ok(())
}
