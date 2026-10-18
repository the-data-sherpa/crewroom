//! GitHub issue delivery (FOR-48) against a simulated GitHub REST API (loopback
//! fixture, not github.com). Proves exactly-once comments across outages, lost
//! replies and restarts; truthful undelivered states with reasons and an
//! operator retry; workflow labels only when configured; and that the issue's
//! open/closed state after a merge is recorded as its source status, separate
//! from Crewroom workflow state. Live github.com evidence is recorded
//! separately in docs/testing.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use crewroom_core::store::Store;
use crewroom_runtime::{
    github_delivery::GitHubDelivery,
    providers::{Credential, Credentials, Error, Unavailable},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "github_pat_synthetic0delivery";
const OWNER: &str = "fixture";
const NAME: &str = "issues";
type Shared = Arc<Mutex<Store>>;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Online,
    /// Every request answers 503.
    Offline,
    /// A comment is stored, but the reply is lost (HTTP 502).
    LostReply,
    /// The comment write never lands and the reply is lost (HTTP 502).
    LostWrite,
    /// Writes are refused with HTTP 403.
    Forbidden,
}
struct World {
    mode: Mode,
    state: String,
    labels: Vec<String>,
    comments: Vec<Value>,
    posts: usize,
    label_writes: usize,
    next: i64,
}
type Fixture = Arc<Mutex<World>>;

