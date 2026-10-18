//! Native controls against production HTTP/WS projections and durable private files.
//! Seeded attempts/check receipts are recorded fixtures, not executed agents or checks.
use super::test_support::{Fixture, click, navigate, open, wait};
use super::*;
use crewroom_core::{roles::Roles, store::Store};
use gpui::{AnyWindowHandle, test::TestWindowExt};
use gpui_kit as gpui;
fn id(v: &Value) -> String {
    string(v, "id")
}
#[gpui::test]
fn native_cancel_remains_available_while_resume_reply_is_pending(cx: &mut gpui::TestAppContext) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let fixture = Fixture::projected(|s, root| {
        let (t, r, _) = paused(s, root);
        task = t;
        run = r;
    });
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(cx, h, SharedString::from(format!("ticket-{}", id(&task))));
    wait(cx, h, &v, &e, |v, _| v.observation.runtime_fresh);
    navigate(cx, h, "Conversation");
    let resume = format!("/runs/{}/resume", id(&run));
    cx.update_window(h, |_, _, cx| {
        v.update(cx, |v, cx| {
            v.pending_commands.insert(resume.clone());
            v.busy = true;
            v.observation.runtime["busy"] = json!(true);
            cx.notify();
        });
    })
    .unwrap();
    click(cx, h, "cancel-run");
    click(cx, h, "ok");
    wait(cx, h, &v, &e, |v, _| {
        v.observation.runtime["run"]["desiredState"] == "cancelled"
            && !v
                .pending_commands
                .contains(&format!("/runs/{}/cancel", id(&run)))
    });
    cx.update_window(h, |_, _, cx| {
        v.update(cx, |v, cx| {
            assert!(v.busy, "The earlier Resume reply is still pending");
            v.accept(
                Event::Result {
                    path: resume,
                    result: Ok(json!({"status":"running"})),
                },
                cx,
            );
            assert!(!v.busy);
            assert_eq!(v.observation.runtime["run"]["desiredState"], "cancelled");
        });
    })
    .unwrap();
}
pub(super) fn paused(s: &Store, root: &std::path::Path) -> (Value, Value, Value) {
    let path = root.join("checkout");
    std::fs::create_dir(&path).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Seed",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let repository = s
        .create_repository(&json!({"name":"Lifecycle checkout","path":path,"targetBranch":"main"}))
        .unwrap();
    let task = s
        .create_task(&json!({"title":"Recorded lifecycle proof","description":"","repositoryId":repository["id"]}))
        .unwrap();
    let task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Retain evidence by revision","criteria":["Saved receipts stay attributable"]})).unwrap();
    super::test_support::accept_plan(s, &id(&task));
    let run = s
        .queue(&id(&task), &json!({"revision":task["revision"]}))
        .unwrap();
    let role = Roles::new(root.join("roles.json"))
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap();
    let admitted=s.admit_assignment(&id(&run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"requested":{"model":"fixture/first","effort":"low"},"resolved":{"selector":"fixture/first","effort":"low"}},"input":"Inspect recorded proof"})).unwrap();
    s.set_desired_state(&id(&run), "paused").unwrap();
    s.begin_pause(&id(&run), None).unwrap();
    s.finish_pause(&id(&run)).unwrap();
    (task, run, admitted)
}
fn select(
    cx: &mut gpui::TestAppContext,
    h: AnyWindowHandle,
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
fn text(
    cx: &mut gpui::TestAppContext,
    h: AnyWindowHandle,
    v: &Entity<Workbench>,
    path: &str,
    key: &str,
    text: &str,
) {
    cx.update_window(h, |_, window, cx| {
        let f = &v.read(cx).collaboration.forms[path].fields;
        if let Some(input) = f.inputs.get(key) {
            input.clone().update(cx, |i, cx| i.focus(window, cx));
        } else {
            f.areas[key].clone().update(cx, |i, cx| i.focus(window, cx));
        }
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.press("backspace", cx);
        window.input(text, cx);
    })
    .unwrap();
}
#[gpui::test]
fn native_budget_repair_cancel_and_restart_keep_durable_state_distinct(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut run = Value::Null;
    let mut assignment = Value::Null;
    let mut fixture = Fixture::projected(|s, root| {
        let (t, r, a) = paused(s, root);
        task = t;
        run = r;
        assignment = a["assignment"].clone();
        let mut attempt = a["attempt"].clone();
        for _ in 0..2 {
            s.record_ownership(&id(&attempt), true, Some("Recorded stopped attempt"))
                .unwrap();
            attempt = s
                .replace_attempt(&id(&assignment), &json!({"automatic":true}))
                .unwrap();
        }
        s.begin_pause(&id(&run), Some("budget")).unwrap();
        s.finish_pause(&id(&run)).unwrap();
    });
    let (h, v, e) = open(cx, &fixture);
    select(cx, h, &v, &e, &task);
    let budget = format!("/runs/{}/budget", id(&run));
    text(cx, h, &v, &budget, "minutes", "90");
    click(cx, h, "set-active-limit");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs")[0]["activeLimitMs"] == 5_400_000
    });
    assert_eq!(
        fixture.get(&format!("/runs/{}/runtime", id(&run)))["run"]["desiredState"],
        "paused"
    );
    let repair = format!(
        "/runs/{}/assignments/{}/repair-allowance",
        id(&run),
        id(&assignment)
    );
    text(
        cx,
        h,
        &v,
        &repair,
        "reason",
        "The recorded failure is understood; allow one retry.",
    );
    click(
        cx,
        h,
        SharedString::from(format!("allow-repair-{}", id(&assignment))),
    );
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && records(&v.observation.runtime, "assignments")[0]["repairAllowances"]
                .as_array()
                .is_some_and(|a| a.len() == 1)
    });
    click(cx, h, "cancel-run");
    click(cx, h, "cancel");
    assert_eq!(
        fixture.get(&format!("/runs/{}/runtime", id(&run)))["run"]["desiredState"],
        "paused"
    );
    click(cx, h, "cancel-run");
    click(cx, h, "ok");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs")[0]["desiredState"] == "cancelled"
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("resume-run").is_none());
    })
    .unwrap();
    fixture.restart();
    let saved = fixture.get(&format!("/runs/{}/runtime", id(&run)));
    assert_eq!(saved["run"]["desiredState"], "cancelled");
    assert_eq!(
        saved["assignments"][0]["repairAllowances"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(saved["attempts"].as_array().unwrap().len(), 3);
}
#[gpui::test]
fn native_history_reopens_prior_attempt_and_back_preserves_the_current_run(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut run = Value::Null;
    let mut prior = Value::Null;
    let fixture = Fixture::projected(|s, root| {
        let (t, r, a) = paused(s, root);
        task = t;
        run = r;
        prior = a["attempt"].clone();
        s.record_ownership(&id(&prior), true, Some("Recorded host interruption"))
            .unwrap();
        s.replace_attempt(&id(&a["assignment"]), &json!({"automatic":false}))
            .unwrap();
    });
    let (h, v, e) = open(cx, &fixture);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "History");
    click(
        cx,
        h,
        SharedString::from(format!("history-attempt-{}", id(&prior))),
    );
    wait(cx, h, &v, &e, |v, _| {
        v.observation.selected_attempt.as_deref() == Some(&id(&prior))
    });
    let history = fixture.get(&format!("/runs/{}/runtime", id(&run)))["history"].clone();
    assert_eq!(history["attempts"][0]["current"], false);
    assert_eq!(history["usage"]["rows"][0]["state"], "unavailable");
    assert!(history["usage"]["rows"][0]["tokens"].is_null());
    let (h2, v2, e2) = open(cx, &fixture);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.connected
            && v.observation.runtime_fresh
            && v.tab == "History"
            && v.observation.selected_attempt.as_deref() == Some(&id(&prior))
    });
    click(cx, h2, "back-to-current-run");
    wait(cx, h2, &v2, &e2, |v, _| {
        v.tab == "Conversation" && v.observation.selected_attempt.is_none()
    });
    assert_eq!(
        cx.read(|cx| v2.read(cx).observation.id.clone()),
        Some(id(&run))
    );
}
#[gpui::test]
fn native_evidence_hash_checks_history_selection_and_environment_cas(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut run = Value::Null;
    let mut output = Value::Null;
    let image = format!("sha256:{}", "a".repeat(64));
    let definition = json!({"id":"unit","command":"/bin/true","args":[],"image":image,"memoryMiB":1024,"required":true,"timeoutMs":1000});
    let fixture = Fixture::projected(|s, root| {
        let (t, r, a) = paused(s, root);
        task = t;
        run = r;
        let artifacts = crewroom_runtime::evidence::Artifacts::new(
            root.canonicalize().unwrap().join("data/artifacts"),
        )
        .unwrap();
        output = artifacts.write("Saved new check output ✓\n").unwrap();
        let old = artifacts.write("Historical output\n").unwrap();
        s.update_coding_record(&id(&run),|_|Ok(json!({"runId":run["id"],"requiredChecks":[definition],"candidates":[{"id":"old","revision":{"head":"old-head","base":"base","tree":"old-tree"}},{"id":"new","revision":{"head":"new-head","base":"base","tree":"new-tree"}}],"checks":[{"id":"old-check","candidateId":"old","checkId":"unit","definition":definition,"attemptId":a["attempt"]["id"],"started":true,"completed":true,"sourceIntact":true,"timedOut":false,"exitCode":0,"output":old,"source":old,"environment":{}},{"id":"new-check","candidateId":"new","checkId":"unit","definition":definition,"attemptId":a["attempt"]["id"],"started":true,"completed":false,"sourceIntact":true,"timedOut":false,"exitCode":null,"output":output,"source":output,"environment":{}}],"reviews":[],"checkPolicyChanges":[],"checkOperations":[]}))).unwrap();
    });
    let (h, v, e) = open(cx, &fixture);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Results");
    click(cx, h, "check-output-new-check");
    wait(cx, h, &v, &e, |v, _| {
        v.observation
            .detail
            .as_ref()
            .is_some_and(|d| matches!(&d.result, Some(Ok(_))))
    });
    assert_eq!(
        cx.read(|cx| v
            .read(cx)
            .observation
            .detail
            .as_ref()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()["content"]
            .clone()),
        "Saved new check output ✓\n"
    );
    click(cx, h, "candidate-old");
    click(cx, h, "check-output-old-check");
    wait(cx, h, &v, &e, |v, _| {
        v.observation
            .detail
            .as_ref()
            .is_some_and(|d| matches!(&d.result,Some(Ok(x)) if x["content"]=="Historical output\n"))
    });
    click(cx, h, "candidate-new");
    click(cx, h, "check-output-new-check");
    wait(cx, h, &v, &e, |v, _| {
        v.observation
            .detail
            .as_ref()
            .is_some_and(|d| d.result.is_some())
    });
    std::fs::write(
        fixture.root.path().join("data/artifacts").join(id(&output)),
        "tampered",
    )
    .unwrap();
    click(cx, h, "reload-evidence");
    wait(cx, h, &v, &e, |v, _| {
        v.observation
            .detail
            .as_ref()
            .is_some_and(|d| matches!(&d.result,Some(Err(x)) if x.contains("hash changed")))
    });
    click(cx, h, "close-evidence");
    let path = format!("/runs/{}/check-environment", id(&run));
    let key = format!("{path}/unit");
    text(cx, h, &v, &key, "memory", "2048");
    text(
        cx,
        h,
        &v,
        &key,
        "reason",
        "Increase memory for the existing check.",
    );
    let payload = json!({"requestId":uuid::Uuid::new_v4().to_string(),"checkId":"unit","expectedImage":image,"image":image,"expectedMemoryMiB":1024,"memoryMiB":3072,"reason":"Concurrent operator correction"});
    let reply = fixture.post(&path, payload);
    assert_eq!(reply.0, 200, "{}", reply.1);
    wait(cx, h, &v, &e, |v, _| {
        v.observation.result["requiredChecks"][0]["memoryMiB"] == 3072
    });
    click(cx, h, "correct-check-unit");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.error.contains("environment changed")
    });
    assert_eq!(
        cx.read(|cx| v.read(cx).collaboration.forms[&key]
            .fields
            .text("memory", cx)),
        "2048"
    );
    click(cx, h, "reload-check-unit");
    text(cx, h, &v, &key, "memory", "4096");
    text(
        cx,
        h,
        &v,
        &key,
        "reason",
        "Use the refreshed baseline for this correction.",
    );
    click(cx, h, "correct-check-unit");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.error.is_empty()
            && v.observation.result["requiredChecks"][0]["memoryMiB"] == 4096
    });
    let saved = fixture.get(&format!("/runs/{}/result", id(&run)));
    assert_eq!(saved["checkPolicyChanges"].as_array().unwrap().len(), 2);
    assert_eq!(saved["checks"].as_array().unwrap().len(), 2);
    assert_eq!(saved["requiredChecks"][0]["command"], "/bin/true");
    assert_eq!(saved["requiredChecks"][0]["required"], true);
}
