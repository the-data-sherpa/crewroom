//! Rendered blocker guidance for disabled decision and recovery controls (FOR-28).
use super::test_support::{Fixture, click, open, wait};
use super::*;
use gpui::AnyWindowHandle;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;
fn saved(f: &Fixture, path: &str, body: Value) -> Value {
    let (status, value) = f.post(path, body);
    assert_eq!(status, 200, "{path}: {value}");
    value
}
fn checkout(path: &std::path::Path) {
    std::fs::create_dir(path).unwrap();
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
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}
fn labels(cx: &mut gpui::TestAppContext, h: AnyWindowHandle) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}
fn contains(labels: &[String], expected: &str) {
    assert!(
        labels.iter().any(|l| l.contains(expected)),
        "Missing {expected:?} in {labels:#?}"
    );
}
/// Button snapshots do not expose a disabled flag, so a disabled control is shown
/// by activating it: nothing is sent and no service refusal is reported.
fn inert(
    cx: &mut gpui::TestAppContext,
    h: AnyWindowHandle,
    v: &Entity<Workbench>,
    events: &async_channel::Receiver<Event>,
    id: &str,
) {
    click(cx, h, SharedString::from(id.to_owned()));
    let sent = cx.read(|cx| v.read(cx).busy);
    wait(cx, h, v, events, |v, _| !v.busy);
    let error = cx.read(|cx| v.read(cx).error.clone());
    assert!(!sent && error.is_empty(), "{id} accepted input: {error}");
}
fn issue(description: &str, updated: &str) -> Value {
    json!({"id":"1","key":"FOR-1","fields":{"summary":"Changed upstream","description":description,"status":{"name":"To Do"},"updated":updated}})
}
fn task(f: &Fixture) -> Value {
    f.get("/workspace")["tasks"][0].clone()
}

#[gpui::test]
fn jira_change_plan_review_names_save_brief_before_any_disabled_control(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let path = fixture.root.path().join("checkout");
    checkout(&path);
    let repo = saved(
        &fixture,
        "/repositories",
        json!({"name":"Blocker checkout","path":path,"targetBranch":"main"}),
    );
    saved(
        &fixture,
        "/import/jira",
        json!({"siteUrl":"https://example.atlassian.net","issues":[issue("First","2026-10-04T12:00:00.000+0000")]}),
    );
    let t = task(&fixture);
    let id = string(&t, "id");
    saved(
        &fixture,
        &format!("/tasks/{id}/repository"),
        json!({"repositoryId":repo["id"],"revision":t["revision"]}),
    );
    let t = task(&fixture);
    saved(
        &fixture,
        &format!("/tasks/{id}/refinement"),
        json!({"revision":t["revision"],"sourceRevision":t["sourceRevision"],"brief":"Keep behavior","criteria":["Retain evidence"]}),
    );
    let t = task(&fixture);
    let source = saved(
        &fixture,
        &format!("/tasks/{id}/plan-source"),
        json!({"revision":t["revision"]}),
    );
    saved(
        &fixture,
        &format!("/tasks/{id}/implementation-plans"),
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":t["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Blocker fixture","scope":["Keep behavior"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Evidence"],"criteria":[0],"validation":["Native fixture"]}]}}),
    );
    saved(
        &fixture,
        "/import/jira",
        json!({"siteUrl":"https://example.atlassian.net","issues":[issue("Second","2026-10-04T13:00:00.000+0000")]}),
    );
    assert_eq!(task(&fixture)["needsReacceptance"], true);

    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected
            && records(&v.workspace, "tasks")
                .first()
                .is_some_and(|t| t["needsReacceptance"] == true)
    });
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    let shown = labels(cx, h);
    contains(
        &shown,
        "To decide: save the brief to accept the changed Jira requirements, refresh the repository basis and save a new plan version.",
    );
    contains(
        &shown,
        "To refresh the basis: save the brief to accept the changed Jira requirements.",
    );
    assert!(
        !shown
            .iter()
            .any(|l| l.starts_with("The task or repository basis changed. Refresh")),
        "Guidance must not lead with the disabled Refresh: {shown:#?}"
    );
    inert(cx, h, &v, &events, "plan-accept");
    inert(cx, h, &v, &events, "refresh-plan-source");
    click(cx, h, "save-brief");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.workspace, "tasks")[0]["needsReacceptance"] == false
    });
    let shown = labels(cx, h);
    contains(
        &shown,
        "To decide: refresh the repository basis and save a new plan version.",
    );
    assert!(
        !shown.iter().any(|l| l.starts_with("To refresh the basis:")),
        "{shown:#?}"
    );
    // The next named step is available: the refreshed basis now covers the saved brief.
    click(cx, h, "refresh-plan-source");
    wait(cx, h, &v, &events, |v, _| !v.busy);
    let error = cx.read(|cx| v.read(cx).error.clone());
    assert!(error.is_empty(), "{error}");
    let revision = task(&fixture)["revision"].clone();
    assert_eq!(
        fixture.get("/workspace")["intake"][&id]["source"]["taskRevision"],
        revision
    );
}

