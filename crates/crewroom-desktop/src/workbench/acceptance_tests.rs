//! Rendered native labels and keyboard interactions over durable, explicitly
//! recorded fixtures. No provider delivery or process execution is claimed.
use super::test_support::{Fixture, click, navigate, navigate_by_keyboard, open, wait};
use super::*;
use crewroom_core::{roles::Roles, store::Store};
use gpui_kit::{self as gpui, AnyWindowHandle, Focusable as _, test::TestWindowExt};
use std::path::Path;
fn id(v: &Value) -> String {
    string(v, "id")
}
pub(super) fn labels(
    cx: &mut gpui::TestAppContext,
    h: AnyWindowHandle,
    scope: Option<&str>,
) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter(|node| {
                scope.is_none_or(|scope| {
                    node.path()
                        .contains(&gpui::ElementId::from(SharedString::from(scope.to_owned())))
                })
            })
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}
pub(super) fn contains(labels: &[String], expected: &str) {
    assert!(
        labels.iter().any(|l| l.contains(expected)),
        "Missing {expected:?} in {labels:#?}"
    );
}
pub(super) fn select(
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
    wait(cx, h, v, e, |v, _| v.observation.runtime_fresh);
}
fn recorded(s: &Store, root: &Path) -> (Value, Value, Value) {
    let task = s
        .create_task(&json!({"title":"Native acceptance records","description":""}))
        .unwrap();
    let run = s
        .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
        .unwrap();
    let role = Roles::new(root.join("roles.json"))
        .read()
        .unwrap()
        .remove(0);
    let model = json!({"requested":{"model":"fixture/first","effort":"low"},"resolved":{"provider":"fixture","id":"first","selector":"fixture/first","effort":"low"}});
    let a=s.admit_assignment(&id(&run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":model,"input":"Recorded acceptance fixture"})).unwrap();
    s.claim_attempt(&id(&a["attempt"])).unwrap();
    // Impossible local PIDs; ownership is stopped before service startup.
    s.record_attempt_identity(&id(&a["attempt"]),&json!({"hostPid":2_000_000_011u32,"ompPid":2_000_000_012u32,"sessionId":"record-only","hostUrl":"http://127.0.0.1:9","pane":"%999"})).unwrap();
    (task, run, a)
}
fn pause(s: &Store, run: &Value, a: &Value) {
    s.set_desired_state(&id(run), "paused").unwrap();
    s.begin_pause(&id(run), None).unwrap();
    s.record_ownership(
        &id(&a["attempt"]),
        true,
        Some("Recorded ownership stopped; no agent ran"),
    )
    .unwrap();
    s.finish_pause(&id(run)).unwrap();
}
#[gpui::test]
fn native_acceptance_attempt_models_and_fallback_survive_service_and_ui_reopen(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run, mut old, mut next) =
        (Value::Null, Value::Null, Value::Null, Value::Null);
    let mut f = Fixture::projected(|s, root| {
        let (t, r, a) = recorded(s, root);
        task = t;
        run = r;
        old = a["attempt"].clone();
        s.record_agent_event(&id(&old),1,&json!({"type":"prompt_result","status":"error","error":{"provider":"fixture","model":"first","retryable":true,"message":"Recorded provider interruption"}})).unwrap();
        pause(s, &run, &a);
        next=s.replace_attempt(&id(&a["assignment"]),&json!({"automatic":true,"fallback":{"eventSequence":1,"model":{"requested":old["model"]["requested"],"resolved":{"provider":"fixture","id":"alternate","selector":"fixture/alternate","effort":"high"},"fallbackFrom":"fixture/first"}}})).unwrap();
        s.record_ownership(
            &id(&next),
            true,
            Some("Recorded replacement; no process started"),
        )
        .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "History");
    click(
        cx,
        h,
        SharedString::from(format!("history-attempt-{}", id(&old))),
    );
    let prior = labels(cx, h, None);
    contains(&prior, "Model: fixture/first · low");
    assert!(!prior.iter().any(|l| l.contains("Model: fixture/alternate")));
    click(
        cx,
        h,
        SharedString::from(format!("history-attempt-{}", id(&next))),
    );
    let current = labels(cx, h, None);
    contains(&current, "Model: fixture/alternate · high");
    contains(&current, "Requested fixture/first · low");
    contains(&current, "Fallback from fixture/first");
    f.restart();
    let (h2, v2, e2) = open(cx, &f);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.connected
            && v.observation.runtime_fresh
            && v.observation.selected_attempt.as_deref() == Some(&id(&next))
    });
    contains(&labels(cx, h2, None), "Model: fixture/alternate · high");
    click(
        cx,
        h2,
        SharedString::from(format!("history-attempt-{}", id(&old))),
    );
    contains(&labels(cx, h2, None), "Model: fixture/first · low");
    let saved = f.get(&format!("/runs/{}/runtime", id(&run)));
    assert_eq!(saved["attempts"][0]["model"], old["model"]);
    assert_eq!(saved["attempts"][1]["model"], next["model"]);
}
#[gpui::test]
fn native_acceptance_observation_error_does_not_replace_recorded_failure(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run, mut attempt) = (Value::Null, Value::Null, Value::Null);
    let f = Fixture::projected(|s, root| {
        let (t, r, a) = recorded(s, root);
        task = t;
        run = r;
        attempt = a["attempt"].clone();
        s.record_agent_event(&id(&attempt),1,&json!({"type":"prompt_result","status":"error","error":{"message":"Recorded provider interruption"}})).unwrap();
        pause(s, &run, &a);
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "History");
    click(
        cx,
        h,
        SharedString::from(format!("history-attempt-{}", id(&attempt))),
    );
    let before = f.get(&format!("/runs/{}/runtime", id(&run)))["attempts"].clone();
    cx.update_window(h,|_,window,cx| {v.update(cx,|v,cx| {let generation=v.observation.generation;v.accept(Event::Live { generation, message: json!({"topic":format!("run:{}",id(&run)),"kind":"error","error":"Fixture observation timeout"}) },cx);});window.render_frame(cx);}).unwrap();
    let view = labels(cx, h, None);
    contains(&view, "Fixture observation timeout");
    contains(&view, "does not establish an agent failure");
    contains(&view, "Recorded provider interruption");
    contains(&view, "recorded history, not a new process observation");
    assert_eq!(
        f.get(&format!("/runs/{}/runtime", id(&run)))["attempts"],
        before
    );
}
#[gpui::test]
fn native_acceptance_steering_states_times_legacy_and_narrow_keyboard(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let mut messages = vec![];
    let mut f = Fixture::projected(|s, root| {
        let (t, r, a) = recorded(s, root);
        task = t;
        run = r;
        for (index, body) in [
            "Stored direction",
            "Unconfirmed direction",
            "Received direction",
            "Applied direction",
            "Rejected direction",
            "Legacy received direction",
        ]
        .into_iter()
        .enumerate()
        {
            let m = s
                .steer(
                    &id(&run),
                    &json!({"requestId":format!("state-{index}"),"body":body}),
                )
                .unwrap();
            messages.push(m.clone());
            if index == 0 {
                continue;
            }
            let input = format!("input-{index}");
            s.dispatch_steering(&id(&a["attempt"]), &[id(&m)], &input)
                .unwrap();
            if index == 1 {
                continue;
            }
            s.receive_steering(&id(&a["attempt"]), &input).unwrap();
            if index == 3 || index == 4 {
                s.apply_steering(&id(&a["attempt"]),&format!("receipt-{index}"),&json!({"messageId":m["id"],"disposition":if index==3{"applied"}else{"rejected"},"reason":if index==3{"Added a check to the plan."}else{"Outside the accepted scope."}})).unwrap();
            }
        }
        s.record_agent_event(&id(&a["attempt"]),1,&json!({"type":"message_end","message":{"role":"assistant","timestamp":1700000000000i64,"content":[{"type":"text","text":"I claim everything is finished."}]}})).unwrap();
        pause(s, &run, &a);
        let db = rusqlite::Connection::open(root.join("data/workflow.sqlite")).unwrap();
        let mut legacy = s.steering(&id(&run)).unwrap().pop().unwrap();
        legacy.as_object_mut().unwrap().remove("transitions");
        db.execute(
            "UPDATE steering SET data=? WHERE id=?",
            rusqlite::params![legacy.to_string(), id(&legacy)],
        )
        .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Conversation");
    wait(cx, h, &v, &e, |v, _| !v.observation.events.is_empty());
    for (index, label) in [
        "Stored · Receipt unconfirmed",
        "Dispatched · Receipt unconfirmed",
        "Receipt confirmed",
        "Applied to plan",
        "Received · Coordinator declined",
        "Receipt confirmed",
    ]
    .into_iter()
    .enumerate()
    {
        let scope = format!("operator-message-{}", id(&messages[index]));
        let view = labels(cx, h, Some(&scope));
        contains(&view, label);
        contains(&view, "Stored: ");
        let timed = view.iter().filter(|l| l.contains("UTC")).count();
        assert_eq!(timed, if index == 5 { 1 } else { (index + 1).min(4) });
        if index == 5 {
            assert_eq!(
                view.iter().filter(|l| l.contains("Time unknown")).count(),
                2
            );
        }
        if index == 3 {
            contains(&view, "This does not confirm completed work");
        }
    }
    // One section-level note replaces a disposition caveat repeated on every reply.
    let view = labels(cx, h, None);
    assert_eq!(
        view.iter()
            .filter(|l| l.contains(
                "A reply does not mean your steering was applied; check each message's status."
            ))
            .count(),
        1
    );
    assert!(
        !view
            .iter()
            .any(|l| l.contains("requires a recorded disposition"))
    );
    assert!(view.iter().any(|l| l == "Received · Coordinator declined"));
    assert!(!view.iter().any(|l| l == "Rejected"));
    contains(&view, "Saved now. Delivery waits for Resume.");
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).steering[&id(&run)].clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input("Keyboard steering at 390", cx);
        window.press("cmd-enter", cx);
    })
    .unwrap();
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "messages").len() == 7
    });
    f.restart();
    let (h2, v2, e2) = open(cx, &f);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.connected && v.observation.runtime_fresh && records(&v.workspace, "messages").len() == 7
    });
    contains(&labels(cx, h2, None), "Keyboard steering at 390");
    navigate(cx, h2, "Brief");
    click(cx, h2, "close-refinement");
    click(cx, h2, "ok");
    wait(cx, h2, &v2, &e2, |v, _| {
        !v.busy && records(&v.workspace, "runs")[0]["status"] == "closed"
    });
    navigate(cx, h2, "Conversation");
    contains(&labels(cx, h2, None), "New steering is unavailable");
    cx.update_window(h2, |_, window, cx| {
        let input = v2.read(cx).steering[&id(&run)].clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input("Must not be delivered", cx);
        window.press("cmd-enter", cx);
    })
    .unwrap();
    click(cx, h2, "save-steering");
    assert!(!cx.read(|cx| v2.read(cx).busy));
    assert_eq!(f.get("/workspace")["messages"].as_array().unwrap().len(), 7);
}

