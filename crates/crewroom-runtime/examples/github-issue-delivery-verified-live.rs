//! Live GitHub issue delivery probe for verified updates, workflow labels and
//! the merge-to-close observation (FOR-48). Drives the production
//! `GitHubDelivery` outbox, `GitHubIssueClient` and `GitHubPullClient` against
//! a disposable github.com repository and prints a JSON receipt per stage. The
//! token is read from `CREWROOM_GITHUB_TOKEN` for each request and never
//! printed.
//!
//! Usage (`data` is a new private directory kept between stages; `helper` is
//! the built `crewroom-process` binary):
//! `cargo run -p crewroom-runtime --example github-issue-delivery-verified-live -- \
//!     <owner> <name> <repository-id> <issue> <data> <helper> <stage>`
//!
//! Coding stages, in order:
//! - `candidate` clones the repository, imports the issue, records a ready
//!   candidate (a real commit on the default branch; check and review receipts
//!   recorded directly), confirms workflow labels are refused while off, posts
//!   the evidence comment and replays a restart with an unknown write.
//! - `publish` pushes the candidate to a branch, opens a PR whose body says
//!   `Closes #<issue>` through the FOR-46 client, records the observed PR,
//!   shows no labels are written while off, then configures labels, posts the
//!   PR-link comment, applies in-progress/in-review labels and replays a
//!   restart with an interrupted write.
//! - Merge the PR on GitHub (outside the probe); GitHub closes the issue.
//! - `merged` observes the merged PR, records it and reads the issue state
//!   GitHub reports into the ticket's source status.
//!
//! The research stage uses its own `data` directory and issue:
//! - `research` queues pinned research on the cloned repository, submits a
//!   cited artifact, requests completion, holds until the outcome is queued,
//!   completes, posts the outcome and replays a restart with an unknown write.
use crewroom_core::{authority::Authority, store::Store};
use crewroom_runtime::{
    evidence::{Evidence, Research},
    github_delivery::GitHubDelivery,
    providers::{
        Credential, Credentials, Error,
        github::{GitHubClient, issues::Reread},
        github_issue_client::GitHubIssueClient,
        github_pulls::GitHubPullClient,
    },
    repository::Repository,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type Shared = Arc<Mutex<Store>>;
const BRANCH_PREFIX: &str = "crewroom/for-48-live-";

fn token() -> Option<String> {
    std::env::var("CREWROOM_GITHUB_TOKEN")
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}
fn credentials() -> Credentials {
    Arc::new(move || {
        let value = token();
        Box::pin(async move {
            value
                .map(Credential::Bearer)
                .ok_or_else(|| Error::Invalid("CREWROOM_GITHUB_TOKEN is not set.".into()))
        })
    })
}
fn summary(update: &Value) -> Value {
    json!({"kind":update["kind"],"eventKey":update["eventKey"],"state":update["state"],"externalId":update["externalId"],"issueState":update["issueState"],"closesOnMerge":update["closesOnMerge"],"reason":update["reason"]})
}
fn summaries(delivery: &GitHubDelivery<Shared>, run: &str) -> Value {
    json!(
        delivery
            .read(run)
            .unwrap()
            .iter()
            .map(summary)
            .collect::<Vec<_>>()
    )
}
fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Crewroom probe",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
/// Crewroom comments for this run on the issue: id and first line only.
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
async fn issue_labels(client: &GitHubIssueClient, issue: u64) -> Value {
    json!(client.issue(issue).await.unwrap().labels)
}

/// Store, repository, evidence and delivery services over one data directory.
struct World {
    data: PathBuf,
    store: Shared,
    repository: Repository<Shared>,
    evidence: Evidence<Shared>,
    research: Research<Shared>,
    delivery: GitHubDelivery<Shared>,
}
impl World {
    fn open(data: &Path, helper: &Path) -> Self {
        let data = data.canonicalize().unwrap();
        let store: Shared = Arc::new(Mutex::new(
            Store::open(&data.join("workflow.sqlite")).unwrap(),
        ));
        let authority = Arc::new(Authority::acquire_for_execution(&data.join("data")).unwrap());
        let repository = Repository::new(
            store.clone(),
            data.join("data/worktrees"),
            helper.canonicalize().unwrap(),
            authority,
        )
        .unwrap();
        let evidence = Evidence::new(repository.clone(), data.join("data/artifacts")).unwrap();
        let research = Research::new(evidence.clone());
        let delivery = GitHubDelivery::new(
            store.clone(),
            credentials(),
            None,
            Some(evidence.clone()),
            Some(research.clone()),
        );
        Self {
            data,
            store,
            repository,
            evidence,
            research,
            delivery,
        }
    }
    fn with<T>(&self, f: impl FnOnce(&Store) -> T) -> T {
        f(&self.store.lock().unwrap())
    }
    /// A restart: every service is closed and rebuilt over a new connection.
    async fn restart(self, helper: &Path) -> Self {
        let Self {
            data,
            store,
            repository,
            evidence,
            research,
            delivery,
        } = self;
        drop((delivery, research, evidence, store));
        repository.close().unwrap().wait().await.unwrap();
        // The controller authority is released only with its last holder.
        drop(repository);
        Self::open(&data, helper)
    }
    async fn close(self) {
        let Self {
            repository,
            evidence,
            research,
            delivery,
            ..
        } = self;
        drop((delivery, research, evidence));
        repository.close().unwrap().wait().await.unwrap();
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.data.join("probe.json")).unwrap()).unwrap()
    }
    fn save_state(&self, state: &Value) {
        std::fs::write(
            self.data.join("probe.json"),
            serde_json::to_vec_pretty(state).unwrap(),
        )
        .unwrap();
    }
}

