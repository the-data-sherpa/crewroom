//! Live GitHub issue delivery probe (FOR-48). Drives the production
//! `GitHubDelivery` outbox against one issue in a disposable github.com
//! repository and prints a JSON receipt. The token is read from
//! `CREWROOM_GITHUB_TOKEN` for each request and never printed.
//!
//! Usage (`data` is a new private directory kept between stages):
//! `cargo run -p crewroom-runtime --example github-issue-delivery-live -- \
//!     <owner> <name> <issue> <data> <deliver|observe>`
//!
//! `deliver` imports the issue into a fresh store, queues reporter comments,
//! delivers them, replays a restart with an unknown and an interrupted write,
//! refuses an invalid token and retries, then records a synthetic merged
//! publication and reads the issue's state. Close the issue on GitHub, then
//! run `observe` to read and record the closed state.
//!
//! Only comments are written. Workflow labels are not configured (off), and
//! the merge is a synthetic publication record: the probe never merges.
use crewroom_core::store::Store;
use crewroom_runtime::{
    github_delivery::GitHubDelivery,
    providers::{
        Credential, Credentials, Error,
        github::{GitHubClient, issues::Reread},
        github_issue_client::GitHubIssueClient,
    },
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

type Shared = Arc<Mutex<Store>>;

fn token() -> Option<String> {
    std::env::var("CREWROOM_GITHUB_TOKEN")
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}
fn credentials(fixed: Option<&'static str>) -> Credentials {
    Arc::new(move || {
        let value = fixed.map(str::to_owned).or_else(token);
        Box::pin(async move {
            value
                .map(Credential::Bearer)
                .ok_or_else(|| Error::Invalid("CREWROOM_GITHUB_TOKEN is not set.".into()))
        })
    })
}
fn delivery(store: &Shared, fixed: Option<&'static str>) -> GitHubDelivery<Shared> {
    GitHubDelivery::new(store.clone(), credentials(fixed), None, None, None)
}
fn summary(update: &Value) -> Value {
    json!({"kind":update["kind"],"eventKey":update["eventKey"],"state":update["state"],"externalId":update["externalId"],"issueState":update["issueState"],"reason":update["reason"]})
}
async fn crewroom_comments(client: &GitHubIssueClient, issue: u64, run: &str) -> Vec<Value> {
    client
        .comments(issue)
        .await
        .unwrap()
        .into_iter()
        .filter(|c| {
            c["body"]
                .as_str()
                .is_some_and(|b| b.contains(&format!("Run {run}")))
        })
        .map(|c| json!({"id":c["id"],"firstLine":c["body"].as_str().unwrap_or("").lines().next()}))
        .collect()
}
fn git(repo: &Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

/// A coding run on the imported issue with an admitted delivery reporter.
fn prepare(
    data: &Path,
    owner: &str,
    name: &str,
    issue: &Value,
) -> (Shared, String, String, String) {
    let store = Store::open(&data.join("workflow.sqlite")).unwrap();
    let repo = data.join("repository");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Fixture",
        ],
    );
    let repository = store
        .create_repository(&json!({"name":"Probe","path":repo,"targetBranch":"main"}))
        .unwrap();
    // The project's confirmed GitHub Issues source drives its sync (FOR-45).
    let project = repository["id"].as_str().unwrap().to_owned();
    store
        .confirm_ticket_source(
            &project,
            &json!({"tickets":"github","repository":format!("{owner}/{name}")}),
            &crewroom_core::project_source::observe(Some(&format!(
                "https://github.com/{owner}/{name}.git"
            ))),
        )
        .unwrap();
    let saved = store
        .save_github_filters(&project, &json!({"state":"all"}))
        .unwrap();
    store
        .commit_github_sync(
            &project,
            &saved["source"],
            &json!([issue]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    let task = store.snapshot().unwrap()["tasks"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let revision = |s: &Store| s.get_task(&task).unwrap()["revision"].clone();
    store
        .assign_repository(
            &task,
            &json!({"revision":revision(&store),"repositoryId":repository["id"]}),
        )
        .unwrap();
    store
        .refine(&task, &json!({"revision":revision(&store),"sourceRevision":1,"brief":"Live FOR-48 delivery probe; synthetic content.","criteria":["Updates reach the issue once"]}))
        .unwrap();
    let current = store.get_task(&task).unwrap();
    let source = store
        .save_plan_source(
            &store
                .prepare_plan_source(&task, current["revision"].as_u64().unwrap())
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    let plan = store.save_implementation_plan(&task, &json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":current["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Probe delivery","scope":[current["brief"]],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"probe","name":"Probe","dependsOn":[],"outputs":["Receipt"],"criteria":[0],"validation":["Live probe"]}]}})).unwrap();
    store
        .decide_implementation_plan(plan["id"].as_str().unwrap(), &json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":current["revision"],"sourceId":source["id"],"decision":"accept"}))
        .unwrap();
    let run = store
        .queue(&task, &json!({"revision":revision(&store)}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../../crewroom-core/src/default-roles.json")).unwrap();
    let role = roles
        .iter()
        .find(|r| r["id"] == "delivery-reporter")
        .unwrap()
        .clone();
    let admitted = store
        .admit_assignment(
            &run,
            &json!({"name":"Reporter","coordinator":false,"role":role,"model":{},"input":"Report"}),
        )
        .unwrap();
    let attempt = admitted["attempt"]["id"].as_str().unwrap().to_owned();
    store.claim_attempt(&attempt).unwrap();
    store
        .record_attempt_identity(&attempt, &json!({"hostPid":1,"ompPid":2,"hostUrl":"http://127.0.0.1:59999","sessionId":"probe","pane":"%1"}))
        .unwrap();
    store.set_desired_state(&run, "running").unwrap();
    store.start_execution(&run).unwrap();
    (Arc::new(Mutex::new(store)), run, attempt, task)
}

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [owner, name, issue, data, stage] = args.as_slice() else {
        eprintln!("usage: <owner> <name> <issue> <data> <deliver|observe>");
        std::process::exit(2);
    };
    let number: u64 = issue.parse().unwrap();
    let data = Path::new(data);
    let client =
        GitHubIssueClient::new(&format!("{owner}/{name}"), credentials(None), None).unwrap();
    let mut receipt =
        json!({"stage":stage,"observedAt":crewroom_core::timestamp(crewroom_core::now_ms())});
    match stage.as_str() {
        "deliver" => {
            std::fs::create_dir(data).expect("data must be a new directory");
            let raw = match GitHubClient::new(None)
                .unwrap()
                .issue(
                    &token().expect("CREWROOM_GITHUB_TOKEN"),
                    owner,
                    name,
                    number,
                )
                .await
            {
                Ok(Reread::Issue(issue)) => issue,
                _ => panic!("the issue could not be read"),
            };
            let (store, run, reporter, task) = prepare(data, owner, name, &raw);
            receipt["run"] = json!(run);
            let live = delivery(&store, None);
            let before = crewroom_comments(&client, number, &run).await.len();
            for input in [
                json!({"kind":"milestone","eventKey":"impact-reviewed","body":"Live FOR-48 probe: impact reviewed (synthetic)."}),
                json!({"kind":"blocker","eventKey":"awaiting-fixture","body":"Live FOR-48 probe: synthetic blocker for delivery evidence."}),
            ] {
                live.prepare(&reporter, &input).await.unwrap();
            }
            let labels = live
                .prepare(
                    &reporter,
                    &json!({"kind":"in_progress","eventKey":"execution-started"}),
                )
                .await;
            receipt["labelsOff"] = json!(labels.err().map(|e| e.to_string()));
            live.flush().await.unwrap();
            receipt["firstPass"] = json!(
                live.read(&run)
                    .unwrap()
                    .iter()
                    .map(summary)
                    .collect::<Vec<_>>()
            );
            receipt["commentsAfterFirstPass"] =
                json!(crewroom_comments(&client, number, &run).await.len() - before);

            // Restart: a new store connection and outbox. One confirmed write is
            // replayed as an unknown outcome and one as interrupted mid-send.
            drop(live);
            drop(store);
            let store: Shared = Arc::new(Mutex::new(
                Store::open(&data.join("workflow.sqlite")).unwrap(),
            ));
            {
                let s = store.lock().unwrap();
                let mut updates = s.github_updates(Some(&run)).unwrap();
                for (update, state) in updates.iter_mut().zip(["uncertain", "sending"]) {
                    update["state"] = json!(state);
                    if let Some(f) = update.as_object_mut() {
                        f.remove("externalId");
                    }
                    s.save_github_update(update).unwrap();
                }
            }
            let restarted = delivery(&store, None);
            restarted.flush().await.unwrap();
            restarted.flush().await.unwrap();
            receipt["afterRestart"] = json!(
                restarted
                    .read(&run)
                    .unwrap()
                    .iter()
                    .map(summary)
                    .collect::<Vec<_>>()
            );
            receipt["commentsAfterRestart"] =
                json!(crewroom_comments(&client, number, &run).await.len() - before);

            // A rejected token is undelivered with its reason, then retried.
            restarted
                .prepare(&reporter, &json!({"kind":"milestone","eventKey":"after-token-fix","body":"Live FOR-48 probe: delivered after an operator retry (synthetic)."}))
                .await
                .unwrap();
            let rejected = delivery(&store, Some("github_pat_synthetic0invalid0token"));
            rejected.flush().await.unwrap();
            let blocked = rejected
                .read(&run)
                .unwrap()
                .into_iter()
                .find(|u| u["eventKey"] == "after-token-fix")
                .unwrap();
            receipt["invalidToken"] = summary(&blocked);
            restarted.retry(blocked["id"].as_str().unwrap()).unwrap();
            restarted.flush().await.unwrap();
            receipt["afterRetry"] = summary(
                &restarted
                    .read(&run)
                    .unwrap()
                    .into_iter()
                    .find(|u| u["eventKey"] == "after-token-fix")
                    .unwrap(),
            );
            receipt["commentsAfterRetry"] = json!(crewroom_comments(&client, number, &run).await);

            // Synthetic merged publication: Crewroom reads, never closes, the issue.
            {
                let s = store.lock().unwrap();
                s.save_publication(&json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run,"attemptId":null,"candidateId":uuid::Uuid::new_v4().to_string(),"repository":{"provider":"github","owner":owner,"name":name,"id":1,"uuid":"github:1"},"revision":{"head":"synthetic"},"push":{"state":"confirmed"},"pr":{"state":"confirmed","id":1,"providerState":"MERGED"},"merged":{"commit":"synthetic","observedAt":crewroom_core::timestamp(crewroom_core::now_ms())},"ready":false}))
                    .unwrap();
            }
            restarted.reconcile().await.unwrap();
            receipt["mergeObservation"] = summary(
                &restarted
                    .read(&run)
                    .unwrap()
                    .into_iter()
                    .find(|u| u["kind"] == "done")
                    .unwrap(),
            );
            let s = store.lock().unwrap();
            receipt["ticketSourceStatus"] = s.get_task(&task).unwrap()["sourceStatus"].clone();
            receipt["runStatus"] = s.get_run(&run).unwrap()["status"].clone();
        }
        "observe" => {
            let store: Shared = Arc::new(Mutex::new(
                Store::open(&data.join("workflow.sqlite")).unwrap(),
            ));
            let (run, task) = {
                let s = store.lock().unwrap();
                let run = s.snapshot().unwrap()["runs"][0].clone();
                (
                    run["id"].as_str().unwrap().to_owned(),
                    run["taskId"].as_str().unwrap().to_owned(),
                )
            };
            let live = delivery(&store, None);
            let before = with_status(&store, &run, &task);
            live.reconcile().await.unwrap();
            receipt["before"] = before;
            receipt["mergeObservation"] = summary(
                &live
                    .read(&run)
                    .unwrap()
                    .into_iter()
                    .find(|u| u["kind"] == "done")
                    .unwrap(),
            );
            receipt["after"] = with_status(&store, &run, &task);
            receipt["crewroomComments"] =
                json!(crewroom_comments(&client, number, &run).await.len());
        }
        _ => panic!("unknown stage"),
    }
    let text = serde_json::to_string_pretty(&receipt).unwrap();
    if let Some(t) = token() {
        assert!(!text.contains(&t), "the receipt must not carry the token");
    }
    println!("{text}");
}

fn with_status(store: &Shared, run: &str, task: &str) -> Value {
    let s = store.lock().unwrap();
    let run = s.get_run(run).unwrap();
    let task = s.get_task(task).unwrap();
    json!({"ticketSourceStatus":task["sourceStatus"],"sourceUpdatedAt":task["sourceUpdatedAt"],"runStatus":run["status"],"runDesiredState":run["desiredState"]})
}