// Prepared outside source while the integrated gate is frozen.
#[gpui::test]
fn native_acceptance_closed_and_completed_keep_legacy_times_unknown_after_reopen(
    cx: &mut gpui::TestAppContext,
) {
    for state in ["closed", "completed"] {
        let (mut task, mut run, mut message) = (Value::Null, Value::Null, Value::Null);
        let mut f = Fixture::projected(|s, root| {
            let (t, r, a) = recorded(s, root);
            task = t;
            run = r;
            message = s
                .steer(
                    &id(&run),
                    &json!({"requestId":"legacy","body":"Legacy received direction"}),
                )
                .unwrap();
            s.dispatch_steering(&id(&a["attempt"]), &[id(&message)], "legacy-input")
                .unwrap();
            s.receive_steering(&id(&a["attempt"]), "legacy-input")
                .unwrap();
            pause(s, &run, &a);
            let db = rusqlite::Connection::open(root.join("data/workflow.sqlite")).unwrap();
            let mut saved = s.steering(&id(&run)).unwrap().remove(0);
            saved.as_object_mut().unwrap().remove("transitions");
            db.execute(
                "UPDATE steering SET data=? WHERE id=?",
                rusqlite::params![saved.to_string(), id(&saved)],
            )
            .unwrap();
            let mut saved = s.get_run(&id(&run)).unwrap();
            saved["status"] = json!(state);
            saved["desiredState"] = json!("cancelled");
            if state == "completed" {
                saved["purpose"] = json!("research")
            };
            db.execute(
                "UPDATE runs SET data=? WHERE id=?",
                rusqlite::params![saved.to_string(), id(&saved)],
            )
            .unwrap();
        });
        for pass in 0..2 {
            if pass == 1 {
                f.restart();
            }
            let (h, v, e) = open(cx, &f);
            select(cx, h, &v, &e, &task);
            navigate(cx, h, "Conversation");
            let scope = format!("operator-message-{}", id(&message));
            let shown = labels(cx, h, Some(&scope));
            contains(&shown, "Receipt confirmed");
            assert_eq!(
                shown.iter().filter(|l| l.contains("Time unknown")).count(),
                2
            );
            contains(&labels(cx, h, None), "New steering is unavailable");
            assert_eq!(
                f.post(
                    &format!("/runs/{}/steering", id(&run)),
                    json!({"requestId":format!("late-{pass}"),"body":"No new work"})
                )
                .0,
                409
            );
        }
    }
}
pub(super) fn keyboard_activate(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..100 {
            window.press("tab", cx);
            window.render_frame(cx);
            window.render_frame(cx);
            if window.find(SharedString::from(id.to_owned())).focused() == Some(true) {
                let bounds = window.find(SharedString::from(id.to_owned())).bounds();
                assert!(
                    bounds.origin.x >= px(0.) && bounds.right() <= window.viewport_size().width,
                    "{id} overflows narrow window: {bounds:?}"
                );
                assert!(
                    bounds.origin.y >= px(0.) && bounds.bottom() <= window.viewport_size().height,
                    "{id} stays outside the visible viewport after Tab: {bounds:?}"
                );
                // Space activates the focused native button. Return is the alert
                // dialog default action even when its Cancel button has focus.
                window.press("space", cx);
                window.render_frame(cx);
                return;
            }
        }
        panic!("Keyboard could not reach {id}");
    })
    .unwrap();
}
pub(super) fn coding_plan(s: &Store, root: &Path) -> (Value, Value, Value) {
    let checkout = root.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
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
            "Fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let repo=s.create_repository(&json!({"name":"Original captured repository","path":checkout,"targetBranch":"main","branchPrefix":"original/"})).unwrap();
    let task = s
        .create_task(
            &json!({"title":"Retained repository plan","description":"","repositoryId":repo["id"]}),
        )
        .unwrap();
    let task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Inspect captured configuration","criteria":["Preserve original evidence"]})).unwrap();
    super::test_support::accept_plan(s, &id(&task));
    let run = s
        .queue(&id(&task), &json!({"revision":task["revision"]}))
        .unwrap();
    (task, run, repo)
}
#[gpui::test]
fn native_acceptance_unowned_cancel_is_confirmed_by_keyboard_at_390_and_only_new_branch_queues(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let mut f = Fixture::projected(|s, root| {
        let (t, r, _) = coding_plan(s, root);
        task = t;
        run = r;
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    // Run controls belong to the session column; a narrow window shows it instead of the drawer.
    navigate_by_keyboard(cx, h, "Conversation");
    keyboard_activate(cx, h, "cancel-run");
    std::thread::sleep(std::time::Duration::from_millis(300));
    // The plan is still queued: no agent exists to stop.
    let shown = labels(cx, h, None);
    contains(&shown, "Cancel this queued plan?");
    contains(&shown, "No agent has started.");
    contains(&shown, "Keep plan");
    assert!(!shown.iter().any(|l| l.contains("Agents stop")));
    keyboard_activate(cx, h, "cancel");
    assert_ne!(f.get("/workspace")["runs"][0]["status"], "cancelled");
    keyboard_activate(cx, h, "cancel-run");
    std::thread::sleep(std::time::Duration::from_millis(300));
    keyboard_activate(cx, h, "ok");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs")[0]["status"] == "cancelled"
    });
    contains(&labels(cx, h, None), "No owned branch is available");
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("continue-later").is_none());
        assert!(window.try_find("resume-run").is_none());
    })
    .unwrap();
    navigate(cx, h, "Brief");
    keyboard_activate(cx, h, "queue-plan");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs").len() == 2
    });
    let saved = f.get("/workspace");
    let next = records(&saved, "runs")
        .into_iter()
        .find(|r| r["id"] != run["id"])
        .unwrap();
    assert_eq!(next["launch"]["repository"]["mode"], "new");
    assert!(next["continuedFrom"].is_null());
    f.restart();
    assert_eq!(f.get("/workspace")["runs"].as_array().unwrap().len(), 2);
    assert!(
        f.get(&format!("/runs/{}/runtime", id(&next)))["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[gpui::test]
#[ignore = "Real prepared Git worktree needs workspace binaries; run the native journey gate."]
fn native_acceptance_retired_repository_keeps_captured_settings_in_results_and_history(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run, mut captured) = (Value::Null, Value::Null, Value::Null);
    let mut f = Fixture::projected(|s, root| {
        let (t, r, repo) = coding_plan(s, root);
        task = t;
        run = r;
        let root = root.canonicalize().unwrap();
        let helper = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("crewroom-process")
            .canonicalize()
            .unwrap();
        let authority = std::sync::Arc::new(
            crewroom_core::authority::Authority::acquire_for_execution(&root.join("data")).unwrap(),
        );
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            Store::open(&root.join("data/workflow.sqlite")).unwrap(),
        ));
        let repository = crewroom_runtime::repository::Repository::new(
            store,
            root.join("data/worktrees"),
            helper,
            authority,
        )
        .unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let work = rt
            .block_on(repository.prepare(&id(&run), &json!({"mode":"new"})))
            .unwrap();
        captured = work["repository"].clone();
        assert_eq!(captured["name"], repo["name"]);
        assert_eq!(captured["branchPrefix"], repo["branchPrefix"]);
        rt.block_on(async { repository.close().unwrap().wait().await.unwrap() });
        s.set_desired_state(&id(&run), "cancelled").unwrap();
        s.begin_pause(&id(&run), None).unwrap();
        s.finish_pause(&id(&run)).unwrap();
        let update = json!({"revision":repo["revision"],"requestId":uuid::Uuid::new_v4().to_string(),"name":"Current renamed repository","path":repo["path"],"targetBranch":"main","branchPrefix":"changed/","checks":[]});
        let checked = crewroom_core::repositories::validate_update(&update).unwrap();
        let updated = s.update_repository(&id(&repo), &checked, &update).unwrap();
        s.retire_repository(
            &id(&repo),
            &json!({"revision":updated["revision"],"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap();
    });
    for pass in 0..2 {
        if pass == 1 {
            f.restart()
        }
        let (h, v, e) = open(cx, &f);
        select(cx, h, &v, &e, &task);
        wait(cx, h, &v, &e, |v, _| v.observation.result_fresh);
        navigate(cx, h, "Results");
        let rendered = labels(cx, h, None);
        contains(&rendered, "Original captured repository");
        contains(&rendered, "Target: main · prefix original/");
        contains(&rendered, "This repository is now retired");
        contains(&rendered, "Current repository settings differ");
        navigate(cx, h, "History");
        let rendered = labels(cx, h, None);
        contains(&rendered, "Original captured repository");
        contains(&rendered, "Target main · branch prefix original/");
        assert_eq!(
            f.get(&format!("/runs/{}/result", id(&run)))["repository"],
            captured
        );
    }
}
#[gpui::test]
fn native_acceptance_history_tabs_and_back_work_by_keyboard_at_390(cx: &mut gpui::TestAppContext) {
    let (mut task, mut run, mut prior) = (Value::Null, Value::Null, Value::Null);
    let f = Fixture::projected(|s, root| {
        let (t, r, a) = recorded(s, root);
        task = t;
        run = r;
        prior = a["attempt"].clone();
        pause(s, &run, &a);
        let next = s
            .replace_attempt(&id(&a["assignment"]), &json!({"automatic":false}))
            .unwrap();
        s.record_ownership(&id(&next), true, Some("Recorded replacement"))
            .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    navigate_by_keyboard(cx, h, "History");
    keyboard_activate(cx, h, &format!("history-attempt-{}", id(&prior)));
    contains(&labels(cx, h, None), "Read-only prior attempt");
    keyboard_activate(cx, h, "back-to-current-run");
    assert_eq!(cx.read(|cx| v.read(cx).tab), "Conversation");
    navigate_by_keyboard(cx, h, "Results");
    assert_eq!(cx.read(|cx| v.read(cx).tab), "Results");
    navigate_by_keyboard(cx, h, "Brief");
    assert_eq!(cx.read(|cx| v.read(cx).tab), "Brief");
    assert_eq!(
        f.get(&format!("/runs/{}/runtime", id(&run)))["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[gpui::test]
fn native_acceptance_queued_specialist_keyboard_at_390_keeps_pending_after_restart(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let mut f = Fixture::seeded(true, |s, root| {
        let (t, r, _) = coding_plan(s, root);
        task = t;
        run = r;
        let mut roles = Roles::new(root.join("roles.json")).read().unwrap();
        for role in &mut roles {
            role["model"] = json!("fixture/model");
            role["effort"] = json!("low");
        }
        std::fs::write(root.join("roles.json"), serde_json::to_vec(&roles).unwrap()).unwrap();
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    navigate_by_keyboard(cx, h, "Conversation");
    navigate_by_keyboard(cx, h, "specialist-picker");
    keyboard_activate(cx, h, "specialist-role-tech-writer");
    let path = format!("/runs/{}/specialists", id(&run));
    cx.update_window(h, |_, window, cx| {
        let input = v.read(cx).collaboration.forms[&path].fields.areas["input"].clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input("Explain the result through the coordinator.", cx);
    })
    .unwrap();
    keyboard_activate(cx, h, "propose-specialist");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.observation.runtime, "teamProposals").len() == 1
    });
    contains(&labels(cx, h, None), "Proposed · not started");
    f.restart();
    let (h2, v2, e2) = open(cx, &f);
    wait(cx, h2, &v2, &e2, |v, _| {
        v.connected
            && v.observation.runtime_fresh
            && records(&v.observation.runtime, "teamProposals").len() == 1
    });
    contains(&labels(cx, h2, None), "Proposed · not started");
    let saved = f.get(&format!("/runs/{}/runtime", id(&run)));
    assert_eq!(
        saved["teamProposals"][0]["specialists"][0]["role"]["id"],
        "tech-writer"
    );
    assert!(saved["assignments"].as_array().unwrap().is_empty());
    assert!(saved["attempts"].as_array().unwrap().is_empty());
}

#[gpui::test]
fn native_acceptance_tab_reveals_nested_controls_without_moving_siblings_or_pinning_scroll(
    cx: &mut gpui::TestAppContext,
) {
    struct Scrolls {
        outer: gpui::ScrollHandle,
        inner: gpui::ScrollHandle,
        sibling: gpui::ScrollHandle,
    }
    impl Render for Scrolls {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("outer-scroll")
                .w(px(350.))
                .h(px(180.))
                .overflow_y_scroll()
                .track_scroll(&self.outer)
                .flex()
                .flex_col()
                .child(Button::new("top").label("Top").flex_shrink_0())
                .child(div().h(px(300.)).flex_shrink_0())
                .child(
                    div()
                        .flex()
                        .h(px(120.))
                        .flex_shrink_0()
                        .child(
                            div()
                                .id("inner-scroll")
                                .w(px(160.))
                                .h_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.inner)
                                .flex()
                                .flex_col()
                                .child(Button::new("inner-first").label("First").flex_shrink_0())
                                .child(div().h(px(400.)).flex_shrink_0())
                                .child(Button::new("inner-last").label("Last").flex_shrink_0()),
                        )
                        .child(
                            div()
                                .id("sibling-scroll")
                                .w(px(160.))
                                .h_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.sibling)
                                .child(div().h(px(700.))),
                        ),
                )
        }
    }
    let (h, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(Default::default(), cx, |_, cx| {
            cx.new(|_| Scrolls {
                outer: Default::default(),
                inner: Default::default(),
                sibling: Default::default(),
            })
        })
        .unwrap()
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let outer = view.read(cx).outer.clone();
        let inner = view.read(cx).inner.clone();
        let sibling = view.read(cx).sibling.clone();
        sibling.set_offset(gpui::point(px(0.), px(-200.)));
        window.render_frame(cx);
        for name in ["top", "inner-first", "inner-last"] {
            if name == "top" {
                window.focus_next(cx);
            } else {
                window.press("tab", cx);
            }
            window.render_frame(cx);
            window.render_frame(cx);
            let shown = window.find(name);
            assert_eq!(shown.focused(), Some(true), "Expected focused {name}");
            assert!(
                shown.bounds().top() >= outer.bounds().top()
                    && shown.bounds().bottom() <= outer.bounds().bottom(),
                "{name}: {:?} outside {:?}",
                shown.bounds(),
                outer.bounds()
            );
            if name != "top" {
                assert!(
                    shown.bounds().top() >= inner.bounds().top()
                        && shown.bounds().bottom() <= inner.bounds().bottom(),
                    "{name}: {:?} outside inner {:?}",
                    shown.bounds(),
                    inner.bounds()
                );
            }
            assert_eq!(sibling.offset().y, px(-200.));
        }
        // A subsequent user scroll is not undone just because Last still has focus.
        inner.set_offset(gpui::point(px(0.), px(0.)));
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(inner.offset().y, px(0.));
        for name in ["inner-first", "top"] {
            window.press("shift-tab", cx);
            window.render_frame(cx);
            window.render_frame(cx);
            let shown = window.find(name);
            assert_eq!(shown.focused(), Some(true), "Expected focused {name}");
            assert!(
                shown.bounds().top() >= outer.bounds().top()
                    && shown.bounds().bottom() <= outer.bounds().bottom(),
                "{name}: {:?} outside {:?}",
                shown.bounds(),
                outer.bounds()
            );
        }
        assert_eq!(sibling.offset().y, px(-200.));
    })
    .unwrap();
}
#[gpui::test]
fn native_session_shell_groups_tickets_keeps_four_facts_separate_and_drawer_follows_width(
    cx: &mut gpui::TestAppContext,
) {
    let (mut queued, mut active, mut refining) = (Value::Null, Value::Null, Value::Null);
    let f = Fixture::projected(|s, root| {
        let (t, _, repo) = coding_plan(s, root);
        queued = t;
        let task = s
            .create_task(
                &json!({"title":"Active session","description":"","repositoryId":repo["id"]}),
            )
            .unwrap();
        let task = s.refine(&id(&task), &json!({"revision":1,"sourceRevision":1,"brief":"Hold the slot","criteria":["Facts stay separate"]})).unwrap();
        super::test_support::accept_plan(s, &id(&task));
        let run = s
            .queue(&id(&task), &json!({"revision":task["revision"]}))
            .unwrap();
        s.claim_coding_slot(&id(&run), true).unwrap();
        let role = Roles::new(root.join("roles.json"))
            .read()
            .unwrap()
            .into_iter()
            .find(|r| r["id"] == "coordinator")
            .unwrap();
        s.admit_assignment(&id(&run), &json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"requested":{"model":"fixture/model","effort":"low"},"resolved":{"selector":"fixture/model","effort":"low"}},"input":"Recorded coordinator"})).unwrap();
        s.set_desired_state(&id(&run), "paused").unwrap();
        s.begin_pause(&id(&run), None).unwrap();
        s.finish_pause(&id(&run)).unwrap();
        active = task;
        refining = s
            .create_task(&json!({"title":"Still refining","description":""}))
            .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &active);

    // Active runs, the local queue and other tickets are separate groups, in that order.
    let side = labels(cx, h, Some("ticket-list"));
    let top = |cx: &mut gpui::TestAppContext, id: String| {
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.find(SharedString::from(id)).bounds().origin.y
        })
        .unwrap()
    };
    let order = [
        "group-active-runs".to_owned(),
        format!("row-key-{}", id(&active)),
        "group-queue".to_owned(),
        format!("row-key-{}", id(&queued)),
        "group-other-tickets".to_owned(),
        format!("row-key-{}", id(&refining)),
    ];
    let tops: Vec<_> = order.iter().map(|i| top(cx, i.clone())).collect();
    assert!(
        tops.windows(2).all(|w| w[0] < w[1]),
        "Sidebar order {order:?} at {tops:?}"
    );
    // Each row keeps Jira status, workflow and agent activity as separate chips.
    let mut row =
        |task: &Value, kind: &str| labels(cx, h, Some(&format!("row-{kind}-{}", id(task))));
    assert_eq!(row(&active, "jira"), ["Not linked"]);
    assert_eq!(row(&active, "workflow"), ["Paused"]);
    assert_eq!(row(&active, "activity"), ["1 not started"]);
    assert_eq!(row(&queued, "workflow"), ["Queued"]);
    assert_eq!(row(&queued, "activity"), ["No agent processes"]);
    assert_eq!(row(&refining, "workflow"), ["Refining"]);
    contains(&side, "#1 in queue");

    // The header shows four facts; admitted is never reported as executing or ready.
    assert_eq!(labels(cx, h, Some("fact-jira")), ["Not linked"]);
    assert_eq!(labels(cx, h, Some("fact-workflow")), ["Paused"]);
    assert_eq!(labels(cx, h, Some("fact-activity")), ["1 not started"]);
    let readiness = labels(cx, h, Some("fact-readiness"));
    assert!(
        readiness == ["Not ready"] || readiness == ["Evidence not current"],
        "{readiness:?}"
    );

    // Wide: the session and the details drawer are visible together.
    let visible = |cx: &mut gpui::TestAppContext, id: &str| {
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(SharedString::from(id.to_owned())).is_some()
        })
        .unwrap()
    };
    assert!(visible(cx, "hide-details") && visible(cx, "fact-workflow"));
    // Narrow: one at a time, and the switch is keyboard reachable without overflow.
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    assert!(visible(cx, "hide-details") && !visible(cx, "fact-workflow") && visible(cx, "Brief"));
    keyboard_activate(cx, h, "hide-details");
    assert!(visible(cx, "fact-workflow") && !visible(cx, "Brief"));
    keyboard_activate(cx, h, "toggle-details");
    assert!(visible(cx, "Brief") && !visible(cx, "fact-workflow"));

    // Settings sit behind the gear with one sub-navigation.
    cx.simulate_window_resize(h, gpui::size(px(1280.), px(1000.)));
    click(cx, h, "settings");
    for id in ["roles", "repositories", "learning", "import-jira"] {
        assert!(visible(cx, id), "{id} missing from settings navigation");
    }
    assert!(!visible(cx, "hide-details"));
}
#[gpui::test]
fn native_session_repeated_slots_have_distinct_accessibility_paths(cx: &mut gpui::TestAppContext) {
    let mut task = Value::Null;
    let f = Fixture::projected(|s, root| {
        task = coding_plan(s, root).0;
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Conversation");
    let header = labels(cx, h, None);
    for caption in ["SOURCE STATUS", "WORKFLOW", "AGENT ACTIVITY", "READINESS"] {
        contains(&header, caption);
    }
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let slots: Vec<_> = gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter(|node| node.label() == Some("3 of 3 specialist slots free"))
            .collect();
        assert_eq!(slots.len(), 3);
        let paths: std::collections::HashSet<_> = slots
            .iter()
            .map(|node| format!("{:?}", node.path()))
            .collect();
        assert_eq!(
            paths.len(),
            3,
            "Repeated labels must have distinct accessibility identities"
        );
    })
    .unwrap();
    v.update(cx, |view, cx| {
        view.observation.runtime["teamProposals"] = json!([{
            "status":"pending", "specialists":[
                {"name":"Writer", "role":{"name":"Technical writer"}},
                {"name":"Reviewer", "role":{"name":"Code reviewer"}}
            ]
        }]);
        cx.notify();
    });
    let notes = labels(cx, h, None)
        .into_iter()
        .filter(|label| label.starts_with("No process exists yet"))
        .count();
    assert_eq!(notes, 2, "Each proposed specialist retains its own label");
}

