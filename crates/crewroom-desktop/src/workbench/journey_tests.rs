//! Positive native operator journeys use real Git, the production service and
//! GPUI controls. Attempt identities are stopped records, never live agent proof.
//! Run after `cargo build --workspace --locked`, with `--ignored`.
use super::test_support::{Fixture, click, navigate, navigate_by_keyboard, open, wait};
mod external;
mod feature_recipes;
mod intake;
mod operator;
use super::*;
use crewroom_core::{authority::Authority, roles::Roles, store::Store};
use crewroom_runtime::{evidence::Evidence, repository::Repository};
use gpui_kit::{self as gpui, test::TestWindowExt};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
fn id(v: &Value) -> String {
    string(v, "id")
}
fn helper() -> PathBuf {
    let path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crewroom-process");
    assert!(
        path.is_file(),
        "Build the workspace binaries before running native journeys."
    );
    path.canonicalize().unwrap()
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Native fixture",
            "-c",
            "user.email=native@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "Git fixture failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}
fn options(_root: &Path) -> crewroom_service::ServiceOptions {
    crewroom_service::ServiceOptions {
        runtime: None,
        helper: helper(),
        host: "/usr/bin/false".into(),
        tmux: "/usr/bin/false".into(),
        jira: None,
        publication: vec![],
        environment: json!({}),
    }
}
struct Coding {
    task: Value,
    run: Value,
    candidate: Value,
    work: Value,
}
fn seed_coding(s: &Store, root: &Path, cancelled: bool) -> Coding {
    let root = root.canonicalize().unwrap();
    let checkout = root.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    std::fs::write(checkout.join("source.txt"), "Before native journey\n").unwrap();
    git(&checkout, &["add", "source.txt"]);
    git(&checkout, &["commit", "-m", "Initial fixture"]);
    let repository = s
        .create_repository(
            &json!({"name":"Native journey repository","path":checkout,"targetBranch":"main"}),
        )
        .unwrap();
    let task=s.create_task(&json!({"title":"Continue retained native work","repositoryId":repository["id"],"description":""})).unwrap();
    let task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Keep original evidence and ownership","criteria":["Preserve retained branch"]})).unwrap();
    super::test_support::accept_plan(s, &id(&task));
    let run = s
        .queue(&id(&task), &json!({"revision":task["revision"]}))
        .unwrap();
    let role = Roles::new(root.join("roles.json"))
        .read()
        .unwrap()
        .remove(0);
    let admitted=s.admit_assignment(&id(&run),&json!({"name":"Recorded specialist","coordinator":false,"role":role,"model":{"requested":{"model":"fixture/model","effort":"low"},"resolved":{"selector":"fixture/model","effort":"low"}},"input":"Stopped fixture identity"})).unwrap();
    let authority = Arc::new(Authority::acquire_for_execution(&root.join("data")).unwrap());
    let shared = Arc::new(Mutex::new(
        Store::open(&root.join("data/workflow.sqlite")).unwrap(),
    ));
    let repository =
        Repository::new(shared, root.join("data/worktrees"), helper(), authority).unwrap();
    let evidence = Evidence::new(repository.clone(), root.join("data/artifacts")).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let work = rt
        .block_on(repository.prepare(&id(&run), &json!({"mode":"new"})))
        .unwrap();
    let owned = Path::new(work["path"].as_str().unwrap());
    std::fs::write(
        owned.join("source.txt"),
        "Retained 日本語 🦀 implementation\n",
    )
    .unwrap();
    git(owned, &["add", "source.txt"]);
    git(owned, &["commit", "-m", "Recorded native candidate"]);
    // These impossible-on-macOS PIDs are never probed or signalled. Ownership is
    // explicitly stopped before the production service is allowed to open.
    let attempt = id(&admitted["attempt"]);
    s.claim_attempt(&attempt).unwrap();
    s.record_attempt_identity(&attempt,&json!({"hostPid":2_000_000_001u32,"ompPid":2_000_000_002u32,"sessionId":attempt,"hostUrl":"http://127.0.0.1:9","pane":"%999"})).unwrap();
    s.record_ownership(&attempt, true, Some("Recorded fixture; no agent executed"))
        .unwrap();
    s.set_desired_state(&id(&run), if cancelled { "cancelled" } else { "paused" })
        .unwrap();
    s.begin_pause(&id(&run), None).unwrap();
    s.finish_pause(&id(&run)).unwrap();
    evidence.configure(&id(&run), &json!([])).unwrap();
    let candidate = rt
        .block_on(evidence.register_candidate(&id(&run), &[attempt], None, None))
        .unwrap();
    let current = rt.block_on(repository.status(&id(&run))).unwrap();
    s.checkpoint_repository(&id(&run),&json!({"id":uuid::Uuid::new_v4().to_string(),"at":crewroom_core::timestamp(crewroom_core::now_ms()),"reason":"interrupted","revision":current["revision"]})).unwrap();
    rt.block_on(async { repository.close().unwrap().wait().await.unwrap() });
    Coding {
        task,
        run,
        candidate,
        work,
    }
}
fn select(
    cx: &mut gpui::TestAppContext,
    h: gpui::AnyWindowHandle,
    v: &Entity<Workbench>,
    e: &async_channel::Receiver<Event>,
    task: &Value,
) {
    wait(cx, h, v, e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(cx, h, SharedString::from(format!("ticket-{}", id(task))));
    wait(cx, h, v, e, |v, _| {
        v.observation.runtime_fresh && v.observation.result_fresh
    });
}
#[gpui::test]
#[ignore = "Real Git journey requires workspace binaries; run the native journey gate."]
fn native_journey_cancelled_continuation_preserves_branch_diff_lineage_and_reopen(
    cx: &mut gpui::TestAppContext,
) {
    let mut seed = None;
    let mut f = Fixture::configured(|s, root| seed = Some(seed_coding(s, root, true)), options);
    let seed = seed.unwrap();
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &seed.task);
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    navigate_by_keyboard(cx, h, "Results");
    super::acceptance_tests::keyboard_activate(cx, h, "view-candidate-diff");
    wait(cx, h, &v, &e, |v, _| {
        v.observation.detail.as_ref().is_some_and(|d|matches!(&d.result,Some(Ok(x)) if x["patch"].as_str().is_some_and(|p|p.contains("+Retained 日本語 🦀 implementation"))))
    });
    let content = cx.read(|cx| {
        v.read(cx)
            .observation
            .detail
            .as_ref()
            .unwrap()
            .content
            .clone()
            .unwrap()
    });
    cx.update_window(h, |_, window, cx| {
        content.update(cx, |input, cx| input.focus(window, cx));
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.input("Must remain read only", cx);
        assert!(content.read(cx).value().contains("Retained 日本語"));
    })
    .unwrap();
    super::acceptance_tests::keyboard_activate(cx, h, "close-evidence");
    navigate_by_keyboard(cx, h, "Conversation");
    super::acceptance_tests::keyboard_activate(cx, h, "continue-later");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs").len() == 2
    });
    let workspace = f.get("/workspace");
    let next = records(&workspace, "runs")
        .into_iter()
        .find(|r| r["id"] != seed.run["id"])
        .unwrap();
    assert_eq!(next["continuedFrom"]["runId"], seed.run["id"]);
    assert_eq!(next["continuedFrom"]["branch"], seed.work["branch"]);
    wait(cx, h, &v, &e, |v, _| {
        v.observation.id.as_deref() == next["id"].as_str() && v.observation.runtime_fresh
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("resume-run").is_none());
        assert!(
            window
                .try_find(SharedString::from(format!("linked-run-{}", id(&seed.run))))
                .is_some()
        );
    })
    .unwrap();
    super::acceptance_tests::keyboard_activate(cx, h, &format!("linked-run-{}", id(&seed.run)));
    wait(cx, h, &v, &e, |v, _| {
        v.observation.id.as_deref() == seed.run["id"].as_str() && v.observation.result_fresh
    });
    assert_eq!(
        f.get(&format!("/runs/{}/result", id(&seed.run)))["candidates"][0],
        seed.candidate
    );
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("continue-later").is_none());
        assert!(
            window
                .try_find(SharedString::from(format!("linked-run-{}", id(&next))))
                .is_some()
        );
    })
    .unwrap();
    f.restart();
    wait(cx, h, &v, &e, |v, _| {
        v.connected && records(&v.workspace, "runs").len() == 2
    });
    let (h2, v2, e2) = open(cx, &f);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.observation.id.as_deref() == seed.run["id"].as_str() && v.observation.result_fresh
    });
    assert_eq!(
        f.get(&format!("/runs/{}/runtime", id(&seed.run)))["run"]["status"],
        "cancelled"
    );
    assert_eq!(
        git(
            Path::new(seed.work["repository"]["path"].as_str().unwrap()),
            &["show", "HEAD:source.txt"]
        ),
        "Before native journey"
    );
}

