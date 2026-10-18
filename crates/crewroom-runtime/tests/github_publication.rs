//! GitHub PR publication host against a simulated GitHub REST API (loopback
//! fixture, not github.com). Proves normalization into the provider-neutral
//! publication shape, keyed continuation of the same PR, hosted readiness at a
//! named head and base, paginated feedback and truthful refusals with remedies.
//! Live github.com evidence is recorded separately in docs/testing.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use crewroom_runtime::{
    providers::{Credential, Credentials, Error, github_pulls::GitHubPullClient},
    publication::assess,
    publication_remote::PullRequestHost,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "github_pat_synthetic0publication";
const OWNER: &str = "octo-fixture";
const NAME: &str = "sandbox";
const REPO_ID: i64 = 42;
const BASE: &str = "1111111111111111111111111111111111111111";
const HEAD: &str = "2222222222222222222222222222222222222222";
const NEXT: &str = "3333333333333333333333333333333333333333";
const MERGE: &str = "4444444444444444444444444444444444444444";

#[derive(Default)]
struct World {
    prs: Vec<Value>,
    /// Status code the next mutation answers with instead of applying it.
    refuse: Option<(u16, bool)>,
    push: bool,
    statuses: HashMap<String, Vec<Value>>,
    checks: HashMap<String, Vec<Value>>,
    issue_comments: Vec<Value>,
    review_comments: Vec<Value>,
    reviews: Vec<Value>,
    requests: Vec<String>,
}
type Fixture = Arc<Mutex<World>>;

fn authorized(fixture: &Fixture, headers: &HeaderMap, path: &str) -> Option<Response> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    assert_eq!(header("user-agent"), Some("crewroom"));
    assert_eq!(header("accept"), Some("application/vnd.github+json"));
    assert_eq!(header("x-github-api-version"), Some("2022-11-28"));
    fixture.lock().unwrap().requests.push(path.into());
    (header("authorization") != Some(&format!("Bearer {TOKEN}"))).then(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message":"Bad credentials"})),
        )
            .into_response()
    })
}
fn page(rows: &[Value], query: &HashMap<String, String>) -> Vec<Value> {
    let per: usize = query["per_page"].parse().unwrap();
    let page: usize = query["page"].parse().unwrap();
    rows.iter()
        .skip((page - 1) * per)
        .take(per)
        .cloned()
        .collect()
}
fn pr(number: i64, title: &str, body: &str, branch: &str, head: &str) -> Value {
    let repo = json!({"id":REPO_ID,"full_name":format!("{OWNER}/{NAME}")});
    json!({"number":number,"title":title,"body":body,"state":"open","draft":false,"merged_at":null,"merge_commit_sha":"9999999999999999999999999999999999999999","updated_at":"2026-10-18T09:00:00Z","html_url":format!("https://github.com/{OWNER}/{NAME}/pull/{number}"),"head":{"ref":branch,"sha":head,"repo":repo},"base":{"ref":"main","sha":BASE,"repo":repo}})
}