#[gpui::test]
fn native_session_agents_show_terminals_only_for_admitted_attempts_and_cap_specialists(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut attempts) = (Value::Null, vec![]);
    let f = Fixture::projected(|s, root| {
        let (t, run, _) = coding_plan(s, root);
        task = t;
        let run = id(&run);
        s.claim_coding_slot(&run, true).unwrap();
        let roles = Roles::new(root.join("roles.json")).read().unwrap();
        let role = |id: &str| roles.iter().find(|r| r["id"] == id).unwrap().clone();
        let model = json!({"requested":{"model":"fixture/model","effort":"low"},"resolved":{"selector":"fixture/model","effort":"low"}});
        for (name, role_id, coordinator) in [
            ("Coordinator", "coordinator", true),
            ("Implementer", "implementer", false),
        ] {
            let admitted = s.admit_assignment(&run, &json!({"name":name,"coordinator":coordinator,"role":role(role_id),"model":model,"input":"Recorded fixture"})).unwrap();
            attempts.push(id(&admitted["attempt"]));
        }
        s.propose_operator_specialist(&run, &json!({"requestId":uuid::Uuid::new_v4().to_string(),"roleId":"tech-writer","input":"Document the change","dependsOn":[],"criteria":[]}), &role("tech-writer"), &model).unwrap();
        s.set_desired_state(&run, "paused").unwrap();
        s.begin_pause(&run, None).unwrap();
        s.finish_pause(&run).unwrap();
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Conversation");
    wait(cx, h, &v, &e, |v, _| {
        records(&v.observation.runtime, "teamProposals").len() == 1
    });
    let visible = |cx: &mut gpui::TestAppContext, id: &str| {
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(SharedString::from(id.to_owned())).is_some()
        })
        .unwrap()
    };
    let (coordinator, implementer) = (&attempts[0], &attempts[1]);
    // Admitted attempts get tiles; recorded fixtures have no process, so no terminal is claimed.
    assert_eq!(
        labels(cx, h, Some(&format!("agent-name-{coordinator}"))),
        ["Coordinator"]
    );
    assert_eq!(
        labels(cx, h, Some(&format!("agent-name-{implementer}"))),
        ["Implementer"]
    );
    contains(
        &labels(cx, h, Some(&format!("agent-note-{implementer}"))),
        "No terminal process yet",
    );
    contains(
        &labels(cx, h, Some(&format!("agent-state-{implementer}"))),
        "Not started",
    );
    // The pending proposal is drawn as not started, and one of three slots remains.
    assert_eq!(
        labels(cx, h, Some("proposed-state-0")),
        ["Proposed · not started"]
    );
    contains(&labels(cx, h, Some("proposed-name-0")), "Technical writer");
    assert!(visible(cx, "add-specialist-slot-2") && !visible(cx, "add-specialist-slot-1"));
    // Steering stays beside the coordinator in the session.
    assert!(visible(cx, "save-steering"));
    // The picker states why insertion is blocked; this fixture has no runner configured.
    click(cx, h, "add-specialist-slot-2");
    contains(
        &labels(cx, h, None),
        "Runner integration is unavailable; specialist models cannot be validated.",
    );
    // Maximize shows one tile; the timeline replaces the terminal area for that tile only.
    click(
        cx,
        h,
        SharedString::from(format!("agent-maximize-{implementer}")),
    );
    assert!(!visible(cx, &format!("agent-name-{coordinator}")) && !visible(cx, "save-steering"));
    click(
        cx,
        h,
        SharedString::from(format!("agent-view-{implementer}")),
    );
    contains(
        &labels(cx, h, None),
        "No recorded events for this attempt yet.",
    );
    click(
        cx,
        h,
        SharedString::from(format!("agent-maximize-{implementer}")),
    );
    assert!(visible(cx, &format!("agent-name-{coordinator}")) && visible(cx, "save-steering"));
    // Narrow windows keep every agent control inside the viewport.
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    keyboard_activate(cx, h, &format!("agent-maximize-{coordinator}"));
    assert!(!visible(cx, &format!("agent-name-{implementer}")));
}
fn fill(
    cx: &mut gpui::TestAppContext,
    h: AnyWindowHandle,
    v: &Entity<Workbench>,
    path: &str,
    text: &str,
) {
    cx.update_window(h, |_, window, cx| {
        let f = &v.read(cx).collaboration.forms[path].fields;
        if let Some(input) = f.inputs.get("reason") {
            input.clone().update(cx, |i, cx| i.focus(window, cx));
        } else {
            f.areas["reason"]
                .clone()
                .update(cx, |i, cx| i.focus(window, cx));
        }
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.press("backspace", cx);
        window.input(text, cx);
    })
    .unwrap();
}
fn needs(v: &Workbench) -> Vec<String> {
    records(&v.workspace, "needsYou")
        .iter()
        .map(|i| string(i, "id"))
        .collect()
}
#[gpui::test]
fn native_acceptance_needs_you_lists_recorded_decisions_and_drops_them_when_resolved(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run, mut assignment, mut learning) =
        (Value::Null, Value::Null, Value::Null, Value::Null);
    let f = Fixture::projected(|s, root| {
        let (t, r, a) = recorded(s, root);
        // Terminal-style output alone never creates a decision.
        s.record_agent_event(&id(&a["attempt"]),1,&json!({"type":"message","text":"Waiting for operator approval: allow one more repair cycle?"})).unwrap();
        pause(s, &r, &a);
        let mut attempt = s
            .replace_attempt(&id(&a["assignment"]), &json!({"automatic":true}))
            .unwrap();
        s.record_ownership(&id(&attempt), true, Some("Recorded stopped attempt"))
            .unwrap();
        attempt = s
            .replace_attempt(&id(&a["assignment"]), &json!({"automatic":true}))
            .unwrap();
        s.record_ownership(&id(&attempt), true, Some("Recorded stopped attempt"))
            .unwrap();
        // Synthetic recorded proposal; no agent or provider delivery is claimed.
        learning = s.propose_learning(&json!({"runId":r["id"],"attemptId":"record-fixture","toolCallId":uuid::Uuid::new_v4().to_string(),"target":{"kind":"role","roleId":"reviewer"},"baseVersion":1,"change":"Quote the scenario result at the reviewed revision.","rationale":"Reviews approved without the named scenario.","evidence":[{"kind":"steering","id":"recorded-fixture-evidence"}]})).unwrap();
        (task, run, assignment) = (t, r, a["assignment"].clone());
    });
    let (h, v, e) = open(cx, &f);
    let repair = format!("repair-{}", id(&assignment));
    let proposal = format!("learning-{}", id(&learning));
    wait(cx, h, &v, &e, |v, _| {
        v.connected && needs(v) == [repair.clone(), proposal.clone()]
    });
    contains(&labels(cx, h, Some("needs-you")), "Needs you · 2");
    click(cx, h, "needs-you");
    let page = labels(cx, h, None);
    contains(&page, "Coordinator used 2 of 2 automatic repair cycles");
    contains(&page, "Coordinator proposes a change to role");
    contains(&page, "Quote the scenario result at the reviewed revision.");
    click(cx, h, SharedString::from(format!("needs-open-{repair}")));
    v.read_with(cx, |v, _| {
        assert!(v.panel == Panel::Ticket && v.selected.as_deref() == Some(id(&task).as_str()))
    });
    click(cx, h, "needs-you");
    // The inbox action uses the session's own request path, form draft and server check.
    let path = format!(
        "/runs/{}/assignments/{}/repair-allowance",
        id(&run),
        id(&assignment)
    );
    fill(cx, h, &v, &path, "The recorded failure is understood.");
    click(cx, h, SharedString::from(format!("needs-allow-{repair}")));
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && needs(v) == [proposal.clone()]
    });
    let saved = f.get(&format!("/runs/{}/runtime", id(&run)));
    assert_eq!(
        saved["assignments"][0]["repairAllowances"][0]["reason"],
        "The recorded failure is understood."
    );
    contains(&labels(cx, h, Some("needs-you")), "Needs you · 1");
    // A decision recorded elsewhere removes the item without an inbox action.
    let (status, _) = f.post(
        &format!("/learning/{}/decision", id(&learning)),
        json!({"decision":"reject","reason":"Decided on the learning screen","requestId":uuid::Uuid::new_v4().to_string()}),
    );
    assert_eq!(status, 200);
    wait(cx, h, &v, &e, |v, _| needs(v).is_empty());
    contains(&labels(cx, h, None), "Nothing needs you");
    // A saved brief revision asks the run to adopt it; started work must give a reason.
    let (status, _) = f.post(
        &format!("/tasks/{}/refinement", id(&task)),
        json!({"revision":task["revision"],"sourceRevision":task["sourceRevision"],"brief":"Adopted brief","criteria":["Adopted criterion"]}),
    );
    assert_eq!(status, 200);
    let adopt = format!("requirements-adopt-{}", id(&run));
    wait(cx, h, &v, &e, |v, _| needs(v) == [adopt.clone()]);
    let item = records(&v.read_with(cx, |v, _| v.workspace.clone()), "needsYou")[0].clone();
    assert_eq!(item["reasonRequired"], true);
    let button = SharedString::from(format!("needs-adopt-{adopt}"));
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(button.clone()).is_some());
    })
    .unwrap();
    click(cx, h, button.clone());
    assert_eq!(v.read_with(cx, |v, _| needs(v)), [adopt.as_str()]);
    fill(
        cx,
        h,
        &v,
        &format!("/runs/{}/adopt-requirements", id(&run)),
        "Scope clarified before resuming.",
    );
    click(cx, h, button);
    wait(cx, h, &v, &e, |v, _| !v.busy && needs(v).is_empty());
    assert_eq!(
        f.get(&format!("/runs/{}/runtime", id(&run)))["run"]["taskSnapshot"]["brief"],
        "Adopted brief"
    );
    // The inbox stays reachable by keyboard from the narrow navigation at 390 px.
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    keyboard_activate(cx, h, "open-navigation");
    keyboard_activate(cx, h, "needs-you");
    contains(&labels(cx, h, None), "Nothing needs you");
}
#[gpui::test]
fn native_acceptance_needs_you_is_unavailable_without_the_execution_layer(
    cx: &mut gpui::TestAppContext,
) {
    let f = Fixture::start();
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| v.connected);
    contains(&labels(cx, h, Some("needs-you")), "Needs you · unavailable");
    click(cx, h, "needs-you");
    contains(&labels(cx, h, None), "Decisions unavailable");
}