#[gpui::test]
#[ignore = "Real Git journey requires workspace binaries; run the native journey gate."]
fn native_journey_target_refresh_and_reasoned_conflict_retry_keep_revision_history(
    cx: &mut gpui::TestAppContext,
) {
    for conflict in [false, true] {
        let mut seed = None;
        let f = Fixture::configured(
            |s, root| {
                let current = seed_coding(s, root, false);
                let checkout = Path::new(current.work["repository"]["path"].as_str().unwrap());
                if conflict {
                    std::fs::write(checkout.join("source.txt"), "Conflicting target version\n")
                        .unwrap();
                    std::fs::write(
                        checkout.join("AGENTS.md"),
                        "Changed accepted project instructions\n",
                    )
                    .unwrap();
                } else {
                    std::fs::write(checkout.join("target.txt"), "New target file\n").unwrap();
                }
                git(checkout, &["add", "."]);
                git(checkout, &["commit", "-m", "Move fixture target"]);
                seed = Some(current);
            },
            options,
        );
        let seed = seed.unwrap();
        let (h, v, e) = open(cx, &f);
        select(cx, h, &v, &e, &seed.task);
        navigate(cx, h, "Results");
        click(cx, h, "target-refresh");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy && !records(&v.observation.result, "refreshes").is_empty()
        });
        let result = f.get(&format!("/runs/{}/result", id(&seed.run)));
        if conflict {
            assert_eq!(
                result["refreshes"][0]["resolutionWorkspace"]["state"],
                "operator_required"
            );
            assert_eq!(result["candidates"].as_array().unwrap().len(), 1);
            assert!(!result["assessment"]["ready"].as_bool().unwrap_or(false));
            let path = format!(
                "/runs/{}/target-refresh/{}",
                id(&seed.run),
                string(&result["refreshes"][0], "requestId")
            );
            wait(cx, h, &v, &e, |v, _| {
                v.observation.result["refreshes"][0]["resolutionWorkspace"]["state"]
                    == "operator_required"
                    && v.collaboration.forms.contains_key(&path)
            });
            cx.update_window(h, |_, window, cx| {
                let input = v.read(cx).collaboration.forms[&path].fields.areas["reason"].clone();
                input.update(cx, |i, cx| i.focus(window, cx));
                window.input(
                    "Reviewed changed instructions; preserve both retained conflicts.",
                    cx,
                );
            })
            .unwrap();
            click(cx, h, "target-refresh");
            wait(cx, h, &v, &e, |v, _| {
                !v.busy && records(&v.observation.result, "refreshes").len() == 2
            });
            let result = f.get(&format!("/runs/{}/result", id(&seed.run)));
            assert_eq!(result["candidates"].as_array().unwrap().len(), 1);
            assert_eq!(
                result["refreshes"][0]["supersededBy"]["requestId"],
                result["refreshes"][1]["requestId"]
            );
            assert_eq!(
                result["refreshes"][1]["supersedes"]["reason"],
                "Reviewed changed instructions; preserve both retained conflicts."
            );
            for refresh in records(&result, "refreshes") {
                assert!(
                    Path::new(refresh["resolutionWorkspace"]["path"].as_str().unwrap()).exists()
                );
            }
        } else {
            assert_eq!(result["candidates"].as_array().unwrap().len(), 2);
            assert_eq!(
                result["candidates"][1]["refreshedFrom"],
                seed.candidate["id"]
            );
            assert_eq!(result["refreshes"][0]["state"], "completed");
            assert!(!result["assessment"]["ready"].as_bool().unwrap_or(false));
            click(
                cx,
                h,
                SharedString::from(format!("candidate-{}", id(&seed.candidate))),
            );
            click(cx, h, "view-candidate-diff");
            wait(cx, h, &v, &e, |v, _| {
                v.observation.detail.as_ref().is_some_and(|d|matches!(&d.result,Some(Ok(x)) if x["head"]==seed.candidate["revision"]["head"]))
            });
        }
        assert_eq!(
            f.get(&format!("/runs/{}/runtime", id(&seed.run)))["run"]["desiredState"],
            "paused"
        );
        cx.update_window(h, |_, window, _| window.remove_window())
            .unwrap();
    }
}

