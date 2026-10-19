//! Snoozed and Settled shelves (FOR-78) over an injected workspace projection. Nothing here
//! reaches a service or an agent; the point is that shelving never changes one.
use super::acceptance_tests::labels;
use super::navigation::{Navigation, Watch};
use super::projects_tests::{open_injected, present, press, render, task, workspace};
use super::*;
use chrono::{Duration as Hours, Utc};
use gpui_kit::{self as gpui, AnyWindowHandle, test::TestWindowExt};
use std::time::Duration;

fn one(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> Vec<String> {
    labels(cx, h, Some(id))
}

fn chips(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> Vec<Vec<String>> {
    ["source", "workflow", "activity"]
        .iter()
        .map(|kind| one(cx, h, &format!("row-{kind}-{id}")))
        .collect()
}

fn button_label(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> String {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window
            .find(SharedString::from(id.to_owned()))
            .label()
            .unwrap_or_default()
            .to_owned()
    })
    .unwrap()
}

#[gpui::test]
fn snooze_and_settle_shelve_tickets_without_changing_run_workflow_or_source(
    cx: &mut gpui::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    let descriptor = root.path().join("native-connection.json");
    let (h, view) = open_injected(cx, workspace(), Some(&descriptor));
    render(cx, h);
    let before = view.read_with(cx, |v, _| v.workspace.clone());
    let running_chips = chips(cx, h, "a-running");
    let queued_chips = chips(cx, h, "a-queued");

    // Snooze opens its choices; events that cannot be watched say so and stay disabled.
    press(cx, h, "snooze-a-other");
    assert!(present(cx, h, "snooze-1h-a-other"));
    assert_eq!(
        one(cx, h, "snooze-unavailable-a-other"),
        ["PR events need a confirmed PR; source status needs a linked ticket source."]
    );
    for disabled in [
        "snooze-review-a-other",
        "snooze-ci-a-other",
        "snooze-source-a-other",
    ] {
        press(cx, h, disabled);
    }
    assert!(view.read_with(cx, |v, _| v.shelves().snoozed.is_empty()));
    press(cx, h, "snooze-1h-a-other");
    // The shelf starts collapsed: the ticket leaves Other and is counted on Snoozed.
    assert!(!present(cx, h, "row-key-a-other"));
    assert!(!present(cx, h, "project-alpha-other"));
    assert_eq!(
        button_label(cx, h, "shelf-alpha-snoozed"),
        "Expand Snoozed · 1"
    );
    assert!(one(cx, h, "shelf-undo-label")[0].starts_with("Snoozed SIM-a-other until "));

    // Settle a running and a queued ticket: they leave their sections, yet the project still
    // shows them running, and their chips stay exactly what they were.
    press(cx, h, "settle-a-running");
    press(cx, h, "settle-a-queued");
    assert_eq!(one(cx, h, "project-running-alpha"), ["● 2 running"]);
    assert_eq!(
        button_label(cx, h, "shelf-alpha-settled"),
        "Expand Settled · 2"
    );
    assert!(!present(cx, h, "row-key-a-running"));
    press(cx, h, "shelf-alpha-settled");
    assert_eq!(chips(cx, h, "a-running"), running_chips);
    assert_eq!(chips(cx, h, "a-queued"), queued_chips);
    press(cx, h, "shelf-alpha-snoozed");
    assert!(one(cx, h, "row-snoozed-a-other")[0].starts_with("Snoozed until "));

    // No command was sent and the workspace projection is untouched.
    view.read_with(cx, |v, _| {
        assert!(v.pending_commands.is_empty() && !v.busy);
        assert_eq!(v.workspace, before);
    });

    // Un-settle returns the ticket to its section; Wake now returns a snoozed one.
    press(cx, h, "unsettle-a-queued");
    assert!(present(cx, h, "project-alpha-queued"));
    assert_eq!(one(cx, h, "row-workflow-a-queued"), ["Queued"]);
    press(cx, h, "wake-a-other");
    assert!(present(cx, h, "project-alpha-other"));
    assert!(!present(cx, h, "shelf-alpha-snoozed"));

    // Shelves are per project: beta's settled shelf has its own collapsed state.
    press(cx, h, "settle-b-other");
    assert_eq!(
        button_label(cx, h, "shelf-beta-settled"),
        "Expand Settled · 1"
    );
    assert!(!present(cx, h, "row-key-b-other"));
    assert_eq!(
        button_label(cx, h, "shelf-alpha-settled"),
        "Collapse Settled · 1"
    );

    // The shelves and their open state are the only thing saved, and they survive a restart.
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
    let saved = Navigation::new(Some(&descriptor)).saved().shelves.clone();
    assert_eq!(saved.settled, ["a-running", "b-other"]);
    assert!(saved.snoozed.is_empty());
    assert_eq!(saved.open, ["alpha/settled", "alpha/snoozed"]);
    let (h, _) = open_injected(cx, workspace(), Some(&descriptor));
    render(cx, h);
    assert!(present(cx, h, "row-key-a-running"));
    assert!(!present(cx, h, "row-key-b-other"));
}

#[gpui::test]
fn undo_is_offered_for_five_seconds_after_each_shelf_action(cx: &mut gpui::TestAppContext) {
    let (h, view) = open_injected(cx, workspace(), None);
    render(cx, h);
    let shelves = |cx: &mut gpui::TestAppContext| view.read_with(cx, |v, _| v.shelves().clone());

    // Settle → Undo.
    press(cx, h, "settle-a-other");
    assert_eq!(one(cx, h, "shelf-undo-label"), ["Settled SIM-a-other"]);
    press(cx, h, "shelf-undo");
    assert!(shelves(cx).settled.is_empty());
    assert!(!present(cx, h, "shelf-undo-bar"));

    // Snooze → Undo.
    press(cx, h, "snooze-a-other");
    press(cx, h, "snooze-4h-a-other");
    press(cx, h, "shelf-undo");
    assert!(shelves(cx).snoozed.is_empty());

    // Un-settle → Undo puts it back on the shelf.
    press(cx, h, "settle-b-other");
    press(cx, h, "shelf-beta-settled");
    press(cx, h, "unsettle-b-other");
    assert_eq!(one(cx, h, "shelf-undo-label"), ["Un-settled SIM-b-other"]);
    press(cx, h, "shelf-undo");
    assert_eq!(shelves(cx).settled, ["b-other"]);

    // Wake now → Undo restores the same snooze.
    press(cx, h, "snooze-a-other");
    press(cx, h, "snooze-1h-a-other");
    let snoozed = shelves(cx).snoozed;
    press(cx, h, "shelf-alpha-snoozed");
    press(cx, h, "wake-a-other");
    assert!(shelves(cx).snoozed.is_empty());
    press(cx, h, "shelf-undo");
    assert_eq!(shelves(cx).snoozed, snoozed);

    // The offer lasts five seconds, measured on the test clock.
    press(cx, h, "settle-a-queued");
    cx.executor().advance_clock(Duration::from_millis(4900));
    render(cx, h);
    assert!(present(cx, h, "shelf-undo"));
    cx.executor().advance_clock(Duration::from_millis(200));
    render(cx, h);
    assert!(!present(cx, h, "shelf-undo"));
    assert!(shelves(cx).settled.contains(&"a-queued".to_owned()));
}

/// The alpha workspace plus a ticket with a GitHub source and a confirmed PR.
fn watched_workspace() -> Value {
    let mut w = workspace();
    let mut pr = task("a-pr", Some("alpha"));
    pr["source"] = json!("github");
    pr["sourceStatus"] = json!("open");
    w["tasks"].as_array_mut().unwrap().push(pr);
    w["runs"].as_array_mut().unwrap().push(json!({
        "id":"run-pr","taskId":"a-pr","status":"paused","purpose":"coding",
        "publicationWatch":{"pr":7,"providerState":"OPEN","feedbackRead":true,
            "feedback":["comment:1"],"ci":{"state":"running","head":"abc"}}
    }));
    w
}

fn refresh(
    cx: &mut gpui::TestAppContext,
    view: &Entity<Workbench>,
    change: impl FnOnce(&mut Value),
) {
    view.update(cx, |v, cx| {
        let mut next = v.workspace.clone();
        change(&mut next);
        v.accept(crate::client::Event::Workspace(next), cx);
    });
}

#[gpui::test]
fn snoozed_tickets_wake_with_their_reason_and_survive_a_restart(cx: &mut gpui::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let descriptor = root.path().join("native-connection.json");
    let (h, view) = open_injected(cx, watched_workspace(), Some(&descriptor));
    render(cx, h);
    let woke = |cx: &mut gpui::TestAppContext, view: &Entity<Workbench>, id: &str| {
        view.read_with(cx, |v, _| {
            v.shelves()
                .woke
                .iter()
                .find(|w| w.task == id)
                .map(|w| w.reason.clone())
        })
    };
    // Every watched event is offered for a ticket with a source and a confirmed PR.
    press(cx, h, "snooze-a-pr");
    assert!(!present(cx, h, "snooze-unavailable-a-pr"));
    press(cx, h, "snooze-review-a-pr");
    let now = Utc::now();
    view.update(cx, |v, cx| {
        v.snooze("a-other", Some(now + Hours::hours(1)), None, cx);
        v.snooze("b-other", None, None, cx);
    });
    assert!(
        view.read_with(cx, |v, _| v.snoozed("b-other").is_none()),
        "a snooze needs a time or an event"
    );

    // Snoozes survive a restart before they wake.
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
    let (h, view) = open_injected(cx, watched_workspace(), Some(&descriptor));
    render(cx, h);
    view.read_with(cx, |v, _| {
        assert_eq!(v.snoozed("a-pr").unwrap().event, Some(Watch::Review));
        assert_eq!(v.snoozed("a-pr").unwrap().baseline, json!(["comment:1"]));
        assert!(v.snoozed("a-other").unwrap().until.is_some());
    });
    // Nothing changed yet: an unchanged refresh wakes nothing.
    refresh(cx, &view, |_| {});
    assert!(view.read_with(cx, |v, _| v.shelves().woke.is_empty()));

    // Time: once it passes, the ticket returns with its reason.
    assert!(view.update(cx, |v, _| v.wake_at(now + Hours::hours(2))));
    assert!(
        woke(cx, &view, "a-other")
            .unwrap()
            .starts_with("Snooze ended ")
    );
    assert!(present(cx, h, "row-key-a-other"));
    assert!(one(cx, h, "row-woke-a-other")[0].starts_with("Woke · Snooze ended "));

    // A PR review the PR did not have when snoozed.
    refresh(cx, &view, |w| {
        w["runs"][3]["publicationWatch"]["feedback"] = json!(["comment:1", "review:9"])
    });
    assert_eq!(
        woke(cx, &view, "a-pr").as_deref(),
        Some("PR review submitted")
    );

    // CI finishing.
    view.update(cx, |v, cx| v.snooze("a-pr", None, Some(Watch::Ci), cx));
    refresh(cx, &view, |w| {
        w["runs"][3]["publicationWatch"]["ci"] =
            json!({"state":"finished","head":"abc","outcome":"passed"})
    });
    assert_eq!(
        woke(cx, &view, "a-pr").as_deref(),
        Some("CI finished · passed")
    );

    // Source status changing; the snooze replaced the earlier wake reason.
    view.update(cx, |v, cx| v.snooze("a-pr", None, Some(Watch::Source), cx));
    assert_eq!(woke(cx, &view, "a-pr"), None);
    refresh(cx, &view, |w| {
        w["tasks"][7]["sourceStatus"] = json!("closed")
    });
    assert_eq!(
        woke(cx, &view, "a-pr").as_deref(),
        Some("Source status changed to GitHub closed")
    );

    // A ticket that needs the operator wakes at once, even with a long snooze ahead, and it is
    // listed under Needs you rather than on the shelf.
    let workflow = one(cx, h, "row-workflow-a-running");
    view.update(cx, |v, cx| {
        v.snooze("a-running", Some(now + Hours::hours(48)), None, cx)
    });
    assert!(!present(cx, h, "row-key-a-running"));
    refresh(cx, &view, |w| {
        w["needsYou"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"q","kind":"repair","title":"Coordinator asks a question","taskId":"a-running","runId":"run-running"}))
    });
    assert_eq!(
        woke(cx, &view, "a-running").as_deref(),
        Some("Needs you · Coordinator asks a question")
    );
    assert!(present(cx, h, "row-key-a-running"));
    assert_eq!(one(cx, h, "project-alpha-needs-you"), ["Needs you · 2"]);
    // Waking changed no workflow fact.
    assert_eq!(one(cx, h, "row-workflow-a-running"), workflow);

    // Wake reasons survive a restart until dismissed.
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
    let (h, view) = open_injected(cx, watched_workspace(), Some(&descriptor));
    render(cx, h);
    assert!(one(cx, h, "row-woke-a-other")[0].starts_with("Woke · Snooze ended "));
    press(cx, h, "woke-dismiss-a-other");
    assert!(!present(cx, h, "row-woke-a-other"));
    assert_eq!(view.read_with(cx, |v, _| v.shelves().woke.len()), 2);
}