#[gpui::test]
fn native_packages_keep_plans_receipts_handoff_and_drafts_distinct_at_390(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let f = Fixture::projected(|s, root| {
        let (t, _, _) = coding_plan(s, root);
        task = t;
    });
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    navigate(cx, h, "Conversation");
    let draft = cx
        .update_window(h, |_, window, cx| {
            window.render_frame(cx);
            let input = v.read(cx).steering.values().next().unwrap().clone();
            input.update(cx, |input, cx| input.focus(window, cx));
            window.input("Keep cursor zero and 日本語 in scope", cx);
            input
        })
        .unwrap();
    v.update(cx,|v,cx| {
        v.observation.runtime["choreography"] = json!({"revision":9,"binding":{"planId":"accepted-plan","requirementRevision":2},"schedule":{"assignments":[{"name":"Writer","responsibility":"writer","stage":0,"packages":["backend"],"rationale":"Implement bounded pagination"},{"name":"Navigator","responsibility":"navigator","stage":0,"packages":["backend"],"rationale":"Inspect cursor edge cases"},{"name":"Docs","responsibility":"documentation","stage":1,"packages":["backend"],"rationale":"Explain the continuation cursor"}]},"packages":[{"id":"backend","state":"blocked","reason":"Resolve zero cursor handling"}],"reports":[{"id":"report","packageId":"backend","name":"Docs","summary":"Cursor semantics documented","source":{"head":"abcdef12345"},"documentation":{"disposition":"changed","reason":"Document cursor zero"}}],"exchanges":[{"id":"concern","packageId":"backend","kind":"challenge","messageId":"mail","artifact":"pagination.js","body":"A zero cursor must not restart pagination.","proposedChange":"Add a zero-cursor regression test.","source":{"head":"abcdef12345"}}]});
        v.observation.runtime["choreographyBlockers"] = json!(["Package backend has an unresolved navigator concern."]);
        v.observation.runtime["messages"] = json!([{"id":"mail","disposition":"stored"}]);
        v.observation.runtime["packageCompletions"] = json!([{"id":"checkpoint","mode":"checkpoint","state":"requested","successor":"Docs","summary":"Stop before documentation transfer"}]);
        cx.notify();
    });
    let before = labels(cx, h, None);
    contains(&before, "Planned · No process");
    contains(&before, "Current writer: None");
    contains(&before, "unresolved navigator concern");
    contains(
        &before,
        "backend · challenge · concern · Stored · Receipt unconfirmed",
    );
    contains(&before, "Stop and ownership verification pending");
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    contains(
        &labels(cx, h, None),
        "A zero cursor must not restart pagination.",
    );
    v.update(cx, |v, cx| {
        v.observation.runtime["packageCompletions"][0]["state"] = json!("completed");
        v.observation.runtime["packageClaims"] = json!([{"handoffId":"checkpoint"}]);
        v.observation.runtime["messages"][0]["disposition"] = json!("received");
        cx.notify();
    });
    contains(
        &labels(cx, h, None),
        "Transferred · Successor claim recorded",
    );
    contains(
        &labels(cx, h, Some("package-exchange-concern")),
        "Receipt confirmed",
    );
    // The existing steering field remains keyboard reachable beside package history.
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("save-steering").is_some());
        assert_eq!(
            draft.read(cx).value(),
            "Keep cursor zero and 日本語 in scope"
        );
        assert!(draft.read(cx).focus_handle(cx).is_focused(window));
        window.press("tab", cx);
        window.press("shift-tab", cx);
        assert!(draft.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
}
#[gpui::test]
fn native_jira_ticket_labels_source_status_with_provider_name_apart_from_workflow_and_activity(
    cx: &mut gpui::TestAppContext,
) {
    // FOR-40: the provider-neutral source model drives the Jira label.
    let mut task = Value::Null;
    let f = Fixture::projected(|s, _| {
        let imported = s.import_jira(&json!({"siteUrl":"https://fixture.atlassian.net/","issues":[{"id":"700","key":"SIM-7","fields":{"summary":"Imported fixture","description":"Source text","status":{"id":"3","name":"Backlog"}}}]})).unwrap();
        task = imported[0].clone();
        s.queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
    });
    assert_eq!(task["sourceStatus"], "Backlog");
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    let row = |cx: &mut gpui::TestAppContext, kind: &str| {
        labels(cx, h, Some(&format!("row-{kind}-{}", id(&task))))
    };
    assert_eq!(row(cx, "jira"), ["Jira Backlog"]);
    assert_eq!(
        labels(cx, h, Some(&format!("row-key-{}", id(&task)))),
        ["SIM-7 · jira"]
    );
    let workflow = row(cx, "workflow");
    let activity = row(cx, "activity");
    assert!(!workflow.is_empty() && !activity.is_empty());
    assert!(
        !workflow
            .iter()
            .chain(&activity)
            .any(|l| l.contains("Backlog"))
    );
    assert_eq!(labels(cx, h, Some("fact-jira")), ["Backlog"]);
    let header = labels(cx, h, None);
    for caption in [
        "JIRA STATUS",
        "Imported Jira field",
        "SIM-7  /  Jira ticket",
    ] {
        contains(&header, caption);
    }
    for fact in ["fact-workflow", "fact-activity"] {
        assert!(
            !labels(cx, h, Some(fact))
                .iter()
                .any(|l| l.contains("Backlog"))
        );
    }
}
#[gpui::test]
fn native_github_issue_shows_github_open_closed_apart_from_workflow_and_activity(
    cx: &mut gpui::TestAppContext,
) {
    // FOR-45: a GitHub issue's open/closed state is its source status only.
    let mut task = Value::Null;
    let f = Fixture::projected(|s, _| {
        let source = s
            .save_github_source(&json!({"repository":"fixture/issues"}))
            .unwrap()["source"]
            .clone();
        s.commit_github_sync(
            &source,
            &json!([{"id":9012,"number":12,"title":"Synthetic GitHub issue","body":"Source text","state":"open","html_url":"https://github.com/fixture/issues/issues/12","updated_at":"2026-10-18T08:00:00Z"}]),
            &json!([]),
        )
        .unwrap();
        task = s.snapshot().unwrap()["tasks"][0].clone();
        s.queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
    });
    assert_eq!(task["sourceStatus"], "open");
    let (h, v, e) = open(cx, &f);
    select(cx, h, &v, &e, &task);
    let row = |cx: &mut gpui::TestAppContext, kind: &str| {
        labels(cx, h, Some(&format!("row-{kind}-{}", id(&task))))
    };
    assert_eq!(row(cx, "jira"), ["GitHub open"]);
    assert_eq!(
        labels(cx, h, Some(&format!("row-key-{}", id(&task)))),
        ["#12 · github"]
    );
    let workflow = row(cx, "workflow");
    let activity = row(cx, "activity");
    assert!(!workflow.is_empty() && !activity.is_empty());
    assert!(
        !workflow
            .iter()
            .chain(&activity)
            .any(|l| l.contains("GitHub"))
    );
    assert_eq!(labels(cx, h, Some("fact-jira")), ["open"]);
    let header = labels(cx, h, None);
    for caption in [
        "GITHUB STATUS",
        "Imported GitHub field",
        "#12  /  GitHub ticket",
    ] {
        contains(&header, caption);
    }
    for fact in ["fact-workflow", "fact-activity"] {
        assert!(!labels(cx, h, Some(fact)).iter().any(|l| l == "open"));
    }
}