fn guard(f: &Fixture, headers: &HeaderMap, write: bool) -> Option<Response> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    assert_eq!(header("user-agent"), Some("crewroom"));
    assert_eq!(header("x-github-api-version"), Some("2022-11-28"));
    if header("authorization") != Some(&format!("Bearer {TOKEN}")) {
        return Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message":"Bad credentials"})),
            )
                .into_response(),
        );
    }
    let mode = f.lock().unwrap().mode;
    match mode {
        Mode::Offline => Some(StatusCode::SERVICE_UNAVAILABLE.into_response()),
        Mode::Forbidden if write => Some(
            (
                StatusCode::FORBIDDEN,
                Json(json!({"message":"Resource not accessible by personal access token"})),
            )
                .into_response(),
        ),
        _ => None,
    }
}
fn issue_json(world: &World) -> Value {
    json!({"id":7001,"number":1,"title":"Synthetic","state":world.state,"html_url":format!("https://github.com/{OWNER}/{NAME}/issues/1"),"updated_at":"2026-10-18T10:00:00Z","labels":world.labels.iter().map(|l| json!({"name":l})).collect::<Vec<_>>()})
}
async fn issue(State(f): State<Fixture>, headers: HeaderMap, Path(n): Path<u64>) -> Response {
    if let Some(r) = guard(&f, &headers, false) {
        return r;
    }
    assert_eq!(n, 1);
    Json(issue_json(&f.lock().unwrap())).into_response()
}
async fn comments(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = guard(&f, &headers, false) {
        return r;
    }
    let per: usize = q["per_page"].parse().unwrap();
    let page: usize = q["page"].parse().unwrap();
    let world = f.lock().unwrap();
    let rows: Vec<_> = world
        .comments
        .iter()
        .skip((page - 1) * per)
        .take(per)
        .cloned()
        .collect();
    Json(rows).into_response()
}
async fn comment(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Some(r) = guard(&f, &headers, true) {
        return r;
    }
    let mut world = f.lock().unwrap();
    world.posts += 1;
    if world.mode == Mode::LostWrite {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    world.next += 1;
    // GitHub stores Markdown bodies with CRLF line endings.
    let created =
        json!({"id":world.next,"body":body["body"].as_str().unwrap().replace('\n', "\r\n")});
    world.comments.push(created.clone());
    if world.mode == Mode::LostReply {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    (StatusCode::CREATED, Json(created)).into_response()
}
async fn add_labels(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Some(r) = guard(&f, &headers, true) {
        return r;
    }
    let mut world = f.lock().unwrap();
    world.label_writes += 1;
    for label in body["labels"].as_array().unwrap() {
        let label = label.as_str().unwrap().to_owned();
        if !world.labels.contains(&label) {
            world.labels.push(label);
        }
    }
    Json(json!([])).into_response()
}
async fn remove_label(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Path((_, label)): Path<(u64, String)>,
) -> Response {
    if let Some(r) = guard(&f, &headers, true) {
        return r;
    }
    let mut world = f.lock().unwrap();
    world.label_writes += 1;
    world.labels.retain(|l| *l != label);
    Json(json!([])).into_response()
}
async fn serve() -> (Fixture, String) {
    let fixture: Fixture = Arc::new(Mutex::new(World {
        mode: Mode::Online,
        state: "open".into(),
        labels: vec![],
        comments: vec![],
        posts: 0,
        label_writes: 0,
        next: 100,
    }));
    let base = format!("/repos/{OWNER}/{NAME}/issues");
    let app = Router::new()
        .route(&format!("{base}/{{number}}"), get(issue))
        .route(
            &format!("{base}/{{number}}/comments"),
            get(comments).post(comment),
        )
        .route(
            &format!("{base}/{{number}}/labels"),
            axum::routing::post(add_labels),
        )
        .route(
            &format!("{base}/{{number}}/labels/{{label}}"),
            delete(remove_label),
        )
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fixture, origin)
}

fn token() -> Credentials {
    Arc::new(|| Box::pin(async { Ok(Credential::Bearer(TOKEN.into())) }))
}
fn not_connected() -> Credentials {
    Arc::new(|| {
        Box::pin(async {
            Err(Error::Unavailable(Unavailable {
                message: "GitHub is not connected. Connect GitHub under Settings → Integrations."
                    .into(),
                status: None,
                request_sent: false,
            }))
        })
    })
}
fn delivery(store: &Shared, origin: &str, credentials: Credentials) -> GitHubDelivery<Shared> {
    GitHubDelivery::new(store.clone(), credentials, Some(origin.into()), None, None)
}

struct Setup {
    _root: tempfile::TempDir,
    path: PathBuf,
    store: Shared,
    run: String,
    reporter: String,
    other: String,
    task: String,
}
/// A GitHub-sourced ticket (#1 in fixture/issues) with a running run, an
/// admitted delivery reporter and one other specialist.
fn setup() -> Setup {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let saved = store
        .save_github_source(&json!({"repository":format!("{OWNER}/{NAME}")}))
        .unwrap();
    store
        .commit_github_sync(
            &saved["source"],
            &json!([{"id":7001,"number":1,"title":"Synthetic","body":"Synthetic body","state":"open","html_url":format!("https://github.com/{OWNER}/{NAME}/issues/1"),"updated_at":"2026-10-18T08:00:00Z"}]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    let task = store.snapshot().unwrap()["tasks"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // A coding run: repository, accepted brief and accepted plan.
    let repo = root.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
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
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let repository = store
        .create_repository(&json!({"name":"Fixture","path":repo,"targetBranch":"main"}))
        .unwrap();
    let revision = |store: &Store| store.get_task(&task).unwrap()["revision"].clone();
    store
        .assign_repository(
            &task,
            &json!({"revision":revision(&store),"repositoryId":repository["id"]}),
        )
        .unwrap();
    store
        .refine(&task, &json!({"revision":revision(&store),"sourceRevision":1,"brief":"Report progress on the issue","criteria":["Updates reach the issue once"]}))
        .unwrap();
    let current = store.get_task(&task).unwrap();
    let plan_source = store
        .save_plan_source(
            &store
                .prepare_plan_source(&task, current["revision"].as_u64().unwrap())
                .unwrap()
                .validate()
                .unwrap(),
        )
        .unwrap();
    let plan = store.save_implementation_plan(&task, &json!({"requestId":uuid_like(1),"revision":current["revision"],"sourceId":plan_source["id"],"content":{"schemaVersion":1,"goal":"Exercise delivery","scope":[current["brief"]],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Revision-bound evidence"],"criteria":[0],"validation":["This test"]}]}})).unwrap();
    store
        .decide_implementation_plan(plan["id"].as_str().unwrap(), &json!({"requestId":uuid_like(2),"revision":current["revision"],"sourceId":plan_source["id"],"decision":"accept"}))
        .unwrap();
    let run = store
        .queue(&task, &json!({"revision":revision(&store)}))
        .unwrap();
    let run = run["id"].as_str().unwrap().to_owned();
    let roles: Vec<Value> =
        serde_json::from_str(include_str!("../../crewroom-core/src/default-roles.json")).unwrap();
    let role = |id: &str| roles.iter().find(|r| r["id"] == id).unwrap().clone();
    let admit = |name: &str, role: Value, pid: u64| {
        let admitted = store
            .admit_assignment(
                &run,
                &json!({"name":name,"coordinator":false,"role":role,"model":{},"input":"Report"}),
            )
            .unwrap();
        let attempt = admitted["attempt"]["id"].as_str().unwrap().to_owned();
        store.claim_attempt(&attempt).unwrap();
        store
            .record_attempt_identity(&attempt, &json!({"hostPid":pid,"ompPid":pid+1,"hostUrl":"http://127.0.0.1:59999","sessionId":format!("fixture-{pid}"),"pane":"%1"}))
            .unwrap();
        attempt
    };
    let reporter = admit("Reporter", role("delivery-reporter"), 101);
    let other = admit("Writer", role("implementer"), 201);
    store.set_desired_state(&run, "running").unwrap();
    store.start_execution(&run).unwrap();
    Setup {
        _root: root,
        path,
        store: Arc::new(Mutex::new(store)),
        run,
        reporter,
        other,
        task,
    }
}
fn uuid_like(n: u8) -> String {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
}
fn with<T>(store: &Shared, f: impl FnOnce(&Store) -> T) -> T {
    f(&store.lock().unwrap())
}
fn publication(run: &str, merged: bool) -> Value {
    let mut record = json!({"id":"00000000-0000-4000-8000-0000000000aa","runId":run,"attemptId":null,"candidateId":"00000000-0000-4000-8000-0000000000bb","repository":{"provider":"github","owner":OWNER,"name":NAME,"id":42,"uuid":"github:42"},"revision":{"head":"fixture-head"},"push":{"state":"confirmed"},"pr":{"state":"confirmed","id":9,"providerState":"OPEN","url":format!("https://github.com/{OWNER}/{NAME}/pull/9")},"ready":false});
    if merged {
        record["pr"]["providerState"] = json!("MERGED");
        record["merged"] = json!({"commit":"fixture-merge","observedAt":"2026-10-18T10:00:00Z"});
    }
    record
}

#[tokio::test]
async fn comments_post_exactly_once_across_an_outage_a_lost_reply_and_a_restart() {
    let (fixture, origin) = serve().await;
    let s = setup();
    let mut delivery = delivery(&s.store, &origin, token());
    let input = json!({"kind":"milestone","eventKey":"impact-reviewed","body":"The overflow cause is identified."});
    // Only the delivery reporter may queue updates.
    assert!(
        delivery
            .prepare(&s.other, &input)
            .await
            .unwrap_err()
            .to_string()
            .contains("delivery reporter")
    );
    let queued = delivery.prepare(&s.reporter, &input).await.unwrap();
    assert_eq!(queued["state"], "pending");
    assert_eq!(queued["issueKey"], "#1");
    assert_eq!(
        delivery.prepare(&s.reporter, &input).await.unwrap()["id"],
        queued["id"],
        "the same event key is the same intent"
    );
    let mut different = input.clone();
    different["body"] = json!("Different request");
    assert!(
        delivery
            .prepare(&s.reporter, &different)
            .await
            .unwrap_err()
            .to_string()
            .contains("different content")
    );
    let mut extra = input.clone();
    extra["title"] = json!("Rewrite the issue");
    assert!(
        delivery
            .prepare(&s.reporter, &extra)
            .await
            .unwrap_err()
            .to_string()
            .contains("Unrecognized")
    );

    fixture.lock().unwrap().mode = Mode::Offline;
    delivery.flush().await.unwrap();
    let pending = &delivery.read(&s.run).unwrap()[0];
    assert_eq!(pending["state"], "pending");
    assert!(
        pending["reason"].as_str().unwrap().contains("503"),
        "{pending}"
    );
    assert_eq!(fixture.lock().unwrap().posts, 0);

    fixture.lock().unwrap().mode = Mode::LostReply;
    delivery.flush().await.unwrap();
    let uncertain = &delivery.read(&s.run).unwrap()[0];
    assert_eq!(uncertain["state"], "uncertain", "{uncertain}");
    assert_eq!(fixture.lock().unwrap().posts, 1);

    // Restart: a new store connection and a new delivery confirm the earlier
    // write by its marker instead of posting it again.
    fixture.lock().unwrap().mode = Mode::Online;
    drop(delivery);
    let reopened: Shared = Arc::new(Mutex::new(Store::open(&s.path).unwrap()));
    delivery = self::delivery(&reopened, &origin, token());
    delivery.flush().await.unwrap();
    let confirmed = &delivery.read(&s.run).unwrap()[0];
    assert_eq!(confirmed["state"], "confirmed", "{confirmed}");
    assert_eq!(confirmed["externalId"], 101);
    assert!(confirmed.get("reason").is_none());
    delivery.flush().await.unwrap();
    let world = fixture.lock().unwrap();
    assert_eq!((world.posts, world.comments.len()), (1, 1));
    let body = world.comments[0]["body"].as_str().unwrap();
    assert!(body.starts_with("**Crewroom milestone — agent report**"));
    assert!(body.contains("The overflow cause is identified."));
    assert!(body.contains(&format!(
        "Crewroom update {}",
        queued["id"].as_str().unwrap()
    )));
    drop(world);
    // Delivery never rewrites the imported requirements or source status.
    let task = with(&reopened, |st| st.get_task(&s.task).unwrap());
    assert_eq!(task["description"], "Synthetic body");
    assert_eq!(task["sourceStatus"], "open");
    // The coordinator is told about the receipt.
    let notices = with(&reopened, |st| st.delivery_notices(&s.run).unwrap());
    assert!(
        notices
            .iter()
            .any(|n| n["payload"]["kind"] == "github" && n["payload"]["state"] == "confirmed")
    );
}

#[tokio::test]
async fn an_unknown_write_stays_uncertain_without_duplicates_until_the_operator_retries() {
    let (fixture, origin) = serve().await;
    let s = setup();
    let delivery = delivery(&s.store, &origin, token());
    let first = delivery
        .prepare(
            &s.reporter,
            &json!({"kind":"milestone","eventKey":"started","body":"Investigating"}),
        )
        .await
        .unwrap();
    delivery
        .prepare(&s.reporter, &json!({"kind":"blocker","eventKey":"blocked-on-access","body":"Awaiting fixture access"}))
        .await
        .unwrap();
    fixture.lock().unwrap().mode = Mode::LostWrite;
    for _ in 0..3 {
        delivery.flush().await.unwrap();
    }
    let updates = delivery.read(&s.run).unwrap();
    assert_eq!(updates[0]["state"], "uncertain");
    assert!(
        updates[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no duplicate")
    );
    assert_eq!(
        updates[1]["state"], "pending",
        "later updates keep their order"
    );
    assert_eq!(fixture.lock().unwrap().posts, 1);
    // Only blocked or uncertain updates can be retried.
    assert!(delivery.retry(updates[1]["id"].as_str().unwrap()).is_err());
    fixture.lock().unwrap().mode = Mode::Online;
    let retried = delivery.retry(first["id"].as_str().unwrap()).unwrap();
    assert_eq!(retried["state"], "pending");
    delivery.flush().await.unwrap();
    let updates = delivery.read(&s.run).unwrap();
    assert_eq!(updates[0]["state"], "confirmed");
    assert_eq!(updates[1]["state"], "confirmed");
    let world = fixture.lock().unwrap();
    assert_eq!((world.posts, world.comments.len()), (3, 2));
    assert!(
        world.comments[1]["body"]
            .as_str()
            .unwrap()
            .starts_with("**Crewroom blocker — agent report**")
    );
}

#[tokio::test]
async fn refusals_and_missing_credentials_are_undelivered_with_a_reason_and_retry() {
    let (fixture, origin) = serve().await;
    let s = setup();
    let disconnected = delivery(&s.store, &origin, not_connected());
    disconnected
        .prepare(
            &s.reporter,
            &json!({"kind":"milestone","eventKey":"impact","body":"Impact noted"}),
        )
        .await
        .unwrap();
    disconnected.flush().await.unwrap();
    let waiting = &disconnected.read(&s.run).unwrap()[0];
    assert_eq!(waiting["state"], "pending");
    assert!(
        waiting["reason"]
            .as_str()
            .unwrap()
            .contains("not connected")
    );
    assert_eq!(fixture.lock().unwrap().posts, 0);

    let connected = delivery(&s.store, &origin, token());
    fixture.lock().unwrap().mode = Mode::Forbidden;
    connected.flush().await.unwrap();
    let blocked = connected.read(&s.run).unwrap()[0].clone();
    assert_eq!(blocked["state"], "blocked", "{blocked}");
    let reason = blocked["reason"].as_str().unwrap();
    assert!(
        reason.contains("HTTP 403") && reason.contains("Issues: Read and write"),
        "{reason}"
    );
    assert!(!blocked.to_string().contains(TOKEN));
    // A blocked update holds later ones; it is not retried by itself.
    connected.flush().await.unwrap();
    assert_eq!(
        fixture.lock().unwrap().posts,
        0,
        "a refused write is not repeated by itself"
    );
    fixture.lock().unwrap().mode = Mode::Online;
    connected.retry(blocked["id"].as_str().unwrap()).unwrap();
    connected.flush().await.unwrap();
    assert_eq!(connected.read(&s.run).unwrap()[0]["state"], "confirmed");
    assert_eq!(fixture.lock().unwrap().comments.len(), 1);
}

#[tokio::test]
async fn labels_apply_only_when_configured_and_the_merge_closes_the_issue_as_a_separate_fact() {
    let (fixture, origin) = serve().await;
    let s = setup();
    let delivery = delivery(&s.store, &origin, token());
    // Off by default: no in-progress label and nothing derived for review.
    assert!(
        delivery
            .prepare(
                &s.reporter,
                &json!({"kind":"in_progress","eventKey":"execution-started"})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("labels are off")
    );
    with(&s.store, |st| {
        st.save_publication(&publication(&s.run, false)).unwrap()
    });
    delivery.flush().await.unwrap();
    assert!(delivery.read(&s.run).unwrap().is_empty());
    assert_eq!(fixture.lock().unwrap().label_writes, 0);

    // Configured: in progress, then in review once the PR is confirmed open.
    with(&s.store, |st| {
        st.save_github_status_labels(
            &json!({"inProgress":"crewroom: in progress","inReview":"crewroom: in review"}),
        )
        .unwrap()
    });
    delivery
        .prepare(
            &s.reporter,
            &json!({"kind":"in_progress","eventKey":"execution-started"}),
        )
        .await
        .unwrap();
    delivery.flush().await.unwrap();
    let updates = delivery.read(&s.run).unwrap();
    let kinds: Vec<_> = updates
        .iter()
        .map(|u| (u["kind"].clone(), u["state"].clone()))
        .collect();
    assert!(
        kinds.contains(&(json!("in_progress"), json!("confirmed"))),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(json!("in_review"), json!("confirmed"))),
        "{kinds:?}"
    );
    assert_eq!(
        fixture.lock().unwrap().labels,
        vec!["crewroom: in review".to_owned()]
    );
    let before = with(&s.store, |st| st.get_run(&s.run).unwrap());

    // Merged, but GitHub has not closed the issue yet: undelivered, with why.
    with(&s.store, |st| {
        st.save_publication(&publication(&s.run, true)).unwrap()
    });
    delivery.reconcile().await.unwrap();
    let done = delivery
        .read(&s.run)
        .unwrap()
        .into_iter()
        .find(|u| u["kind"] == "done")
        .unwrap();
    assert_eq!(done["state"], "pending");
    assert_eq!(done["issueState"], "open");
    assert!(
        done["reason"]
            .as_str()
            .unwrap()
            .starts_with("Waiting for GitHub to close #1"),
        "{done}"
    );
    assert!(
        fixture.lock().unwrap().labels.is_empty(),
        "workflow labels are removed after the merge"
    );
    assert_eq!(
        with(&s.store, |st| st.get_task(&s.task).unwrap())["sourceStatus"],
        "open"
    );
    // Unforced passes do not re-read a waiting issue every few seconds.
    fixture.lock().unwrap().state = "closed".into();
    delivery.flush().await.unwrap();
    assert_eq!(
        delivery
            .read(&s.run)
            .unwrap()
            .into_iter()
            .find(|u| u["kind"] == "done")
            .unwrap()["state"],
        "pending"
    );
    // GitHub closed it through the PR's closing reference; Crewroom records it.
    delivery.reconcile().await.unwrap();
    let done = delivery
        .read(&s.run)
        .unwrap()
        .into_iter()
        .find(|u| u["kind"] == "done")
        .unwrap();
    assert_eq!(done["state"], "confirmed", "{done}");
    assert_eq!(done["issueState"], "closed");
    assert_eq!(done["closesOnMerge"], true);
    let task = with(&s.store, |st| st.get_task(&s.task).unwrap());
    assert_eq!(task["sourceStatus"], "closed");
    assert_eq!(task["sourceUpdatedAt"], "2026-10-18T10:00:00Z");
    // GitHub state is a source fact: workflow state and activity are untouched.
    let after = with(&s.store, |st| st.get_run(&s.run).unwrap());
    for key in ["status", "desiredState", "purpose"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    assert_eq!(
        fixture.lock().unwrap().posts,
        0,
        "no comment was invented for labels or closure"
    );
}