mod provider;
#[gpui::test]
#[ignore = "Native provider journey uses workspace binaries; run the native journey gate."]
fn native_journey_jira_refresh_outage_retains_operator_brief_and_never_queues(
    cx: &mut gpui::TestAppContext,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mode = Arc::new(AtomicUsize::new(0));
    let current = mode.clone();
    let provider = provider::Provider::new(move |request| {
        assert!(request.starts_with("GET /rest/api/3/search/jql?"));
        if current.load(Ordering::SeqCst) == 2 {
            return (503, json!({"error":"Scripted outage"}));
        }
        let changed = current.load(Ordering::SeqCst) == 1;
        let issue = |id: &str, title: &str| json!({"id":id,"key":format!("SIM-{id}"),"fields":{"summary":title,"description":"Imported description","updated":if changed{"2026-10-04T10:00:00Z"}else{"2026-10-03T10:00:00Z"},"status":{"id":if changed{"3"}else{"1"},"name":if changed{"In Progress"}else{"To Do"}}}});
        (
            200,
            json!({"isLast":true,"issues":[issue("2",if changed{"Updated Jira title"}else{"First Jira task"}),issue("1","Second Jira task")]}),
        )
    });
    let origin = provider.origin.clone();
    let f = Fixture::configured(
        |_, _| {},
        move |root| {
            let mut options = options(root);
            options.jira=Some(crewroom_runtime::providers::jira::JiraClient::new(&json!({"siteUrl":"https://fixture.atlassian.net","cloudId":"00000000-0000-4000-8000-000000000022","filterId":"12345","assigneeAccountId":"operator-1","name":"Assigned work"}),provider::credentials(),Some(&origin)).unwrap());
            options
        },
    );
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "import-jira");
    click(cx, h, "refresh-jira");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["jira"]["state"] == "connected"
    });
    let tasks = records(&f.get("/workspace"), "tasks");
    assert_eq!(
        tasks.iter().map(|t| string(t, "key")).collect::<Vec<_>>(),
        ["SIM-2", "SIM-1"]
    );
    let task = &tasks[0];
    click(cx, h, SharedString::from(format!("ticket-{}", id(task))));
    cx.update_window(h, |_, window, cx| {
        let draft = &v.read(cx).drafts[&id(task)];
        let brief = draft.brief.clone();
        let criteria = draft.criteria.clone();
        brief.update(cx, |i, cx| i.focus(window, cx));
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.input("Keep my accepted direction", cx);
        criteria.update(cx, |i, cx| i.focus(window, cx));
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.input("Keep local acceptance criteria", cx);
    })
    .unwrap();
    click(cx, h, "save-brief");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "tasks")[0]["brief"] == "Keep my accepted direction"
    });
    mode.store(1, Ordering::SeqCst);
    navigate(cx, h, "import-jira");
    click(cx, h, "refresh-jira");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "tasks")[0]["title"] == "Updated Jira title"
    });
    let workspace = f.get("/workspace");
    assert_eq!(workspace["tasks"][0]["brief"], "Keep my accepted direction");
    assert_eq!(workspace["tasks"][0]["jiraStatus"], "In Progress");
    assert!(records(&workspace, "runs").is_empty());
    mode.store(2, Ordering::SeqCst);
    click(cx, h, "refresh-jira");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["jira"]["state"] == "unavailable"
    });
    let outage = f.get("/workspace");
    assert_eq!(outage["tasks"], workspace["tasks"]);
    assert_eq!(
        outage["jira"]["lastSyncedAt"],
        workspace["jira"]["lastSyncedAt"]
    );
    assert!(records(&outage, "runs").is_empty());
    assert!(!outage.to_string().contains("synthetic-native-provider"));
}

