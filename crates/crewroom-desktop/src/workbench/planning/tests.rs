use super::super::test_support::{Fixture, click, open, screen_text, wait};
use super::*;
use gpui::test::TestWindowExt;
use gpui::{AnyWindowHandle, size};
use gpui_kit as gpui;
fn saved(f: &Fixture, path: &str, body: Value) -> Value {
    let (status, value) = f.post(path, body);
    assert_eq!(status, 200, "{path}: {value}");
    value
}
fn seed(f: &Fixture) -> (Value, Vec<Value>) {
    let repo = f.root.path().join("checkout");
    std::fs::create_dir(&repo).unwrap();
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
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let repo = saved(
        f,
        "/repositories",
        json!({"name":"Native plan checkout","path":repo,"targetBranch":"main"}),
    );
    let tasks: Vec<Value>=(0..3).map(|i|{let t=saved(f,"/tasks",json!({"title":format!("Plan {i}"),"description":"","repositoryId":repo["id"]}));saved(f,&format!("/tasks/{}/refinement",t["id"].as_str().unwrap()),json!({"revision":1,"sourceRevision":1,"brief":"Keep behavior","criteria":["Retain evidence"]}))}).collect();
    for task in &tasks {
        f.accept_plan(task["id"].as_str().unwrap());
    }
    (repo, tasks)
}
fn select(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, task: &Value) {
    click(
        cx,
        h,
        SharedString::from(format!("ticket-{}", string(task, "id"))),
    );
}
#[gpui::test]
fn native_launch_queue_and_dependencies_keep_intent_separate_from_execution(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (_repo, tasks) = seed(&fixture);
    saved(
        &fixture,
        "/roles",
        json!({"id":"disabled-checker","name":"Disabled checker","category":"Quality","purpose":"Check work","instructions":"Inspect evidence","model":"fixture/model","effort":"high","enabled":false}),
    );
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "tasks").len() == 3
    });
    select(cx, h, &tasks[0]);
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("launch-role-disabled-checker").is_none());
    })
    .unwrap();
    click(
        cx,
        h,
        SharedString::from(format!("add-dependency-{}", string(&tasks[1], "id"))),
    );
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "tasks")
            .iter()
            .any(|t| t["id"] == tasks[0]["id"] && t["dependsOn"] == json!([tasks[1]["id"]]))
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..100 {
            if window.find("queue-plan").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert!(
            window.find("queue-plan").focused() == Some(true),
            "Queue action must be keyboard reachable"
        );
        window.press("enter", cx);
    })
    .unwrap();
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "runs")
            .iter()
            .any(|r| r["taskId"] == tasks[0]["id"])
    });
    let first = cx.read(|cx| records(&v.read(cx).workspace, "runs")[0].clone());
    assert_eq!(first["launch"]["team"][0]["roleId"], "coordinator");
    assert!(first.get("position").is_some());
    click(cx, h, "launch-role-triage");
    click(cx, h, "queue-plan");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "runs")
                .iter()
                .any(|r| r["id"] == first["id"] && r["launch"]["team"][0]["roleId"] == "triage")
    });
    select(cx, h, &tasks[1]);
    click(
        cx,
        h,
        SharedString::from(format!("add-dependency-{}", string(&tasks[0], "id"))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && v.error.contains("cycle")
    });
    click(cx, h, "queue-plan");
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "runs").len() == 2 && !v.busy
    });
    let second = cx.read(|cx| {
        records(&v.read(cx).workspace, "runs")
            .into_iter()
            .find(|r| r["taskId"] == tasks[1]["id"])
            .unwrap()
    });
    click(cx, h, "queue");
    let old_revision = cx.read(|cx| v.read(cx).workspace["queue"]["revision"].clone());
    saved(
        &fixture,
        "/queue/order",
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"expectedRevision":old_revision,"runIds":[second["id"],first["id"]]}),
    );
    // Keep the displayed version old until the click. The real service must refuse it.
    click(
        cx,
        h,
        SharedString::from(format!("queue-down-{}", string(&first, "id"))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && v.error.contains("Queue changed")
    });
    cx.read(|cx| {
        assert_eq!(
            v.read(cx).workspace["queue"]["plans"][0]["id"],
            second["id"]
        )
    });
    click(
        cx,
        h,
        SharedString::from(format!("queue-down-{}", string(&second, "id"))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && v.workspace["queue"]["plans"][0]["id"] == first["id"]
    });
    // Cancelling a queued plan is terminal, so it confirms first and Keep sends nothing.
    let remove = SharedString::from(format!("remove-{}", string(&second, "id")));
    assert!(
        screen_text(cx, h)
            .iter()
            .any(|l| l.contains("Cancel queued plan…"))
    );
    click(cx, h, remove.clone());
    let shown = screen_text(cx, h);
    for expected in [
        "Cancel this queued plan?",
        "No agent has started.",
        "cannot resume",
    ] {
        assert!(
            shown.iter().any(|l| l.contains(expected)),
            "Missing {expected:?} in {shown:#?}"
        );
    }
    assert!(!shown.iter().any(|l| l.contains("Agents stop")));
    click(cx, h, "cancel");
    cx.run_until_parked();
    assert!(!cx.read(|cx| v.read(cx).busy));
    assert_eq!(
        records(&fixture.get("/workspace")["queue"], "plans").len(),
        2
    );
    click(cx, h, remove);
    click(cx, h, "ok");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.workspace["queue"], "plans").len() == 1
    });
    cx.read(|cx| {
        let w = &v.read(cx).workspace;
        assert!(records(w, "activity").is_empty());
        assert_eq!(w["capabilities"]["execution"], false);
        assert_eq!(
            records(w, "runs")
                .iter()
                .find(|r| r["id"] == second["id"])
                .unwrap()["status"],
            "cancelled"
        );
    });
    let mut reopened = cx.new_app();
    let (h2, v2, e2) = open(&mut reopened, &fixture);
    wait(&mut reopened, h2, &v2, &e2, |v, _| {
        v.connected && records(&v.workspace["queue"], "plans").len() == 1
    });
    click(&mut reopened, h2, "queue");
    click(
        &mut reopened,
        h2,
        SharedString::from(format!("open-plan-{}", string(&first, "id"))),
    );
    reopened.read(|cx| {
        assert_eq!(v2.read(cx).selected.as_deref(), tasks[0]["id"].as_str());
        assert_eq!(
            v2.read(cx).workspace["queue"]["plans"][0]["launch"]["team"][0]["roleId"],
            "triage"
        );
    });
    saved(
        &fixture,
        &format!("/tasks/{}/dependencies", string(&tasks[0], "id")),
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"dependencyRevision":1,"dependsOn":[tasks[2]["id"]]}),
    );
    click(
        &mut reopened,
        h2,
        SharedString::from(format!("remove-dependency-{}", string(&tasks[1], "id"))),
    );
    wait(&mut reopened, h2, &v2, &e2, |v, _| {
        !v.busy && v.error.contains("dependencies changed")
    });
    click(
        &mut reopened,
        h2,
        SharedString::from(format!("remove-dependency-{}", string(&tasks[2], "id"))),
    );
    wait(&mut reopened, h2, &v2, &e2, |v, _| {
        !v.busy
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == tasks[0]["id"] && t["dependsOn"] == json!([]))
    });
}
#[gpui::test]
fn native_plan_blocks_unsaved_brief_and_retired_target_at_narrow_width(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (repo, tasks) = seed(&fixture);
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "tasks").len() == 3
    });
    select(cx, h, &tasks[0]);
    cx.update_window(h, |_, window, cx| {
        let input = v.read(cx).drafts[&string(&tasks[0], "id")].brief.clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input(" unsaved", cx);
        window.render_frame(cx);
    })
    .unwrap();
    click(cx, h, "plan-manage-agent-roles");
    click(cx, h, "roles-back-to-ticket");
    click(cx, h, "queue-plan");
    assert!(cx.read(|cx| !v.read(cx).busy));
    cx.read(|cx| assert!(records(&v.read(cx).workspace, "runs").is_empty()));
    click(cx, h, "save-brief");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == tasks[0]["id"] && t["revision"] == 3)
    });
    saved(
        &fixture,
        &format!("/repositories/{}/retire", string(&repo, "id")),
        json!({"revision":1,"requestId":uuid::Uuid::new_v4().to_string()}),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.workspace["repositories"][0]["retiredAt"].is_null()
    });
    click(cx, h, "queue-plan");
    assert!(cx.read(|cx| !v.read(cx).busy));
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        for id in [
            "queue-plan",
            "launch-role-coordinator",
            "launch-role-reviewer",
        ] {
            let b = window.find(id).bounds();
            assert!(b.origin.x >= px(0.) && b.right() <= px(390.), "{id}: {b:?}");
        }
    })
    .unwrap();
    let other_path = fixture.root.path().join("other-checkout");
    assert!(
        std::process::Command::new("git")
            .args(["clone", "--local"])
            .arg(fixture.root.path().join("checkout"))
            .arg(&other_path)
            .output()
            .unwrap()
            .status
            .success()
    );
    let other = saved(
        &fixture,
        "/repositories",
        json!({"name":"Replacement target","path":other_path,"targetBranch":"main"}),
    );
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "repositories").len() == 2
    });
    click(
        cx,
        h,
        SharedString::from(format!("assign-{}", string(&other, "id"))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == tasks[0]["id"] && t["repositoryId"] == other["id"])
    });
    click(cx, h, "open-navigation");
    click(cx, h, "queue");
    cx.read(|cx| assert!(records(&v.read(cx).workspace["queue"], "plans").is_empty()));
}