async fn repository(State(f): State<Fixture>, headers: HeaderMap) -> Response {
    if let Some(r) = authorized(&f, &headers, "repo") {
        return r;
    }
    let push = f.lock().unwrap().push;
    Json(json!({"id":REPO_ID,"full_name":format!("{OWNER}/{NAME}"),"permissions":{"pull":true,"push":push}})).into_response()
}
async fn pulls(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = authorized(&f, &headers, "pulls") {
        return r;
    }
    assert_eq!(query["state"], "all");
    let (owner, branch) = query["head"].split_once(':').unwrap();
    assert_eq!(owner, OWNER);
    let world = f.lock().unwrap();
    let rows = world
        .prs
        .iter()
        .filter(|p| p["head"]["ref"] == branch && p["base"]["ref"] == query["base"].as_str())
        .cloned()
        .collect::<Vec<_>>();
    Json(page(&rows, &query)).into_response()
}
fn refusal(world: &mut World) -> Option<Response> {
    let (status, limited) = world.refuse.take()?;
    let mut response = (
        StatusCode::from_u16(status).unwrap(),
        Json(json!({"message":"Synthetic refusal","errors":[{"message":"synthetic detail"}]})),
    )
        .into_response();
    if limited {
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", "0".parse().unwrap());
    }
    Some(response)
}
async fn create(State(f): State<Fixture>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if let Some(r) = authorized(&f, &headers, "create") {
        return r;
    }
    let mut world = f.lock().unwrap();
    if let Some(r) = refusal(&mut world) {
        return r;
    }
    assert_eq!(body["draft"], false);
    assert_eq!(body["base"], "main");
    let number = 5 + world.prs.len() as i64;
    let created = pr(
        number,
        body["title"].as_str().unwrap(),
        body["body"].as_str().unwrap(),
        body["head"].as_str().unwrap(),
        HEAD,
    );
    world.prs.push(created.clone());
    (StatusCode::CREATED, Json(created)).into_response()
}
async fn pull(State(f): State<Fixture>, headers: HeaderMap, Path(number): Path<i64>) -> Response {
    if let Some(r) = authorized(&f, &headers, "pull") {
        return r;
    }
    let world = f.lock().unwrap();
    match world.prs.iter().find(|p| p["number"] == number) {
        Some(p) => Json(p.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"message":"Not Found"}))).into_response(),
    }
}
async fn update(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Path(number): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(r) = authorized(&f, &headers, "update") {
        return r;
    }
    let mut world = f.lock().unwrap();
    if let Some(r) = refusal(&mut world) {
        return r;
    }
    let p = world
        .prs
        .iter_mut()
        .find(|p| p["number"] == number)
        .unwrap();
    p["title"] = body["title"].clone();
    p["body"] = body["body"].clone();
    p["updated_at"] = json!("2026-10-18T09:30:00Z");
    Json(p.clone()).into_response()
}
async fn comments(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Path((kind, number)): Path<(String, i64)>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = authorized(&f, &headers, "comments") {
        return r;
    }
    assert_eq!(number, 5);
    let world = f.lock().unwrap();
    let rows = if kind == "issues" {
        &world.issue_comments
    } else {
        &world.review_comments
    };
    Json(page(rows, &query)).into_response()
}
async fn reviews(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Path(number): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = authorized(&f, &headers, "reviews") {
        return r;
    }
    assert_eq!(number, 5);
    Json(page(&f.lock().unwrap().reviews, &query)).into_response()
}
fn review(id: i64, user: (&str, i64), state: &str, body: &str, commit: &str, at: &str) -> Value {
    json!({"id":id,"user":{"login":user.0,"id":user.1},"state":state,"body":body,"commit_id":commit,"submitted_at":at})
}
async fn statuses(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Path(sha): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = authorized(&f, &headers, "statuses") {
        return r;
    }
    let world = f.lock().unwrap();
    Json(page(
        world.statuses.get(&sha).map_or(&[][..], Vec::as_slice),
        &query,
    ))
    .into_response()
}
async fn checks(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Path(sha): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = authorized(&f, &headers, "checks") {
        return r;
    }
    let world = f.lock().unwrap();
    let rows = world.checks.get(&sha).map_or(&[][..], Vec::as_slice);
    Json(json!({"total_count":rows.len(),"check_runs":page(rows, &query)})).into_response()
}

async fn serve() -> (Fixture, String) {
    let fixture: Fixture = Arc::new(Mutex::new(World {
        push: true,
        ..World::default()
    }));
    let base = format!("/repos/{OWNER}/{NAME}");
    let app = Router::new()
        .route(&base, get(repository))
        .route(&format!("{base}/pulls"), get(pulls).post(create))
        .route(&format!("{base}/pulls/{{number}}"), get(pull).patch(update))
        .route(
            &format!("{base}/{{kind}}/{{number}}/comments"),
            get(comments),
        )
        .route(&format!("{base}/pulls/{{number}}/reviews"), get(reviews))
        .route(&format!("{base}/commits/{{sha}}/statuses"), get(statuses))
        .route(&format!("{base}/commits/{{sha}}/check-runs"), get(checks))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fixture, origin)
}
fn host(origin: &str, token: &'static str) -> PullRequestHost {
    let credentials: Credentials =
        Arc::new(move || Box::pin(async move { Ok(Credential::Bearer(token.into())) }));
    GitHubPullClient::new(
        &json!({"owner":OWNER,"name":NAME,"id":REPO_ID}),
        credentials,
        Some(origin),
    )
    .unwrap()
    .into()
}
fn unavailable(error: Error) -> (Option<u16>, bool, String) {
    match error {
        Error::Unavailable(u) => (u.status, u.request_sent, u.message),
        Error::Invalid(m) => panic!("expected an unavailable provider, got {m}"),
    }
}
fn check(id: i64, name: &str, conclusion: Option<&str>, head: &str, base: Option<&str>) -> Value {
    let prs = base.map_or(json!([]), |b| {
        json!([{"number":5,"head":{"sha":head,"ref":"crewroom/fix-1"},"base":{"sha":b,"ref":"main"}}])
    });
    json!({"id":id,"name":name,"head_sha":head,"status":if conclusion.is_some(){"completed"}else{"in_progress"},"conclusion":conclusion,"started_at":"2026-10-18T09:01:00Z","completed_at":conclusion.map(|_|"2026-10-18T09:05:00Z"),"pull_requests":prs})
}
fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