#[gpui::test]
#[ignore = "Native research journey uses workspace binaries; run the native journey gate."]
fn native_journey_research_keeps_verified_rejected_superseded_citations_and_readonly_artifacts(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut run = Value::Null;
    let mut artifacts = vec![];
    let mut f = Fixture::configured(
        |s, root| {
            let root = root.canonicalize().unwrap();
            let checkout = root.join("research-source");
            std::fs::create_dir(&checkout).unwrap();
            git(&checkout, &["init", "-b", "main"]);
            std::fs::write(
                checkout.join("queues.txt"),
                "FIFO preserves arrival order.\nPriority can starve low-ranked work.\n",
            )
            .unwrap();
            git(&checkout, &["add", "queues.txt"]);
            git(&checkout, &["commit", "-m", "Research source"]);
            let repo = s
                .create_repository(
                    &json!({"name":"Research source","path":checkout,"targetBranch":"main"}),
                )
                .unwrap();
            task=s.create_task(&json!({"title":"Compare queue choices","repositoryId":repo["id"],"description":""})).unwrap();
            task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Compare queues with explicit limitations","criteria":["Compare both choices"]})).unwrap();
            run=s.queue_research(&id(&task),&json!({"revision":task["revision"],"requestId":uuid::Uuid::new_v4().to_string()})).unwrap();
            let role = Roles::new(root.join("roles.json"))
                .read()
                .unwrap()
                .remove(0);
            let a=s.admit_assignment(&id(&run),&json!({"name":"Recorded researcher","coordinator":true,"role":role,"model":{"requested":{"model":"fixture/model","effort":"low"},"resolved":{"selector":"fixture/model","effort":"low"}},"input":"Recorded research submission"})).unwrap();
            let attempt = id(&a["attempt"]);
            s.claim_attempt(&attempt).unwrap();
            s.record_attempt_identity(&attempt,&json!({"hostPid":2_000_000_003u32,"ompPid":2_000_000_004u32,"sessionId":attempt,"hostUrl":"http://127.0.0.1:9","pane":"%999"})).unwrap();
            s.set_desired_state(&id(&run), "running").unwrap();
            s.start_execution(&id(&run)).unwrap();
            let authority = Arc::new(Authority::acquire_for_execution(&root.join("data")).unwrap());
            let shared = Arc::new(Mutex::new(
                Store::open(&root.join("data/workflow.sqlite")).unwrap(),
            ));
            let work =
                Repository::new(shared, root.join("data/worktrees"), helper(), authority).unwrap();
            let evidence = Evidence::new(work.clone(), root.join("data/artifacts")).unwrap();
            let research = crewroom_runtime::evidence::Research::new(evidence);
            let rt = tokio::runtime::Runtime::new().unwrap();
            let mut input = json!({"title":"Queue design assessment","kind":"design","body":"# Comparison\nFIFO is predictable; priority may starve lower-ranked work.\n","limitations":"Fixture comparison; no production workloads.","citations":[{"path":"queues.txt","commit":run["researchSource"]["commit"],"startLine":1,"endLine":2,"excerpt":"FIFO preserves arrival order.\nPriority can starve low-ranked work."}],"coverage":[{"criterionId":0,"section":"Comparison"}]});
            artifacts.push(
                rt.block_on(research.submit(&attempt, "first", &input))
                    .unwrap(),
            );
            input["citations"][0]["excerpt"] = json!("Incorrect quoted evidence");
            artifacts.push(
                rt.block_on(research.submit(&attempt, "rejected", &input))
                    .unwrap(),
            );
            assert!(
                research
                    .request_completion(&attempt, "premature", &json!({}))
                    .is_err()
            );
            input["citations"][0]["excerpt"] =
                json!("FIFO preserves arrival order.\nPriority can starve low-ranked work.");
            artifacts.push(
                rt.block_on(research.submit(&attempt, "corrected", &input))
                    .unwrap(),
            );
            research
                .request_completion(&attempt, "complete", &json!({}))
                .unwrap();
            s.record_ownership(
                &attempt,
                true,
                Some("Recorded submissions; no live researcher executed"),
            )
            .unwrap();
            s.begin_pause(&id(&run), None).unwrap();
            s.finish_pause(&id(&run)).unwrap();
            research.complete(&id(&run)).unwrap();
            rt.block_on(async { work.close().unwrap().wait().await.unwrap() });
        },
        options,
    );
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Results");
    let result = f.get(&format!("/runs/{}/result", id(&run)));
    assert_eq!(result["research"]["assessment"]["completed"], true);
    assert_eq!(result["research"]["artifacts"].as_array().unwrap().len(), 3);
    assert_eq!(
        result["research"]["artifacts"][0]["supersededBy"],
        artifacts[1]["id"]
    );
    assert_eq!(result["research"]["artifacts"][1]["state"], "rejected");
    assert_eq!(
        result["research"]["artifacts"][2]["citations"][0]["verified"],
        true
    );
    for artifact in &artifacts {
        click(
            cx,
            h,
            SharedString::from(format!("research-artifact-{}", id(artifact))),
        );
        wait(cx, h, &v, &e, |v, _| {
            v.observation.detail.as_ref().is_some_and(|d|matches!(&d.result,Some(Ok(x)) if x["content"].as_str().is_some_and(|body|body.contains("FIFO is predictable"))))
        });
        click(cx, h, "close-evidence");
    }
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        for forbidden in [
            "view-candidate-diff",
            "target-refresh",
            "reconcile-publication",
            "open-pull-request",
            "continue-later",
        ] {
            assert!(
                window.try_find(forbidden).is_none(),
                "Research exposed coding control {forbidden}"
            );
        }
    })
    .unwrap();
    f.restart();
    let (h2, v2, e2) = open(cx, &f);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.observation.result_fresh
            && v.observation.result["research"]["assessment"]["completed"] == true
    });
    assert_eq!(
        f.get(&format!("/runs/{}/result", id(&run)))["research"],
        result["research"]
    );
}

