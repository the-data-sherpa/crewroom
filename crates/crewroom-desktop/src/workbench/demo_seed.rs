//! Seeds a disposable data directory for visual review of the native workbench.
//! Recorded fixtures only: no agent, check or provider action is executed.
//! Run with `CREWROOM_DEMO_DIR=/new/private/dir cargo test -p crewroom-desktop seed_visual_review -- --ignored`.
use crewroom_core::{roles::Roles, store::Store};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().into()
}

#[test]
#[ignore = "Seeds recorded MH5 states for an actual rendered walkthrough in CREWROOM_INTAKE_DEMO_DIR."]
fn seed_intake_visual_review() {
    let root = std::path::PathBuf::from(std::env::var("CREWROOM_INTAKE_DEMO_DIR").unwrap());
    assert!(!root.exists());
    for path in [&root, &root.join("data"), &root.join("checkout")] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let checkout = root.join("checkout");
    std::fs::write(
        checkout.join("source.txt"),
        "Retry failed saves without losing edits.\n",
    )
    .unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "source.txt"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "Pinned source",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let s = Store::open(&root.join("data/workflow.sqlite")).unwrap();
    let repo = s
        .create_repository(
            &json!({"name":"Intake walkthrough","path":checkout,"targetBranch":"main"}),
        )
        .unwrap();
    let role = Roles::new(root.join("roles.json"))
        .read()
        .unwrap()
        .remove(0);
    let mut ids = json!({});
    for (kind, title) in [
        ("review", "Review retry implementation plan"),
        ("waiting", "Choose retry duration"),
        ("ready", "Ready plan for inline save errors"),
        ("idea", "Explore an offline indicator"),
    ] {
        let task=s.create_task(&json!({"title":title,"description":"Disposable native walkthrough. No provider actions.","repositoryId":repo["id"]})).unwrap();
        let task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Keep unsaved edits and make retries visible","criteria":["Failed saves retain edits","Retry behavior is checked"]})).unwrap();
        let source = s
            .save_plan_source(
                &s.prepare_plan_source(&id(&task), 2)
                    .unwrap()
                    .validate()
                    .unwrap(),
            )
            .unwrap();
        ids[kind] = task["id"].clone();
        if kind == "idea" {
            continue;
        }
        if kind == "waiting" {
            let run = s.queue(&id(&task), &json!({"revision":2})).unwrap();
            let admission=s.admit_assignment(&id(&run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Recorded native fixture"})).unwrap();
            let attempt = id(&admission["attempt"]);
            s.claim_attempt(&attempt).unwrap();
            s.record_attempt_identity(&attempt,&json!({"hostPid":123,"ompPid":124,"sessionId":"recorded-native-fixture","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
            s.set_desired_state(&id(&run), "running").unwrap();
            s.start_execution(&id(&run)).unwrap();
            s.record_intake_inspection(&attempt, &id(&source)).unwrap();
            s.ask_intake_questions(&attempt,"question",&json!({"sourceId":source["id"],"questions":[{"id":"retry","prompt":"How long should automatic retries continue?","rationale":"This limits background work and determines when to ask you to retry manually.","choices":["Up to 30 seconds","Up to 2 minutes"]}]})).unwrap();
            s.record_ownership(
                &attempt,
                true,
                Some("Recorded fixture; no OS process was launched"),
            )
            .unwrap();
            s.finish_pause(&id(&run)).unwrap();
        } else {
            let plan=s.save_implementation_plan(&id(&task),&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Keep edits safe when saving fails","scope":["Retain edits after a failed save","Show a retry action beside the error"],"exclusions":["Background synchronization"],"assumptions":["One operator edits a ticket"],"decisions":["Retry uses the same saved request identity"],"workPackages":[{"id":"save-recovery","name":"Preserve edits and show retry","dependsOn":[],"outputs":["A visible saved or failed state"],"criteria":[0],"validation":["Interrupt a save and retry after restart"]},{"id":"review","name":"Verify recovery independently","dependsOn":["save-recovery"],"outputs":["Review against the changed revision"],"criteria":[1],"validation":["Check keyboard use at 390px"]}]}})).unwrap();
            if kind == "ready" {
                s.decide_implementation_plan(&id(&plan),&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"decision":"accept"})).unwrap();
            }
        }
    }
    crewroom_core::authority::write_private(
        &root.join("ids.json"),
        &serde_json::to_vec_pretty(&ids).unwrap(),
    )
    .unwrap();
    crewroom_core::authority::write_private(
        &root.join("data/native-ui-state.json"),
        &serde_json::to_vec(&json!({"task":ids["waiting"],"tab":"Brief"})).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "Writes a demo data directory named by CREWROOM_DEMO_DIR for visual review."]
fn seed_visual_review() {
    let root = std::path::PathBuf::from(
        std::env::var("CREWROOM_DEMO_DIR").expect("Set CREWROOM_DEMO_DIR to a new directory."),
    );
    assert!(!root.exists(), "CREWROOM_DEMO_DIR must not exist yet.");
    for dir in [root.clone(), root.join("data"), root.join("checkout")] {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let checkout = root.join("checkout");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Demo",
            "-c",
            "user.email=demo@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Seed",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let s = Store::open(&root.join("data/workflow.sqlite")).unwrap();
    let repository = s
        .create_repository(
            &json!({"name":"conductor (demo)","path":checkout,"targetBranch":"main"}),
        )
        .unwrap();
    let issue = |n: &str, key: &str, summary: &str, status: &str| json!({"id":n,"key":key,"fields":{"summary":summary,"description":"Example issue for visual review.","status":{"name":status},"updated":"2026-10-04T12:00:00.000+0000"}});
    s.import_jira(&json!({"siteUrl":"https://example.atlassian.net","issues":[
        issue("1","DEMO-1010","Retry Kafka consumer offsets after a rebalance","In Progress"),
        issue("2","DEMO-1011","Show pipeline save errors inline","Selected for Development"),
        issue("3","DEMO-1012","Rate-limit the health endpoint","To Do"),
    ]}))
    .unwrap();
    let research = s
        .create_task(&json!({"title":"Compare retention options for audit logs","description":"Decide how long to keep audit logs and where."}))
        .unwrap();
    let tasks = s.snapshot().unwrap()["tasks"].as_array().unwrap().clone();
    let task = |key: &str| tasks.iter().find(|t| t["key"] == key).unwrap().clone();
    let briefs = [
        (
            "DEMO-1010",
            "Commit offsets only for the current consumer generation and retry after a rebalance completes.",
            vec![
                "Offsets are retried after a rebalance",
                "Stale-generation commits are dropped",
                "One warning is logged per rebalance",
            ],
        ),
        (
            "DEMO-1011",
            "Show a failed pipeline save next to the function that failed instead of a transient toast.",
            vec![
                "The failing function is highlighted",
                "The error stays until the function is edited",
            ],
        ),
        (
            "DEMO-1012",
            "Limit the health endpoint to 10 requests per second per client.",
            vec![
                "Excess requests receive 429 with Retry-After",
                "Leader health checks are exempt",
            ],
        ),
    ];
    let mut runs = vec![];
    for (key, brief, criteria) in briefs {
        let t = task(key);
        let t = s
            .assign_repository(
                &id(&t),
                &json!({"repositoryId":repository["id"],"revision":t["revision"]}),
            )
            .unwrap();
        let t = s.refine(&id(&t), &json!({"revision":t["revision"],"sourceRevision":t["sourceRevision"],"brief":brief,"criteria":criteria})).unwrap();
        runs.push(
            s.queue(&id(&t), &json!({"revision":t["revision"]}))
                .unwrap(),
        );
    }
    s.refine(&id(&research), &json!({"revision":research["revision"],"sourceRevision":research["sourceRevision"],"brief":"Compare object storage tiers against the current 30-day local retention.","criteria":["Options compared on cost, query speed and compliance","Each claim has a cited source"]})).unwrap();

    let roles = Roles::new(root.join("roles.json")).read().unwrap();
    let role = |id: &str| roles.iter().find(|r| r["id"] == id).unwrap().clone();
    let run = id(&runs[0]);
    s.claim_coding_slot(&run, true).unwrap();
    for (name, role_id, coordinator, model) in [
        ("Coordinator", "coordinator", true, "claude-opus-5-5"),
        ("Implementer", "implementer", false, "claude-sonnet-5-5"),
        (
            "Test architect",
            "test-architect",
            false,
            "claude-sonnet-5-5",
        ),
    ] {
        s.admit_assignment(&run, &json!({"name":name,"coordinator":coordinator,"role":role(role_id),"model":{"requested":{"model":model,"effort":"high"},"resolved":{"selector":model,"effort":"high"}},"input":format!("{name} for the accepted brief")})).unwrap();
    }
    s.set_desired_state(&run, "paused").unwrap();
    s.begin_pause(&run, None).unwrap();
    s.finish_pause(&run).unwrap();
    // Pending decisions for "Needs you": a recorded learning proposal and a changed Jira source.
    s.propose_learning(&json!({"runId":run,"attemptId":"demo-record","toolCallId":"demo-learning","target":{"kind":"role","roleId":"reviewer"},"baseVersion":1,"change":"Before approving, run the scenario tests named in the brief at the candidate revision and quote the result.","rationale":"The last two reviews approved candidates without running the rebalance scenario.","evidence":[{"kind":"steering","id":"demo-evidence"}]})).unwrap();
    let mut changed = issue(
        "2",
        "DEMO-1011",
        "Show pipeline save errors inline",
        "Selected for Development",
    );
    changed["fields"]["description"] = json!("Errors must name the failing function.");
    changed["fields"]["updated"] = json!("2026-10-04T13:00:00.000+0000");
    s.import_jira(&json!({"siteUrl":"https://example.atlassian.net","issues":[changed]}))
        .unwrap();
    println!("Seeded {}", root.display());
}