#[tokio::test]
async fn github_pull_requests_publish_continue_and_report_readiness_at_a_named_revision() {
    let (fixture, origin) = serve().await;
    let host = host(&origin, TOKEN);
    let verified = host.verify_repository().await.unwrap();
    assert_eq!(
        verified,
        json!({"uuid":"github:42","full_name":"octo-fixture/sandbox"})
    );

    // First publication: nothing claims the branch, so the PR is created.
    let branch = "crewroom/fix-1";
    assert!(host.pull_requests(branch, "main").await.unwrap().is_empty());
    let description = "Fix the thing.\n\nCloses #12\n\nCrewroom publication P-1";
    let created = host
        .create_pull_request(
            &json!({"title":"#12: Fix the thing","description":description,"branch":branch,"target":"main","head":HEAD}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        created,
        json!({"id":5,"title":"#12: Fix the thing","description":description,"state":"OPEN","updated_on":"2026-10-18T09:00:00Z",
            "source":{"branch":{"name":branch},"commit":{"hash":HEAD},"repository":{"uuid":"github:42"}},
            "destination":{"branch":{"name":"main"},"commit":{"hash":BASE},"repository":{"uuid":"github:42"}},
            "links":{"html":{"href":"https://github.com/octo-fixture/sandbox/pull/5"}},"draft":false,"merge_commit":null})
    );
    // The open test-merge commit is not reported as a merge.
    assert!(created["merge_commit"].is_null());

    // Keyed continuation: the branch search finds the same PR by its marker,
    // the description update keeps its number, and the pushed head is read back.
    let found = host.pull_requests(branch, "main").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["id"], 5);
    fixture.lock().unwrap().prs[0]["head"]["sha"] = json!(NEXT);
    let revised = "Fix the thing, revised.\n\nCloses #12\n\nCrewroom publication P-1";
    let updated = host
        .update_pull_request(
            5,
            &json!({"title":"#12: Fix the thing","description":revised}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(updated["id"], 5);
    assert_eq!(updated["description"], revised);
    assert_eq!(updated["source"]["commit"]["hash"], NEXT);
    assert_eq!(
        fixture.lock().unwrap().prs.len(),
        1,
        "no second PR was opened"
    );

    // Hosted evidence for the current head: a re-run replaces the earlier run
    // of the same check, and the check runs bind head and base for this PR.
    {
        let mut world = fixture.lock().unwrap();
        world.statuses.insert(
            NEXT.into(),
            vec![
                json!({"context":"lint","state":"success","target_url":"https://ci.example/lint","updated_at":"2026-10-18T09:06:00Z"}),
                json!({"context":"lint","state":"pending","target_url":null,"updated_at":"2026-10-18T09:02:00Z"}),
            ],
        );
        world.checks.insert(
            NEXT.into(),
            vec![
                check(100, "test", Some("failure"), NEXT, Some(BASE)),
                check(101, "test", Some("success"), NEXT, Some(BASE)),
                check(102, "build", Some("success"), NEXT, Some(BASE)),
            ],
        );
    }
    let observation = host.observe(5).await.unwrap();
    assert_eq!(
        observation["statuses"],
        json!([
            {"key":"build","name":"build","state":"SUCCESSFUL","updated_on":"2026-10-18T09:05:00Z"},
            {"key":"lint","name":"lint","state":"SUCCESSFUL","updated_on":"2026-10-18T09:06:00Z","url":"https://ci.example/lint"},
            {"key":"test","name":"test","state":"SUCCESSFUL","updated_on":"2026-10-18T09:05:00Z"}
        ])
    );
    let pipeline = &observation["pipelines"][0];
    assert_eq!(pipeline["build_number"], 102);
    assert_eq!(
        pipeline["state"],
        json!({"name":"COMPLETED","result":{"name":"SUCCESSFUL"}})
    );
    assert_eq!(pipeline["target"]["commit"]["hash"], NEXT);
    assert_eq!(pipeline["target"]["destination_commit"]["hash"], BASE);
    assert_eq!(pipeline["target"]["pullrequest"]["id"], 5);
    assert_eq!(
        pipeline["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["build", "test"]
    );

    // The provider-neutral readiness policy accepts the GitHub observation for
    // exactly this head and base, and names what blocks it otherwise.
    let record = json!({"candidateId":"c-2","revision":{"head":NEXT,"base":BASE},"branch":branch,"target":"main","description":revised,"title":"#12: Fix the thing",
        "repository":{"provider":"github","owner":OWNER,"name":NAME,"id":REPO_ID,"uuid":"github:42"},
        "policy":{"statusKeys":["lint","test"],"pipelineSteps":["test"],"optionalSteps":["build"]},
        "push":{"state":"confirmed"},"pr":{"state":"confirmed","id":5,"providerState":"OPEN","describedSha256":digest(revised)},
        "remoteSourceHead":NEXT,"remoteTargetHead":BASE,"observation":observation});
    let local = json!({"ready":true,"candidate":{"id":"c-2","revision":{"head":NEXT,"base":BASE}}});
    let ready = assess(&record, Some(&local), None);
    assert_eq!(ready["ready"], true, "{ready}");
    // FOR-56: a changed GitHub ticket names GitHub, never Jira, and latches review invalidation.
    let changed = assess(&record, Some(&local), Some("GitHub"));
    assert_eq!(changed["ready"], false);
    assert_eq!(changed["reviewInvalidated"], true);
    assert_eq!(
        changed["blockers"],
        json!(["GitHub requirements changed."]),
        "{changed}"
    );
    let mut stale = record.clone();
    stale["revision"]["head"] = json!(HEAD);
    stale["remoteSourceHead"] = json!(HEAD);
    let local_stale =
        json!({"ready":true,"candidate":{"id":"c-2","revision":{"head":HEAD,"base":BASE}}});
    let blocked = assess(&stale, Some(&local_stale), None);
    assert_eq!(blocked["ready"], false);
    assert!(
        blocked["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "The PR head, base or branch no longer matches local evidence."),
        "{blocked}"
    );
    // A check that does not name this PR cannot vouch for the base.
    fixture.lock().unwrap().checks.insert(
        NEXT.into(),
        vec![check(103, "test", Some("success"), NEXT, None)],
    );
    let unlinked = host.observe(5).await.unwrap();
    assert!(unlinked["pipelines"][0]["target"]["destination_commit"].is_null());
    let mut unbound = record.clone();
    unbound["observation"] = unlinked;
    let held = assess(&unbound, Some(&local), None);
    assert!(
        held["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "The required PR pipeline has not validated this head and base."),
        "{held}"
    );
    // A failed required check is a confirmed regression.
    fixture.lock().unwrap().checks.insert(
        NEXT.into(),
        vec![check(104, "test", Some("failure"), NEXT, Some(BASE))],
    );
    let mut failed = record.clone();
    failed["observation"] = host.observe(5).await.unwrap();
    let failed = assess(&failed, Some(&local), None);
    assert_eq!(failed["ready"], false);
    assert_eq!(failed["reviewInvalidated"], true);

    // FOR-55: a reviewer's standing changes-requested decision holds readiness
    // at the named revision; a comment-only review does not change it, and a
    // later approval or a dismissal releases it.
    fixture.lock().unwrap().checks.insert(
        NEXT.into(),
        vec![
            check(105, "test", Some("success"), NEXT, Some(BASE)),
            check(106, "build", Some("success"), NEXT, Some(BASE)),
        ],
    );
    let assess_now = |observation: Value| {
        let mut current = record.clone();
        current["observation"] = observation;
        assess(&current, Some(&local), None)
    };
    let alice = ("reviewer-fixture", 7);
    let bob = ("second-reviewer", 9);
    fixture.lock().unwrap().reviews = vec![
        review(
            700,
            alice,
            "CHANGES_REQUESTED",
            "Add a regression test.",
            NEXT,
            "2026-10-18T09:20:00Z",
        ),
        review(701, alice, "COMMENTED", "", NEXT, "2026-10-18T09:21:00Z"),
        review(702, bob, "APPROVED", "", NEXT, "2026-10-18T09:22:00Z"),
        json!({"id":703,"user":{"login":"author-fixture","id":8},"state":"PENDING","body":"draft","commit_id":NEXT}),
    ];
    let observed = host.observe(5).await.unwrap();
    assert_eq!(
        observed["reviews"],
        json!([
            {"id":700,"user":{"uuid":"github:7","display_name":"reviewer-fixture"},"state":"CHANGES_REQUESTED","commit":NEXT,"submitted_on":"2026-10-18T09:20:00Z"},
            {"id":702,"user":{"uuid":"github:9","display_name":"second-reviewer"},"state":"APPROVED","commit":NEXT,"submitted_on":"2026-10-18T09:22:00Z"}
        ])
    );
    let held = assess_now(observed);
    assert_eq!(held["ready"], false);
    assert_eq!(
        held["blockers"],
        json!([format!(
            "Reviewer reviewer-fixture requested changes on GitHub at this revision {}. It stands until they approve or the review is dismissed.",
            &NEXT[..12]
        )])
    );
    assert_eq!(
        held["reviewInvalidated"], false,
        "a human decision is not a regression of local evidence"
    );
    // Submitted at an earlier revision, the decision still stands on GitHub and
    // is named as such.
    fixture.lock().unwrap().reviews[0]["commit_id"] = json!(HEAD);
    let earlier = assess_now(host.observe(5).await.unwrap());
    assert!(
        earlier["blockers"][0]
            .as_str()
            .unwrap()
            .contains(&format!("at earlier revision {}", &HEAD[..12])),
        "{earlier}"
    );
    // The same reviewer's later approval releases readiness.
    fixture.lock().unwrap().reviews.push(review(
        704,
        alice,
        "APPROVED",
        "Thanks.",
        NEXT,
        "2026-10-18T09:30:00Z",
    ));
    assert_eq!(assess_now(host.observe(5).await.unwrap())["ready"], true);
    // A dismissed changes request no longer stands.
    {
        let mut world = fixture.lock().unwrap();
        world.reviews.pop();
        world.reviews[0]["state"] = json!("DISMISSED");
    }
    let dismissed = host.observe(5).await.unwrap();
    assert_eq!(dismissed["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(assess_now(dismissed.clone())["ready"], true);
    // A GitHub observation without review decisions is unknown, not approval.
    let mut unread = dismissed;
    unread.as_object_mut().unwrap().remove("reviews");
    assert_eq!(
        assess_now(unread)["blockers"],
        json!(["GitHub review decisions have not been observed."])
    );
    // A malformed review fails the observation instead of being skipped.
    fixture.lock().unwrap().reviews[0]["state"] = json!("SURPRISE");
    let (_, _, message) = unavailable(host.observe(5).await.unwrap_err());
    assert!(
        message.contains("unexpected pull request review"),
        "{message}"
    );
    fixture.lock().unwrap().reviews.clear();

    // Merged state is GitHub's own fact, with its merge commit.
    {
        let mut world = fixture.lock().unwrap();
        let p = &mut world.prs[0];
        p["state"] = json!("closed");
        p["merged_at"] = json!("2026-10-18T10:00:00Z");
        p["merge_commit_sha"] = json!(MERGE);
    }
    let merged = host.pull_request(5).await.unwrap();
    assert_eq!(merged["state"], "MERGED");
    assert_eq!(merged["merge_commit"]["hash"], MERGE);
    fixture.lock().unwrap().prs[0]["merged_at"] = Value::Null;
    assert_eq!(host.pull_request(5).await.unwrap()["state"], "DECLINED");
}

#[tokio::test]
async fn github_feedback_reads_every_page_of_conversation_and_review_comments() {
    let (fixture, origin) = serve().await;
    let host = host(&origin, TOKEN);
    {
        let mut world = fixture.lock().unwrap();
        world.issue_comments = (1..=120)
            .map(|id| json!({"id":id,"body":format!("comment {id}"),"user":{"login":"reviewer-fixture","id":7},"created_at":"2026-10-18T09:00:00Z","updated_at":"2026-10-18T09:00:00Z"}))
            .collect();
        world.review_comments = vec![
            json!({"id":5001,"body":"Inline note","user":{"login":"reviewer-fixture","id":7},"created_at":"2026-10-18T09:10:00Z","updated_at":"2026-10-18T09:11:00Z","path":"src/lib.rs","line":14,"original_line":12}),
            json!({"id":5002,"body":"Reply","user":{"login":"author-fixture","id":8},"created_at":"2026-10-18T09:12:00Z","updated_at":"2026-10-18T09:12:00Z","path":"src/lib.rs","line":null,"original_line":12,"in_reply_to_id":5001}),
        ];
    }
    let read = host.comments(5).await.unwrap();
    assert_eq!(read["state"], "complete");
    let comments = read["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 122);
    assert_eq!(
        comments[0],
        json!({"id":1,"created_on":"2026-10-18T09:00:00Z","updated_on":"2026-10-18T09:00:00Z","deleted":false,"content":{"raw":"comment 1"},"user":{"uuid":"github:7","display_name":"reviewer-fixture"}})
    );
    assert_eq!(
        comments[120]["inline"],
        json!({"path":"src/lib.rs","outdated":false,"to":14,"from":12})
    );
    assert_eq!(comments[121]["inline"]["outdated"], true);
    assert_eq!(comments[121]["parent"]["id"], 5001);
    let pages = fixture
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| *r == "comments")
        .count();
    assert_eq!(pages, 3, "two conversation pages and one review page");
}

#[tokio::test]
async fn github_feedback_includes_submitted_reviews_with_and_without_inline_comments() {
    let (fixture, origin) = serve().await;
    let host = host(&origin, TOKEN);
    let reviewer = ("reviewer-fixture", 7);
    {
        let mut world = fixture.lock().unwrap();
        world.issue_comments = vec![
            json!({"id":1,"body":"Conversation note","user":{"login":"reviewer-fixture","id":7},"created_at":"2026-10-18T09:00:00Z","updated_at":"2026-10-18T09:00:00Z"}),
        ];
        // An inline comment belongs to a comment-only review with no summary.
        world.review_comments = vec![
            json!({"id":5001,"pull_request_review_id":100,"body":"Inline note","user":{"login":"reviewer-fixture","id":7},"created_at":"2026-10-18T09:10:00Z","updated_at":"2026-10-18T09:10:00Z","path":"src/lib.rs","line":14,"original_line":14}),
        ];
        // 100 summary-less comment-only reviews fill the first page; the
        // decisions follow on the second page.
        world.reviews = (100..200)
            .map(|id| review(id, reviewer, "COMMENTED", "", HEAD, "2026-10-18T09:10:00Z"))
            .collect();
        world.reviews.extend([
            // A changes request with only a summary and no inline comments.
            // Its id equals conversation comment 1: a separate GitHub id space.
            review(1, reviewer, "CHANGES_REQUESTED", "Please add a regression test.", HEAD, "2026-10-18T09:20:00Z"),
            review(201, reviewer, "COMMENTED", "Overall this reads well.", HEAD, "2026-10-18T09:25:00Z"),
            review(202, ("second-reviewer", 9), "APPROVED", "", NEXT, "2026-10-18T09:30:00Z"),
            json!({"id":203,"user":{"login":"author-fixture","id":8},"state":"PENDING","body":"Unsubmitted draft","commit_id":NEXT}),
        ]);
    }
    let read = host.comments(5).await.unwrap();
    assert_eq!(read["state"], "complete");
    let items = read["comments"].as_array().unwrap();
    let reviews = items
        .iter()
        .filter(|c| c["kind"] == "review")
        .collect::<Vec<_>>();
    assert_eq!(
        items.len() - reviews.len(),
        2,
        "conversation and inline comments"
    );
    assert_eq!(
        reviews[0],
        &json!({"id":1,"kind":"review","created_on":"2026-10-18T09:20:00Z","updated_on":"2026-10-18T09:20:00Z","deleted":false,
            "content":{"raw":"Please add a regression test."},"review":{"state":"CHANGES_REQUESTED","commit":HEAD},
            "user":{"uuid":"github:7","display_name":"reviewer-fixture"}})
    );
    assert_eq!(
        reviews
            .iter()
            .map(|r| (
                r["id"].as_i64().unwrap(),
                r["review"]["state"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            (1, "CHANGES_REQUESTED"),
            (201, "COMMENTED"),
            (202, "APPROVED")
        ],
        "summary-less comment-only reviews and pending drafts are not feedback"
    );
    assert_eq!(reviews[2]["content"]["raw"], "");
    assert_eq!(reviews[2]["review"]["commit"], NEXT);
    let pages = fixture
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| *r == "reviews")
        .count();
    assert_eq!(pages, 2, "every review page is read");

    // A review read that fails part-way keeps what was read as partial.
    fixture.lock().unwrap().reviews[50]["submitted_at"] = json!("not a date");
    let partial = host.comments(5).await.unwrap();
    assert_eq!(partial["state"], "partial");
    assert_eq!(partial["comments"].as_array().unwrap().len(), 2);
    assert!(
        partial["reason"]
            .as_str()
            .unwrap()
            .contains("inconsistent PR comments")
    );
}

#[tokio::test]
async fn github_refusals_are_reported_with_remedies_and_never_skipped() {
    let (fixture, origin) = serve().await;
    let input = json!({"title":"t","description":"d","branch":"crewroom/fix-1","target":"main","head":HEAD});

    // A rejected token names Settings → Integrations; nothing was created.
    let rejected = host(&origin, "ghp_synthetic0revoked");
    let (status, _, message) = unavailable(rejected.verify_repository().await.unwrap_err());
    assert_eq!(status, Some(401));
    assert!(
        message.contains("Reconnect GitHub in Settings → Integrations"),
        "{message}"
    );
    assert!(!message.contains("ghp_synthetic0revoked"));
    let (status, sent, _) = unavailable(
        rejected
            .create_pull_request(&input, None)
            .await
            .unwrap_err(),
    );
    assert_eq!((status, sent), (Some(401), false));

    let host = host(&origin, TOKEN);
    // Missing permission: GitHub's 403 names the token permissions and leaves
    // the intent retryable (no PR was created).
    fixture.lock().unwrap().refuse = Some((403, false));
    let (status, sent, message) =
        unavailable(host.create_pull_request(&input, None).await.unwrap_err());
    assert_eq!((status, sent), (Some(403), false));
    assert!(
        message.contains("Pull requests: Read and write"),
        "{message}"
    );
    assert!(
        message.contains("GitHub said: \"Synthetic refusal: synthetic detail\""),
        "{message}"
    );
    // Not visible to the token (fine-grained repository selection).
    fixture.lock().unwrap().refuse = Some((404, false));
    let (status, sent, message) =
        unavailable(host.create_pull_request(&input, None).await.unwrap_err());
    assert_eq!((status, sent), (Some(404), false));
    assert!(message.contains("repository access"), "{message}");
    // Rate limits stay retryable as 429.
    fixture.lock().unwrap().refuse = Some((403, true));
    let (status, _, message) =
        unavailable(host.create_pull_request(&input, None).await.unwrap_err());
    assert_eq!(status, Some(429));
    assert!(message.contains("rate-limited"));
    // A validation refusal (rule, missing branch, existing PR) stays a sent,
    // blocking outcome for the operator to inspect.
    fixture.lock().unwrap().refuse = Some((422, false));
    let (status, sent, message) =
        unavailable(host.create_pull_request(&input, None).await.unwrap_err());
    assert_eq!((status, sent), (Some(422), true));
    assert!(message.contains("Inspect the repository"));
    assert!(fixture.lock().unwrap().prs.is_empty());

    // A read-only account is refused before any write.
    fixture.lock().unwrap().push = false;
    let (status, sent, message) = unavailable(host.verify_repository().await.unwrap_err());
    assert_eq!((status, sent), (Some(403), false));
    assert!(
        message.contains("cannot push to octo-fixture/sandbox"),
        "{message}"
    );
}
