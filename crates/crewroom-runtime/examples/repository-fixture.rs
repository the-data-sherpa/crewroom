//! Disposable native repository driver. Fixture agent identities are records only.
use crewroom_core::{Result, authority::Authority, store::Store};
use crewroom_runtime::{
    evidence::{Checks, Evidence, Research},
    jira_delivery::JiraDelivery,
    jira_sync::JiraSync,
    messaging::StoreAccess,
    providers::{Credential, Credentials, Error as ProviderError, Unavailable, jira::JiraClient},
    publication::Publication,
    publication_remote::{Connection, PublicationRemote},
    repository::Repository,
    target_refresh::TargetRefresh,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
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
    let store = Arc::new(Mutex::new(Store::open(Path::new(
        config["database"].as_str().unwrap(),
    ))?));
    let work = Repository::new(
        store.clone(),
        config["root"].as_str().unwrap().into(),
        std::fs::canonicalize(
            config["helper"]
                .as_str()
                .unwrap_or("target/debug/crewroom-process"),
        )?,
        authority.clone(),
    )?;
    if let Some(root) = config["targetObserver"].as_str() {
        let root = std::path::PathBuf::from(root);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        work.set_target_observer(move |_| {
            let root = root.clone();
            let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            async move {
                tokio::fs::write(root.join("called"), b"called").await?;
                tokio::fs::write(root.join(format!("called-{call}")), b"called").await?;
                let started = std::time::Instant::now();
                while (root.join("hold").exists() && !root.join("release").exists())
                    || (root.join(format!("hold-{call}")).exists()
                        && !root.join(format!("release-{call}")).exists())
                {
                    if started.elapsed() > std::time::Duration::from_secs(10) {
                        return Err(crewroom_core::Error::Invalid(
                            "Scripted target observer timed out".into(),
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                let value: Value = serde_json::from_slice(
                    &tokio::fs::read(if root.join(format!("result-{call}.json")).exists() {
                        root.join(format!("result-{call}.json"))
                    } else {
                        root.join("result.json")
                    })
                    .await?,
                )?;
                if value["error"] == true {
                    return Err(crewroom_core::Error::Invalid(
                        "Scripted target observation unavailable".into(),
                    ));
                }
                Ok(value["head"].as_str().map(String::from))
            }
        });
    }
    if let Some(root) = config["continuationObserver"].as_str() {
        let root = std::path::PathBuf::from(root);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        work.set_continuation_observer(move |id| {
            let root = root.clone();
            let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            async move {
                tokio::fs::write(root.join("called"), id).await?;
                tokio::fs::write(root.join(format!("called-{call}")), b"called").await?;
                let started = std::time::Instant::now();
                while root.join("hold").exists() && !root.join("release").exists() {
                    if started.elapsed() > std::time::Duration::from_secs(10) {
                        return Err(crewroom_core::Error::Invalid(
                            "Scripted continuation observer timed out".into(),
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                let value: Value =
                    serde_json::from_slice(&tokio::fs::read(root.join("result.json")).await?)?;
                if value["verified"] != true {
                    return Err(crewroom_core::Error::Invalid(
                        "Scripted PR verification unavailable or refused".into(),
                    ));
                }
                Ok(())
            }
        });
    }
    let evidence = Evidence::new(
        work.clone(),
        Path::new(config["authority"].as_str().unwrap()).join("artifacts"),
    )?;
    let research = Research::new(evidence.clone());
    let checks = Checks::new(
        evidence.clone(),
        Path::new(config["authority"].as_str().unwrap()).join("runtime"),
        // Resolved by scripts/rust/docker-engine.ts (repository-adapter.ts); no platform guess here.
        config["socket"]
            .as_str()
            .map(std::path::PathBuf::from)
            .expect("repository-fixture requires a Docker socket path in its configuration"),
    )?;
    // Optional local HTTP provider; credentials are fixed synthetic fixture data.
    let credential_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let jira = if let Some(config) = config.get("jira") {
        let count = credential_calls.clone();
        let root = config["credentialRoot"]
            .as_str()
            .map(std::path::PathBuf::from);
        let credentials: Credentials = Arc::new(move || {
            let count = count.clone();
            let root = root.clone();
            Box::pin(async move {
                let call = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let error = || {
                    ProviderError::Unavailable(Unavailable {
                        message: "Fixture credentials unavailable.".into(),
                        status: None,
                        request_sent: false,
                    })
                };
                if let Some(root) = root {
                    let rules: Value = serde_json::from_slice(
                        &tokio::fs::read(root.join("rules.json"))
                            .await
                            .map_err(|_| error())?,
                    )
                    .map_err(|_| error())?;
                    tokio::fs::write(root.join(format!("entered-{call}")), b"entered")
                        .await
                        .map_err(|_| error())?;
                    if rules["holdAt"] == call {
                        let start = std::time::Instant::now();
                        while !root.join(format!("release-{call}")).exists() {
                            if start.elapsed() > std::time::Duration::from_secs(30) {
                                return Err(error());
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        }
                    }
                    if rules["failAt"] == call {
                        return Err(error());
                    }
                }
                Ok(Credential::Bearer("synthetic-jira-token".into()))
            })
        });
        let endpoint = config["apiUrl"]
            .as_str()
            .filter(|s| s.starts_with("http://127.0.0.1:"))
            .ok_or_else(|| std::io::Error::other("Jira fixture requires loopback HTTP"))?;
        let client = JiraClient::new(&config["source"], credentials, Some(endpoint))
            .map_err(std::io::Error::other)?;
        Some((
            JiraSync::new(store.clone(), client.clone()),
            JiraDelivery::new(
                store.clone(),
                client,
                (config["evidence"] != false).then(|| evidence.clone()),
                (config["research"] != false).then(|| research.clone()),
            ),
        ))
    } else {
        None
    };
    let bb_credential_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let remote = if let Some(settings) = config.get("publicationRemote") {
        let mut connections = Vec::new();
        for item in settings["connections"].as_array().unwrap() {
            let endpoint = item["apiUrl"]
                .as_str()
                .filter(|s| s.starts_with("http://127.0.0.1:"))
                .ok_or_else(|| {
                    std::io::Error::other("Publication fixture requires loopback HTTP")
                })?;
            let count = bb_credential_calls.clone();
            let root = settings["credentialRoot"]
                .as_str()
                .map(std::path::PathBuf::from);
            let credentials: Credentials = Arc::new(move || {
                let count = count.clone();
                let root = root.clone();
                Box::pin(async move {
                    let call = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let error = || {
                        ProviderError::Unavailable(Unavailable {
                            message: "Fixture credentials unavailable.".into(),
                            status: None,
                            request_sent: false,
                        })
                    };
                    if let Some(root) = root {
                        let rules: Value = serde_json::from_slice(
                            &tokio::fs::read(root.join("rules.json"))
                                .await
                                .map_err(|_| error())?,
                        )
                        .map_err(|_| error())?;
                        tokio::fs::write(root.join(format!("entered-{call}")), b"entered")
                            .await
                            .map_err(|_| error())?;
                        if rules["holdAt"] == call {
                            let start = std::time::Instant::now();
                            while !root.join(format!("release-{call}")).exists() {
                                if start.elapsed() > std::time::Duration::from_secs(30) {
                                    return Err(error());
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                            }
                        }
                        if rules["failAt"] == call {
                            return Err(error());
                        }
                    }
                    Ok(Credential::Bearer("synthetic-bitbucket-token".into()))
                })
            });

            let client = crewroom_runtime::providers::bitbucket::BitbucketClient::new(
                &item["repository"],
                credentials,
                Some(endpoint),
            )
            .map_err(std::io::Error::other)?;
            connections.push(Connection::new(
                Path::new(item["repositoryPath"].as_str().unwrap()),
                &item["repository"],
                client,
                &item["policy"],
                Some(Path::new(item["fixtureGitPath"].as_str().unwrap())),
            )?);
        }
        let remote = PublicationRemote::new(
            store.clone(),
            Path::new(config["authority"].as_str().unwrap()).join("publication"),
            std::fs::canonicalize(
                config["helper"]
                    .as_str()
                    .unwrap_or("target/debug/crewroom-process"),
            )?,
            authority.clone(),
            connections,
        )?;
        remote.install_observers(&work);
        Some((
            remote.clone(),
            TargetRefresh::new(work.clone(), evidence.clone(), remote.clone()),
            Publication::new(remote, work.clone(), evidence.clone()),
        ))
    } else {
        None
    };
    output(json!({"ready":true}));
    let mut jobs = JoinSet::new();
    let mut cancellations = std::collections::HashMap::<String, tokio::task::AbortHandle>::new();
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
        let work = work.clone();
        let evidence = evidence.clone();
        let checks = checks.clone();
        let research = research.clone();
        let store = store.clone();
        let authority = authority.clone();
        let jira = jira.clone();
        let remote = remote.clone();
        let credential_calls = credential_calls.clone();
        let bb_credential_calls = bb_credential_calls.clone();
        let request_id = r["id"].as_str().unwrap().to_owned();
        let handle=jobs.spawn(async move {let result:Result<Value>=async {authority.assert_current()?;let target=r["target"].as_str().unwrap_or("");let name=r["path"].as_str().unwrap_or("");let key=r["key"].as_str().unwrap_or("");match r["op"].as_str().unwrap_or("") {
            "snapshot"=>store.with_store(|s|s.snapshot()),
            "repository_stop"=>work.stop_run(target)?.wait().await.map_err(|e|crewroom_core::Error::Conflict(e.to_string())),
            "repository_close"=>work.close()?.wait().await.map_err(|e|crewroom_core::Error::Conflict(e.to_string())),
            "repository_busy"=>Ok(json!(work.mutations_busy(target))),
            "target_request"=>remote.as_ref().unwrap().1.refresh(target,&r["input"]).await,
            "target_recover"=>{remote.as_ref().unwrap().1.reconcile().await?;Ok(Value::Null)},
            "claim_slot"=>store.with_store(|s|Ok(json!(s.claim_next_coding_slot()?))),
            "release_slot"=>store.with_store(|s|s.release_coding_slot(target,r["reason"].as_str().unwrap_or("review"),r["candidateId"].as_str())),
            "reclaim_slot"=>{store.with_store(|s|s.reclaim_coding_slot(target,false))?;Ok(Value::Null)},
            "queue_feedback"=>store.with_store(|s|Ok(json!(s.queue_feedback_resume(target)?))),
            "resume_state"=>store.with_store(|s|s.set_resume_request_state(target,r["input"].as_str().unwrap())),
            "feedback_handoff"=>store.with_store(|s|Ok(json!(s.mark_feedback_handoff(target,key)?))),
            "clear_resume"=>store.with_store(|s|s.clear_resume_request(target,"held")),
            "publication_prepare"=>remote.as_ref().unwrap().2.prepare(target,&r["input"]).await,
            "publication_view"=>remote.as_ref().unwrap().2.view(target).await,
            "publication_feedback"=>Ok(json!(remote.as_ref().unwrap().2.observe_feedback(target).await?)),
            "publication_flush"=>{remote.as_ref().unwrap().2.flush().await?;Ok(Value::Null)},
            "publication_close"=>{remote.as_ref().unwrap().2.close().await?;Ok(Value::Null)},
            "publication_credentials"=>Ok(json!(bb_credential_calls.load(std::sync::atomic::Ordering::SeqCst))),
            "remote_fetch"=>Ok(json!(remote.as_ref().unwrap().0.fetch_target(target,key,r.get("supersedes").cloned()).await?)),
            "remote_observe"=>{let work=store.with_store(|s|s.repository_work(target))?.unwrap();Ok(json!(remote.as_ref().unwrap().0.observe_target(work).await?))},
            "remote_verify"=>{remote.as_ref().unwrap().0.verify_cancelled_continuation(target).await?;Ok(Value::Null)},
            "remote_stop"=>{remote.as_ref().unwrap().0.stop_run(target).await?;Ok(Value::Null)},
            "remote_close"=>{remote.as_ref().unwrap().0.close().await?;Ok(Value::Null)},
            // Jira refresh is per project (FOR-53): a target names the project;
            // without one, every project using the connected site refreshes and
            // the snapshot is the workspace-level setting.
            // Confirms the fixture site as a project's ticket source and offers
            // the connection's saved filter, as the service does at start.
            "jira_project" if target.is_empty()=>jira.as_ref().unwrap().0.offer_workspace_setting(),
            "jira_project"=>{let site=r["site"].as_str().unwrap_or("https://fixture.atlassian.net");store.with_store(|s|s.confirm_ticket_source(target,&json!({"tickets":"jira","site":site}),&crewroom_core::project_source::observe(None)))?;jira.as_ref().unwrap().0.offer_workspace_setting()?;store.with_store(|s|Ok(s.jira_project_source(target)?.unwrap_or(Value::Null)))},
            "jira_snapshot"=>if target.is_empty() {store.with_store(|s|Ok(s.jira_state()?.unwrap_or(Value::Null)))} else {store.with_store(|s|Ok(s.jira_project_source(target)?.unwrap_or(Value::Null)))},
            "jira_refresh"=>if target.is_empty() {Ok(json!(jira.as_ref().unwrap().0.refresh_all().await?))} else {jira.as_ref().unwrap().0.refresh_project(target).await},
            "jira_report"=>jira.as_ref().unwrap().1.prepare(target,&r["input"]).await,
            "jira_updates"=>Ok(json!(jira.as_ref().unwrap().1.read(target)?)),
            "jira_needs_summary"=>Ok(json!(jira.as_ref().unwrap().1.needs_research_summary(target,r["revision"].as_u64().unwrap())?)),
            "jira_flush"=>{jira.as_ref().unwrap().1.flush().await?;Ok(Value::Null)},
            "jira_delivery_close"=>{jira.as_ref().unwrap().1.close().await?;Ok(Value::Null)},
            "jira_sync_close"=>{jira.as_ref().unwrap().0.close().await?;Ok(Value::Null)},
            "jira_credentials"=>Ok(json!(credential_calls.load(std::sync::atomic::Ordering::SeqCst))),
            "prepare"=>work.prepare(target,&r["input"]).await,
            "research_submit"=>research.submit(target,key,&r["input"]).await,
            "research_read"=>research.read(target),
            "research_request"=>research.request_completion(target,key,&r["input"]),
            "research_complete"=>research.complete(target),
            "admit"=>store.with_store(|s|s.admit_assignment(target,&r["input"])),
            "desired"=>store.with_store(|s|s.set_desired_state(target,r["input"].as_str().unwrap())),
            "candidate"=>evidence.register_candidate(target,&serde_json::from_value::<Vec<String>>(r["input"].clone())?,r["refreshed"].as_str(),r.get("feedback")).await,
            "complete"=>evidence.complete(target,key).await,
            "assess"=>evidence.assess(target).await,
            "assign_review"=>evidence.assign_review(target,key,&r["input"]),
            "review"=>evidence.review(target,&r["input"]),
            "check_output"=>evidence.check_output(target,key),
            "prepare_check"=>checks.prepare(target,key,&r["input"]).await,
            "execute_check"=>checks.execute(target,key,&r["input"]).await,
            "finish_check"=>checks.finish(target,key).await,
            "reconcile_checks"=>{checks.reconcile_stopped(target).await?;Ok(Value::Null)},
            "check_definition"=>crewroom_runtime::evidence::check_definition(&r["input"]),
            "configure"=>evidence.configure(target,&r["input"]),
            "artifact"=>evidence.artifact(r["input"].as_str().unwrap()),
            "read_artifact"=>Ok(json!(evidence.read_artifact(&r["input"])?)),
            "correct_environment"=>evidence.correct_check_environment(target,&r["input"]),
            "request_completion"=>work.request_completion(target,key,&r["input"]).await,
            "commit"=>work.commit(target,&r["input"]).await,
            "export"=>Ok(json!(work.export_revision(target,&r["revision"],Path::new(name)).await?)),
            "verify_export"=>Ok(json!(Repository::<Arc<Mutex<Store>>>::verify_export(Path::new(name),&r["input"])?)),
            "status"=>work.status(target).await,
            "refresh"=>work.refresh_base(target,key,r["targetRef"].as_str().unwrap(),r["sourceCandidateId"].as_str().unwrap(),r.get("supersedes")).await,
            "validate_conflict"=>work.validate_conflict_workspace(target).await,
            "active_conflict"=>Ok(work.active_conflict(target)?.unwrap_or(Value::Null)),
            "reconcile_refresh"=>{work.reconcile_refresh(target).await?;Ok(Value::Null)},
            "cleanup_conflict"=>{work.cleanup_promoted_conflict(target,key).await?;Ok(Value::Null)},
            "run"=>store.with_store(|s|s.get_run(target)),
            "verify_cancelled"=>work.verify_cancelled_work(target,r["successor"].as_str()).await,
            "queue_later"=>work.queue_cancelled_continuation(target,&r["input"],r["roles"].as_array().unwrap()).await,
            "cancel"=>store.with_store(|s|{s.set_desired_state(target,"cancelled")?;s.begin_pause(target,None)}),
            "cancel_queued"=>store.with_store(|s|s.cancel_queued(target,&r["input"])),
            "checkpoint"=>{let current=work.status(target).await?;store.with_store(|s|s.checkpoint_repository(target,&json!({"id":uuid::Uuid::new_v4().to_string(),"at":crewroom_core::timestamp(crewroom_core::now_ms()),"reason":"interrupted","revision":current["revision"]})))},
            "save_publication"=>store.with_store(|s|s.save_publication(&r["input"])),
            "publication"=>store.with_store(|s|Ok(s.publication(target)?.unwrap_or(Value::Null))),
            "reconcile_writer"=>work.reconcile_stopped_writer(target).await,
            "notices"=>store.with_store(|s|Ok(json!(s.delivery_notices(target)?))),
            "associate_refresh"=>evidence.associate_refresh(target,key).await,
            "work_record"=>store.with_store(|s|Ok(s.repository_work(target)?.unwrap_or(Value::Null))),
            "dependency_hold"=>Ok(json!(work.dependency_base_hold(target,None).await?)),
            "list"=>work.list_files(target).await,
            "read"=>work.read_file(target,name).await,
            "read_revision"=>work.read_revision(target,&r["revision"],name).await,
            "diff"=>work.diff_revision(target,&r["revision"]).await,
            "search"=>work.search(target,&r["input"],r.get("revision")).await,
            "pin_research"=>work.pin_research(&r["input"]).await,
            "seed_research"=>{
                let (id, preflight)=store.with_store(|s|{let task=s.create_task(&r["input"])?;let id=task["id"].as_str().unwrap().to_owned();s.refine(&id,&json!({"revision":1,"sourceRevision":1,"brief":"Read the immutable source.","criteria":["Cite source"]}))?;let preflight=s.prepare_research(&id,&json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()}))?;Ok((id,preflight))})?;
                let pinned=tokio::task::spawn_blocking(move||preflight.validate()).await.map_err(std::io::Error::other)??;
                store.with_store(|s|{let run=s.queue_pinned_research(&pinned)?;Ok(json!({"taskId":id,"run":run}))})
            },
            "queue_research"=>{
                let preflight=store.with_store(|s|s.prepare_research(target,&r["input"]))?;
                let pinned=tokio::task::spawn_blocking(move||preflight.validate()).await.map_err(std::io::Error::other)??;
                store.with_store(|s|s.queue_pinned_research(&pinned))
            },
            "read_research"=>work.read_research(target,name).await,
            "search_research"=>work.search_research(target,&r["input"]).await,
            "claim"=>work.claim_writer(target,r.get("claim")).await,
            "claim_implementation"=>work.claim_implementation(target,key,&r["input"]).await,
            "continue_conflict"=>work.continue_target_conflict(target,key,&r["input"]).await,
            "restore_writer"=>Ok(work.restore_writer(target).await?.unwrap_or(Value::Null)),
            "shell"=>work.prepare_shell(target,&r["input"]).await,
            "release"=>work.release_writer(target).await,
            "edit"=>work.edit(target,key,&r["input"]).await,
            "patch"=>work.patch(target,key,&r["input"]).await,
            "reconcile_edits"=>{work.reconcile_stopped_edits(target).await?;Ok(Value::Null)},
            "runtime"=>store.with_store(|s|s.runtime(target)),
            "record"=>store.with_store(|s|Ok(s.coding_record(target)?.unwrap_or(Value::Null))),
            "replace"=>store.with_store(|s|s.replace_attempt(target,&r["input"])),
            "continue"=>store.with_store(|s|s.prepare_continuation(target,s.get_run(target)?["controlRevision"].as_u64().unwrap())),
            "task"=>store.with_store(|s|s.get_task(target)),
            // Explicit operator plan acceptance through the same store gate as the native workbench.
            "plan_source"=>{let preflight=store.with_store(|s|s.prepare_plan_source(target,r["input"]["revision"].as_u64().unwrap()))?;let pinned=tokio::task::spawn_blocking(move||preflight.validate()).await.map_err(|_|crewroom_core::Error::Conflict("Source validation interrupted.".into()))??;store.with_store(|s|s.save_plan_source(&pinned))},
            "plan_save"=>store.with_store(|s|s.save_implementation_plan(target,&r["input"])),
            "plan_decide"=>store.with_store(|s|s.decide_implementation_plan(target,&r["input"])),
            // Seeds later queued intents after the native store owns the schema the legacy workbench can no longer open.
            "create_task"=>store.with_store(|s|s.create_task(&r["input"])),
            "queue"=>store.with_store(|s|s.queue(target,&r["input"])),
            "configure_checks"=>store.with_store(|s|s.update_coding_record(target,|old|{let mut record=old.unwrap();record["requiredChecks"]=r["input"].clone();Ok(record)})),
            "policy_fixture"=>store.with_store(|s|s.update_coding_record(target,|old|{let mut record=old.unwrap();for(k,v)in r["input"].as_object().unwrap(){record[k]=v.clone();}Ok(record)})),
            "activate"=>store.with_store(|s|{
                for a in s.runtime(target)?["assignments"].as_array().unwrap() {let id=a["currentAttemptId"].as_str().unwrap();if s.get_attempt(id)?["ownership"]!="held" {s.claim_attempt(id)?;s.record_attempt_identity(id,&json!({"hostPid":1,"ompPid":2,"sessionId":id,"hostUrl":"http://127.0.0.1:4000","pane":"%0"}))?;}}
                s.set_desired_state(target,"running")?;s.start_execution(target)?;
                if s.get_run(target)?["purpose"]=="research" {return s.get_run(target);}
                s.update_coding_record(target,|old|Ok(old.unwrap_or(json!({"runId":target,"requiredChecks":[],"candidates":[],"checks":[],"reviews":[]}))))
            }),
            "pause"=>store.with_store(|s|s.begin_pause(target,None)),
            "ownership"=>store.with_store(|s|{s.record_ownership(target,r["stopped"].as_bool().unwrap_or(true),None)?;Ok(Value::Null)}),
            "finish_pause"=>store.with_store(|s|s.finish_pause(target)),
            "refine"=>store.with_store(|s|s.refine(target,&r["input"])),
            "adopt"=>store.with_store(|s|s.adopt_requirements(target,&r["input"])),
            _=>Err(crewroom_core::Error::Invalid("Unknown fixture operation".into()))
        }}.await;
        output(match result {Ok(v)=>json!({"id":r["id"],"value":v}),Err(e)=>json!({"id":r["id"],"error":e.to_string()})});});
        cancellations.insert(request_id, handle);
    }
    let repository_close = work.close()?;
    if let Some((_, _, publication)) = remote {
        publication.close().await?;
    }
    if let Some((sync, delivery)) = jira {
        let (a, b) = tokio::join!(sync.close(), delivery.close());
        a?;
        b?;
    }
    while let Some(job) = jobs.join_next().await {
        if let Err(error) = job
            && !error.is_cancelled()
        {
            return Err(std::io::Error::other(error).into());
        }
    }
    let stopped = repository_close
        .wait()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if stopped["stopped"] != true {
        return Err(std::io::Error::other(format!(
            "Repository shutdown remains unresolved: {stopped}"
        ))
        .into());
    }
    Ok(())
}