#[gpui::test]
#[ignore = "Native publication journey uses workspace binaries; run the native journey gate."]
fn native_journey_publication_hosted_checks_and_feedback_outage_remain_separate(
    cx: &mut gpui::TestAppContext,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mode = Arc::new(AtomicUsize::new(0));
    let current = mode.clone();
    let record = Arc::new(Mutex::new(Value::Null));
    let server_record = record.clone();
    let identity = json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"});
    let repo_identity = identity.clone();
    let provider = provider::Provider::new(move |request| {
        assert!(
            request.starts_with("GET "),
            "This recorded publication journey permits provider reads only: {request}"
        );
        let path = request
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap();
        let record = server_record.lock().unwrap();
        let mode = current.load(Ordering::SeqCst);
        if path.ends_with("/repository") {
            return (
                200,
                json!({"uuid":repo_identity["uuid"],"full_name":"fixture/repository"}),
            );
        }
        if path.ends_with("/comments") {
            if mode == 2 {
                return (503, json!({}));
            }
            return (
                200,
                json!({"values":[{"id":7,"content":{"raw":if mode==3{"Clarify the updated limitation."}else{"Explain the retained limitation."}},"created_on":"2026-10-03T01:00:00Z","updated_on":if mode==3{"2026-10-04T12:00:00Z"}else{"2026-10-03T01:00:00Z"},"user":{"uuid":"reviewer","display_name":"Fixture reviewer"},"inline":{"path":"source.txt","to":1},"deleted":false,"pullrequest":{"id":9}}]}),
            );
        }
        if path.ends_with("/pullrequests/9") {
            return (
                200,
                json!({"id":9,"state":"OPEN","title":record["title"],"description":record["description"],"updated_on":"2026-10-04T01:00:00Z","source":{"branch":{"name":record["branch"]},"commit":{"hash":record["revision"]["head"]},"repository":{"uuid":repo_identity["uuid"]}},"destination":{"branch":{"name":"main"},"commit":{"hash":record["revision"]["base"]},"repository":{"uuid":repo_identity["uuid"]}},"links":{"html":{"href":"https://bitbucket.org/fixture/repository/pull-requests/9"}}}),
            );
        }
        if path.ends_with("/statuses") {
            return (
                200,
                json!({"values":[{"key":"build","state":"SUCCESSFUL","name":"Build","url":"https://example.invalid/build","updated_on":"2026-10-04T01:00:00Z"}]}),
            );
        }
        if path.ends_with("/pipelines/") {
            return (
                200,
                json!({"values":[{"uuid":"pipeline-1","build_number":if mode==1{2}else{1},"created_on":"2026-10-04T01:00:00Z","state":{"name":"COMPLETED","result":{"name":"SUCCESSFUL"}},"target":{"type":"pipeline_pullrequest_target","commit":{"hash":record["revision"]["head"]},"destination_commit":{"hash":record["revision"]["base"]},"source":record["branch"],"destination":"main","pullrequest":{"id":9},"selector":{"type":"pull-requests","pattern":"**"}}}]}),
            );
        }
        if path.ends_with("/steps/") {
            return (
                200,
                json!({"values":[{"uuid":"step-1","name":"Tests","state":{"name":"COMPLETED","result":{"name":if mode==1{"SKIPPED"}else{"SUCCESSFUL"}}}}]}),
            );
        }
        panic!("Unexpected provider request: {request}");
    });
    let origin = provider.origin.clone();
    let connection_identity = identity.clone();
    let mut seed = None;
    let mut f = Fixture::configured(
        |s, root| {
            let value = seed_coding(s, root, false);
            let root = root.canonicalize().unwrap();
            let checkout = Path::new(value.work["repository"]["path"].as_str().unwrap());
            git(
                checkout,
                &[
                    "clone",
                    "--bare",
                    checkout.to_str().unwrap(),
                    root.join("remote.git").to_str().unwrap(),
                ],
            );
            let publication_id = uuid::Uuid::new_v4().to_string();
            let publication = json!({"id":publication_id,"runId":value.run["id"],"repositoryPath":checkout,"repository":identity,"branch":value.work["branch"],"target":"main","candidateId":value.candidate["id"],"revision":value.candidate["revision"],"title":"Recorded fixture PR","description":format!("Crewroom publication {publication_id}"),"policy":{"statusKeys":["build"],"pipelineSteps":["Tests"],"optionalSteps":[]},"createdAt":"2026-10-03T01:00:00Z","push":{"state":"confirmed"},"pr":{"id":9,"state":"confirmed","providerState":"OPEN","url":"https://bitbucket.org/fixture/repository/pull-requests/9"}});
            s.save_publication(&publication).unwrap();
            *record.lock().unwrap() = publication;
            seed = Some(value);
        },
        move |root| {
            let mut o = options(root);
            let client = crewroom_runtime::providers::bitbucket::BitbucketClient::new(
                &connection_identity,
                provider::credentials(),
                Some(&origin),
            )
            .unwrap();
            o.publication.push(
                crewroom_runtime::publication_remote::Connection::new(
                    &root.join("checkout"),
                    &connection_identity,
                    client,
                    &json!({"statusKeys":["build"],"pipelineSteps":["Tests"]}),
                    Some(&root.join("remote.git")),
                )
                .unwrap(),
            );
            o
        },
    );
    let seed = seed.unwrap();
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &seed.task);
    navigate(cx, h, "Results");
    wait(cx, h, &v, &e, |v, _| {
        v.observation.result["publication"]["record"]["feedback"]["comments"][0]["content"]["raw"]
            == "Explain the retained limitation."
    });
    click(cx, h, "reconcile-publication");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.observation.result["publication"]["record"]["observation"]["statuses"][0]["state"]
                == "SUCCESSFUL"
    });
    let observed = f.get(&format!("/runs/{}/result", id(&seed.run)));
    assert_eq!(
        observed["publication"]["record"]["pr"]["state"],
        "confirmed"
    );
    assert_eq!(
        observed["publication"]["record"]["ready"], false,
        "Missing local checks/review cannot be supplied by hosted success"
    );
    mode.store(1, Ordering::SeqCst);
    click(cx, h, "reconcile-publication");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.observation.result["publication"]["record"]["observation"]["pipelines"][0]["steps"]
                [0]["state"]["result"]["name"]
                == "SKIPPED"
    });
    mode.store(2, Ordering::SeqCst);
    click(cx, h, "refresh-pr-feedback");
    wait(cx, h, &v, &e, |v, _| {
        !v.observation.feedback_pending
            && v.observation.result["publication"]["record"]["feedback"]["state"] == "unavailable"
    });
    let outage = f.get(&format!("/runs/{}/result", id(&seed.run)));
    assert_eq!(
        outage["publication"]["record"]["feedback"]["comments"][0]["content"]["raw"],
        "Explain the retained limitation."
    );
    assert_eq!(outage["publication"]["record"]["ready"], false);
    mode.store(3, Ordering::SeqCst);
    click(cx, h, "refresh-pr-feedback");
    wait(cx, h, &v, &e, |v, _| {
        !v.observation.feedback_pending
            && v.observation.result["publication"]["record"]["feedback"]["comments"][0]["content"]["raw"]
                == "Clarify the updated limitation."
    });
    f.restart();
    let (h2, v2, e2) = open(cx, &f);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.observation.result_fresh
            && v.observation.result["publication"]["record"]["feedback"]["comments"][0]["content"]["raw"]
                == "Clarify the updated limitation."
    });
    assert_eq!(
        f.get(&format!("/runs/{}/runtime", id(&seed.run)))["run"]["desiredState"],
        "paused"
    );
}