#[gpui::test]
fn queue_playbook_copy_and_header_change_remain_separate_from_readiness(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (_, tasks) = seed(&fixture);
    saved(
        &fixture,
        "/playbooks",
        json!({"id":"delivery","revision":0,"name":"Delivery","description":"Bounded changes","steps":[{"id":"inspect","title":"Inspect evidence"},{"id":"review","title":"Independent review"}]}),
    );
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && records(&v.workspace, "playbooks").len() == 1
    });
    select(cx, h, &tasks[0]);
    click(cx, h, "queue-playbook-delivery");
    click(cx, h, "queue-plan");
    wait(cx, h, &v, &e, |v, _| {
        !records(&v.workspace, "runs").is_empty()
    });
    let run = fixture.get("/workspace")["runs"][0].clone();
    assert_eq!(run["playbook"]["name"], "Delivery");
    assert_eq!(run["status"], "awaiting_runner");
    click(cx, h, "change-run-playbook");
    let path = format!("/runs/{}/playbook", string(&run, "id"));
    cx.update_window(h, |_, w, cx| {
        let i = v.read(cx).collaboration.forms[&path].fields.areas["reason"].clone();
        i.update(cx, |i, cx| i.focus(w, cx));
        w.input("Let the coordinator propose a different checklist.", cx);
    })
    .unwrap();
    click(cx, h, "select-run-playbook-");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs")[0]["playbookVersion"] == 2
    });
    assert!(fixture.get("/workspace")["runs"][0]["playbook"].is_null());
    click(cx, h, "hide-details");
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        let b = w.find("change-run-playbook").bounds();
        assert!(b.origin.x >= px(0.) && b.right() <= px(390.));
        assert!(gpui::base::test_support::snapshots(w).iter().any(|n| {
            n.label()
                .is_some_and(|s| s.contains("readiness still requires revision-specific evidence"))
        }));
    })
    .unwrap();
}