/// Imports the issue, clones the repository and assigns it to the ticket with
/// an accepted brief. Returns the task id.
async fn import(data: &Path, owner: &str, name: &str, number: u64) -> String {
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
    let store = Store::open(&data.join("workflow.sqlite")).unwrap();
    let saved = store
        .save_github_source(&json!({"repository":format!("{owner}/{name}"),"state":"all"}))
        .unwrap();
    store
        .commit_github_sync(&saved["source"], &json!([raw]), &json!([]), &json!([]))
        .unwrap();
    let task = store.snapshot().unwrap()["tasks"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let checkout = data.join("repository");
    let status = std::process::Command::new("git")
        .args(["clone", "-q", &format!("git@github.com:{owner}/{name}.git")])
        .arg(&checkout)
        .status()
        .unwrap();
    assert!(status.success(), "clone failed");
    let repository = store
        .create_repository(&json!({"name":"Live sandbox","path":checkout.canonicalize().unwrap(),"targetBranch":"main"}))
        .unwrap();
    let revision = |s: &Store| s.get_task(&task).unwrap()["revision"].clone();
    store
        .assign_repository(
            &task,
            &json!({"revision":revision(&store),"repositoryId":repository["id"]}),
        )
        .unwrap();
    store
        .refine(&task, &json!({"revision":revision(&store),"sourceRevision":1,"brief":"Live FOR-48 verified delivery probe; synthetic content.","criteria":["Verified updates reach the issue once"]}))
        .unwrap();
    task
}
fn admit(store: &Store, run: &str, team: &[(&str, &str)]) -> Vec<String> {
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../../crewroom-core/src/default-roles.json")).unwrap();
    team.iter()
        .map(|(name, id)| {
            let role = roles.iter().find(|r| r["id"] == *id).unwrap().clone();
            store
                .admit_assignment(
                    run,
                    &json!({"name":name,"coordinator":*id == "coordinator","role":role,"model":{},"input":"Report"}),
                )
                .unwrap()["attempt"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}
/// Claims every admitted attempt with a recorded (never probed) identity and
/// starts execution. No agent process runs in this probe.
fn start(store: &Store, run: &str) {
    let attempts = store.runtime(run).unwrap()["attempts"].clone();
    for (index, attempt) in attempts.as_array().unwrap().iter().enumerate() {
        let attempt = attempt["id"].as_str().unwrap();
        let pid = 101 + 100 * index as u64;
        store.claim_attempt(attempt).unwrap();
        store
            .record_attempt_identity(attempt, &json!({"hostPid":pid,"ompPid":pid+1,"hostUrl":"http://127.0.0.1:59999","sessionId":format!("probe-{pid}"),"pane":"%1"}))
            .unwrap();
    }
    store.set_desired_state(run, "running").unwrap();
    store.start_execution(run).unwrap();
}

async fn candidate(
    owner: &str,
    name: &str,
    number: u64,
    data: &Path,
    helper: &Path,
    client: &GitHubIssueClient,
) -> Value {
    let task = import(data, owner, name, number).await;
    let (run, attempts) = {
        let store = Store::open(&data.join("workflow.sqlite")).unwrap();
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
        let plan = store.save_implementation_plan(&task, &json!({"requestId":uuid(),"revision":current["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Probe verified delivery","scope":[current["brief"]],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"probe","name":"Probe","dependsOn":[],"outputs":["Receipt"],"criteria":[0],"validation":["Live probe"]}]}})).unwrap();
        store
            .decide_implementation_plan(plan["id"].as_str().unwrap(), &json!({"requestId":uuid(),"revision":current["revision"],"sourceId":source["id"],"decision":"accept"}))
            .unwrap();
        let run = store
            .queue(&task, &json!({"revision":current["revision"]}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let attempts = admit(
            &store,
            &run,
            &[
                ("Reporter", "delivery-reporter"),
                ("Writer", "implementer"),
                ("Reviewer", "reviewer"),
            ],
        );
        (run, attempts)
    };
    let (reporter, writer) = (attempts[0].clone(), attempts[1].clone());
    let world = World::open(data, helper);
    let check = json!({"id":"unit","image":format!("sha256:{}", "0".repeat(64)),"command":"/bin/true","args":[],"timeoutMs":1000,"required":true});
    world.evidence.configure(&run, &json!([check])).unwrap();
    let work = world
        .repository
        .prepare(&run, &json!({"mode":"new"}))
        .await
        .unwrap();
    world.with(|s| start(s, &run));
    let path = PathBuf::from(work["path"].as_str().unwrap());
    std::fs::write(
        path.join(format!("for-48-live-{number}.md")),
        format!("Synthetic FOR-48 live probe change for #{number}.\n"),
    )
    .unwrap();
    git(&path, &["add", "."]);
    git(
        &path,
        &[
            "commit",
            "-q",
            "-m",
            &format!("FOR-48 live probe change for #{number} (synthetic)"),
        ],
    );
    world.with(|s| {
        s.record_ownership(
            &writer,
            true,
            Some("Recorded probe; no agent process executed"),
        )
        .unwrap()
    });
    let candidate = world
        .evidence
        .register_candidate(&run, std::slice::from_ref(&writer), None, None)
        .await
        .unwrap();
    let output = world.evidence.artifact("ok\n").unwrap();
    let source = world.evidence.artifact("synthetic source").unwrap();
    let review = world
        .evidence
        .artifact("Synthetic live-probe review")
        .unwrap();
    world.with(|s| {
        let reviewer = s.runtime(&run).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "Reviewer")
            .unwrap()["id"]
            .clone();
        s.update_coding_record(&run, |record| {
            let mut record = record.unwrap();
            let definition = record["requiredChecks"][0].clone();
            record["checks"].as_array_mut().unwrap().push(json!({"id":uuid(),"candidateId":candidate["id"],"checkId":"unit","definition":definition,"started":true,"completed":true,"sourceIntact":true,"exitCode":0,"timedOut":false,"output":output,"source":source}));
            record["reviews"].as_array_mut().unwrap().push(json!({"id":uuid(),"candidateId":candidate["id"],"assignmentId":reviewer,"verdict":"pass","checkPolicyRevision":0,"artifact":review}));
            Ok(record)
        })
        .unwrap()
    });
    let assessment = world.evidence.assess(&run).await.unwrap();
    assert_eq!(assessment["ready"], true, "{assessment}");
    let mut receipt = json!({"run":run,"candidate":{"id":candidate["id"],"head":candidate["revision"]["head"]},"assessmentReady":assessment["ready"]});
    receipt["labelsOff"] = json!(
        world
            .delivery
            .prepare(
                &reporter,
                &json!({"kind":"in_progress","eventKey":"execution-started"})
            )
            .await
            .err()
            .map(|e| e.to_string())
    );
    let before = crewroom_comments(client, number, &run).await.len();
    let evidence = world
        .delivery
        .prepare(
            &reporter,
            &json!({"kind":"evidence","eventKey":"evidence-ready","candidateId":candidate["id"]}),
        )
        .await
        .unwrap();
    receipt["evidenceQueued"] = json!({"state":evidence["state"],"firstLine":evidence["body"].as_str().unwrap_or("").lines().next()});
    world.delivery.flush().await.unwrap();
    receipt["firstPass"] = summaries(&world.delivery, &run);
    receipt["commentsAfterFirstPass"] =
        json!(crewroom_comments(client, number, &run).await.len() - before);

    // Restart with the confirmed evidence write replayed as an unknown outcome.
    world.with(|s| {
        let mut update = s.github_updates(Some(&run)).unwrap().remove(0);
        update["state"] = json!("uncertain");
        update.as_object_mut().unwrap().remove("externalId");
        s.save_github_update(&update).unwrap();
    });
    let world = world.restart(helper).await;
    world.delivery.flush().await.unwrap();
    world.delivery.flush().await.unwrap();
    receipt["afterRestart"] = summaries(&world.delivery, &run);
    receipt["commentsAfterRestart"] = json!(crewroom_comments(client, number, &run).await);
    receipt["issueLabels"] = issue_labels(client, number).await;
    world.save_state(
        &json!({"run":run,"task":task,"reporter":reporter,"candidate":candidate,"before":before}),
    );
    world.close().await;
    receipt
}

async fn publish(
    owner: &str,
    name: &str,
    repository_id: i64,
    number: u64,
    data: &Path,
    helper: &Path,
    client: &GitHubIssueClient,
) -> Value {
    let world = World::open(data, helper);
    let state = world.state();
    let (run, reporter, candidate) = (
        state["run"].as_str().unwrap().to_owned(),
        state["reporter"].as_str().unwrap().to_owned(),
        state["candidate"].clone(),
    );
    let head = candidate["revision"]["head"].as_str().unwrap().to_owned();
    let branch = format!("{BRANCH_PREFIX}{number}");
    let work = world.with(|s| s.repository_work(&run).unwrap().unwrap());
    let worktree = PathBuf::from(work["path"].as_str().unwrap());
    git(
        &worktree,
        &[
            "push",
            "-q",
            &format!("git@github.com:{owner}/{name}.git"),
            &format!("{head}:refs/heads/{branch}"),
        ],
    );
    let repository = json!({"owner":owner,"name":name,"id":repository_id});
    let pulls = GitHubPullClient::new(&repository, credentials(), None).unwrap();
    let verified = pulls.verify_repository().await.unwrap();
    let marker = format!("Crewroom publication {}", candidate["id"].as_str().unwrap());
    let created = pulls
        .create_pull_request(
            &json!({"title":format!("#{number}: FOR-48 live merge probe (synthetic)"),"description":format!("Synthetic FOR-48 live probe change.\n\nCloses #{number}\n\n{marker}"),"branch":branch,"target":"main","head":head}),
            None,
        )
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    let observed = pulls.pull_request(id).await.unwrap();
    let mut receipt = json!({"pr":{"id":id,"state":observed["state"],"head":observed["source"]["commit"]["hash"],"url":observed["links"]["html"]["href"],"target":observed["destination"]["branch"]["name"]},"expectedHead":head});
    let publication = json!({"id":uuid(),"runId":run,"attemptId":null,"candidateId":candidate["id"],"repository":{"provider":"github","owner":owner,"name":name,"id":repository_id,"uuid":verified["uuid"]},"revision":candidate["revision"],"remoteSourceHead":observed["source"]["commit"]["hash"],"push":{"state":"confirmed"},"pr":{"state":"confirmed","id":id,"providerState":observed["state"],"url":observed["links"]["html"]["href"]},"observation":{"pr":observed},"ready":false});
    world.with(|s| s.save_publication(&publication).unwrap());
    world.save_state(&{
        let mut next = state.clone();
        next["publication"] = publication.clone();
        next
    });

    // Labels off: an open PR derives no label change and nothing is written.
    world.delivery.flush().await.unwrap();
    receipt["labelsOff"] = json!({"labelUpdates":world.delivery.read(&run).unwrap().iter().filter(|u| matches!(u["kind"].as_str(),Some("in_progress"|"in_review"))).count(),"issueLabels":issue_labels(client, number).await});

    // PR link comment, then configured labels.
    let pr_link = world
        .delivery
        .prepare(
            &reporter,
            &json!({"kind":"pr_link","eventKey":"pr-opened","candidateId":candidate["id"]}),
        )
        .await
        .unwrap();
    receipt["prLinkQueued"] = json!({"state":pr_link["state"],"lines":pr_link["body"].as_str().unwrap_or("").lines().filter(|l| l.starts_with("Pull request:") || l.contains("closes this issue")).collect::<Vec<_>>()});
    world.with(|s| {
        s.save_github_status_labels(
            &json!({"inProgress":"crewroom: in progress","inReview":"crewroom: in review"}),
        )
        .unwrap()
    });
    world
        .delivery
        .prepare(
            &reporter,
            &json!({"kind":"in_progress","eventKey":"execution-started"}),
        )
        .await
        .unwrap();
    world.delivery.flush().await.unwrap();
    world.delivery.flush().await.unwrap();
    receipt["labelsOn"] = json!({"updates":summaries(&world.delivery, &run),"issueLabels":issue_labels(client, number).await});

    // Restart with the confirmed PR-link write replayed as interrupted mid-send.
    world.with(|s| {
        let mut update = s
            .github_updates(Some(&run))
            .unwrap()
            .into_iter()
            .find(|u| u["kind"] == "pr_link")
            .unwrap();
        update["state"] = json!("sending");
        update.as_object_mut().unwrap().remove("externalId");
        s.save_github_update(&update).unwrap();
    });
    let world = world.restart(helper).await;
    world.delivery.flush().await.unwrap();
    world.delivery.flush().await.unwrap();
    receipt["afterRestart"] = summaries(&world.delivery, &run);
    receipt["crewroomComments"] = json!(crewroom_comments(client, number, &run).await);
    world.close().await;
    receipt
}

async fn merged(
    owner: &str,
    name: &str,
    repository_id: i64,
    number: u64,
    data: &Path,
    helper: &Path,
    client: &GitHubIssueClient,
) -> Value {
    let world = World::open(data, helper);
    let state = world.state();
    let run = state["run"].as_str().unwrap().to_owned();
    let task = state["task"].as_str().unwrap().to_owned();
    let mut publication = state["publication"].clone();
    let pulls = GitHubPullClient::new(
        &json!({"owner":owner,"name":name,"id":repository_id}),
        credentials(),
        None,
    )
    .unwrap();
    let observed = pulls
        .pull_request(publication["pr"]["id"].as_i64().unwrap())
        .await
        .unwrap();
    let mut receipt = json!({"pr":{"id":observed["id"],"state":observed["state"],"mergeCommit":observed["merge_commit"]}});
    assert_eq!(observed["state"], "MERGED", "merge the PR first");
    let status = |w: &World| {
        w.with(|s| {
            let run = s.get_run(&run).unwrap();
            let task = s.get_task(&task).unwrap();
            json!({"ticketSourceStatus":task["sourceStatus"],"sourceUpdatedAt":task["sourceUpdatedAt"],"runStatus":run["status"],"runDesiredState":run["desiredState"],"runPurpose":run["purpose"]})
        })
    };
    receipt["before"] = status(&world);
    publication["pr"]["providerState"] = json!("MERGED");
    publication["observation"]["pr"] = observed.clone();
    publication["merged"] = json!({"commit":observed["merge_commit"]["hash"],"observedAt":crewroom_core::timestamp(crewroom_core::now_ms())});
    world.with(|s| s.save_publication(&publication).unwrap());
    world.delivery.reconcile().await.unwrap();
    world.delivery.flush().await.unwrap();
    receipt["updates"] = summaries(&world.delivery, &run);
    receipt["after"] = status(&world);
    receipt["issueLabels"] = issue_labels(client, number).await;
    receipt["issueState"] = json!(client.issue(number).await.unwrap().state);
    receipt["crewroomComments"] = json!(crewroom_comments(client, number, &run).await.len());
    world.close().await;
    receipt
}

async fn research(
    owner: &str,
    name: &str,
    number: u64,
    data: &Path,
    helper: &Path,
    client: &GitHubIssueClient,
) -> Value {
    let task = import(data, owner, name, number).await;
    let (run, coordinator, reporter) = {
        let store = Store::open(&data.join("workflow.sqlite")).unwrap();
        let revision = store.get_task(&task).unwrap()["revision"].clone();
        let run = store
            .queue_research(&task, &json!({"revision":revision,"requestId":uuid()}))
            .unwrap();
        let id = run["id"].as_str().unwrap().to_owned();
        let attempts = admit(
            &store,
            &id,
            &[
                ("Coordinator", "coordinator"),
                ("Reporter", "delivery-reporter"),
            ],
        );
        start(&store, &id);
        (id, attempts[0].clone(), attempts[1].clone())
    };
    let world = World::open(data, helper);
    let run_record = world.with(|s| s.get_run(&run).unwrap());
    let revision = run_record["taskRevision"].as_u64().unwrap();
    let commit = run_record["researchSource"]["commit"].clone();
    let checkout = world.data.join("repository");
    let first = std::fs::read_to_string(checkout.join("README.md"))
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    let mut receipt =
        json!({"run":run,"taskRevision":revision,"researchSourceCommit":commit.clone()});
    let artifact = world
        .research
        .submit(
            &coordinator,
            "live-research",
            &json!({"title":"Sandbox README heading","kind":"research","body":"# Findings\nThe pinned sandbox README names the repository in its first line.","citations":[{"path":"README.md","commit":commit,"startLine":1,"endLine":1,"excerpt":first}],"limitations":"Synthetic live probe; only the sandbox README was read.","coverage":[{"criterionId":0,"section":"Findings"}]}),
        )
        .await
        .unwrap();
    receipt["artifact"] = json!({"state":artifact["state"],"title":artifact["title"]});
    let input =
        json!({"kind":"research_outcome","eventKey":format!("research-outcome-{revision}")});
    receipt["beforeCompletionRequest"] = json!(
        world
            .delivery
            .prepare(&reporter, &input)
            .await
            .err()
            .map(|e| e.to_string())
    );
    world
        .research
        .request_completion(&coordinator, "complete-1", &json!({}))
        .unwrap();
    receipt["gateBeforeQueue"] = json!(
        world
            .delivery
            .needs_research_summary(&run, revision)
            .unwrap()
    );
    let queued = world.delivery.prepare(&reporter, &input).await.unwrap();
    receipt["gateAfterQueue"] = json!(
        world
            .delivery
            .needs_research_summary(&run, revision)
            .unwrap()
    );
    receipt["queued"] = json!({"state":queued["state"]});
    world.delivery.flush().await.unwrap();
    receipt["whileRunning"] = summaries(&world.delivery, &run);
    let before = crewroom_comments(client, number, &run).await.len();
    receipt["commentsWhileRunning"] = json!(before);
    world.with(|s| {
        for attempt in [&coordinator, &reporter] {
            s.record_ownership(
                attempt,
                true,
                Some("Recorded probe; no agent process executed"),
            )
            .unwrap();
        }
        s.begin_pause(&run, None).unwrap();
        s.finish_pause(&run).unwrap();
    });
    receipt["completed"] = world.research.complete(&run).unwrap()["status"].clone();
    world.delivery.flush().await.unwrap();
    receipt["firstPass"] = summaries(&world.delivery, &run);
    receipt["commentsAfterFirstPass"] = json!(crewroom_comments(client, number, &run).await.len());

    // Restart with the confirmed outcome write replayed as an unknown outcome.
    world.with(|s| {
        let mut update = s.github_updates(Some(&run)).unwrap().remove(0);
        update["state"] = json!("uncertain");
        update.as_object_mut().unwrap().remove("externalId");
        s.save_github_update(&update).unwrap();
    });
    let world = world.restart(helper).await;
    world.delivery.flush().await.unwrap();
    world.delivery.flush().await.unwrap();
    receipt["afterRestart"] = summaries(&world.delivery, &run);
    receipt["crewroomComments"] = json!(crewroom_comments(client, number, &run).await);
    receipt["issueLabels"] = issue_labels(client, number).await;
    receipt["ticketSourceStatus"] =
        world.with(|s| s.get_task(&task).unwrap()["sourceStatus"].clone());
    world.close().await;
    receipt
}

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [owner, name, repository_id, issue, data, helper, stage] = args.as_slice() else {
        eprintln!(
            "usage: <owner> <name> <repository-id> <issue> <data> <helper> <candidate|publish|merged|research>"
        );
        std::process::exit(2);
    };
    let number: u64 = issue.parse().unwrap();
    let repository_id: i64 = repository_id.parse().unwrap();
    let (data, helper) = (Path::new(data), Path::new(helper));
    let client = GitHubIssueClient::new(&format!("{owner}/{name}"), credentials(), None).unwrap();
    let mut receipt = json!({"stage":stage,"issue":number,"observedAt":crewroom_core::timestamp(crewroom_core::now_ms())});
    receipt["result"] = match stage.as_str() {
        "candidate" => candidate(owner, name, number, data, helper, &client).await,
        "publish" => publish(owner, name, repository_id, number, data, helper, &client).await,
        "merged" => merged(owner, name, repository_id, number, data, helper, &client).await,
        "research" => research(owner, name, number, data, helper, &client).await,
        other => panic!("unknown stage {other}"),
    };
    let text = serde_json::to_string_pretty(&receipt).unwrap();
    if let Some(t) = token() {
        assert!(!text.contains(&t), "the receipt must not carry the token");
    }
    println!("{text}");
}