#[gpui::test]
fn empty_repository_state_routes_to_repositories_in_one_activation(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let t = saved(
        &fixture,
        "/tasks",
        json!({"title":"No repository yet","description":""}),
    );
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "tasks").len() == 1
    });
    click(
        cx,
        h,
        SharedString::from(format!("ticket-{}", string(&t, "id"))),
    );
    contains(
        &labels(cx, h),
        "No active repositories. Add one to choose a target.",
    );
    click(cx, h, "plan-add-repository");
    cx.read(|cx| assert!(matches!(v.read(cx).panel, Panel::Repositories)));
}

#[gpui::test]
fn retired_target_and_open_run_name_the_controls_that_clear_them(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let path = fixture.root.path().join("checkout");
    checkout(&path);
    let retired = saved(
        &fixture,
        "/repositories",
        json!({"name":"Old checkout","path":path,"targetBranch":"main"}),
    );
    let t = saved(
        &fixture,
        "/tasks",
        json!({"title":"Retired target","description":"","repositoryId":retired["id"]}),
    );
    saved(
        &fixture,
        &format!("/repositories/{}/retire", string(&retired, "id")),
        json!({"revision":retired["revision"],"requestId":uuid::Uuid::new_v4().to_string()}),
    );
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected
            && records(&v.workspace, "repositories")
                .iter()
                .any(|r| !r["retiredAt"].is_null())
    });
    click(
        cx,
        h,
        SharedString::from(format!("ticket-{}", string(&t, "id"))),
    );
    let shown = labels(cx, h);
    contains(
        &shown,
        "Selected repository is retired. Choose an active repository before you select Queue run.",
    );
    contains(
        &shown,
        "No active repositories. Add one to choose a target.",
    );

    let other = fixture.root.path().join("active");
    checkout(&other);
    let active = saved(
        &fixture,
        "/repositories",
        json!({"name":"Active checkout","path":other,"targetBranch":"main"}),
    );
    let id = string(&t, "id");
    let current = task(&fixture);
    saved(
        &fixture,
        &format!("/tasks/{id}/repository"),
        json!({"repositoryId":active["id"],"revision":current["revision"]}),
    );
    let current = task(&fixture);
    saved(
        &fixture,
        &format!("/tasks/{id}/refinement"),
        json!({"revision":current["revision"],"sourceRevision":current["sourceRevision"],"brief":"Keep behavior","criteria":["Retain evidence"]}),
    );
    fixture.accept_plan(&id);
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "tasks")[0]["repositoryId"] == active["id"]
            && v.workspace["intake"][&id]["currentPlan"].is_object()
    });
    click(cx, h, "queue-plan");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && !records(&v.workspace, "runs").is_empty()
    });
    contains(
        &labels(cx, h),
        "To change the target: cancel the open run or let it finish.",
    );
    inert(
        cx,
        h,
        &v,
        &events,
        &format!("assign-{}", string(&active, "id")),
    );
}
