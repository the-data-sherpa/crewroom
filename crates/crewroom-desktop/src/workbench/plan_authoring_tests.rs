//! Plan authoring guidance and decision outcomes in operator words (FOR-35).
use super::test_support::{Fixture, click, open, screen_text, wait};
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
fn contains(labels: &[String], expected: &str) {
    assert!(
        labels.iter().any(|l| l.contains(expected)),
        "Missing {expected:?} in {labels:#?}"
    );
}
fn absent(labels: &[String], unexpected: &str) {
    assert!(
        !labels.iter().any(|l| l.contains(unexpected)),
        "Unexpected {unexpected:?} in {labels:#?}"
    );
}
fn task(f: &Fixture, id: &str) -> Value {
    records(&f.get("/workspace"), "tasks")
        .into_iter()
        .find(|t| t["id"] == id)
        .unwrap()
}
/// A local ticket with a target repository and a saved brief of `criteria`.
fn ready_ticket(f: &Fixture, criteria: &[&str]) -> String {
    let path = f.root.path().join("checkout");
    checkout(&path);
    let repo = saved(
        f,
        "/repositories",
        json!({"name":"Plan checkout","path":path,"targetBranch":"main"}),
    );
    let t = saved(
        f,
        "/tasks",
        json!({"title":"Plan authoring","description":"","repositoryId":repo["id"]}),
    );
    let id = string(&t, "id");
    let t = task(f, &id);
    saved(
        f,
        &format!("/tasks/{id}/refinement"),
        json!({"revision":t["revision"],"sourceRevision":t["sourceRevision"],"brief":"Keep behavior","criteria":criteria}),
    );
    id
}
fn type_into(
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
fn queue_without_an_accepted_plan_reports_scoping_not_a_queued_plan(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let id = ready_ticket(&fixture, &["Retain evidence"]);
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    click(cx, h, "queue-plan");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && !records(&v.workspace, "runs").is_empty()
    });
    let error = cx.read(|cx| v.read(cx).error.clone());
    assert!(error.is_empty(), "{error}");
    assert_eq!(
        cx.read(|cx| v.read(cx).notice.clone()),
        "Scoping requested. A coordinator will draft a plan for you to review; coding waits for your acceptance."
    );
}

#[gpui::test]
fn rejecting_an_operator_plan_names_who_acts_next(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let id = ready_ticket(&fixture, &["Retain evidence"]);
    let t = task(&fixture, &id);
    let source = saved(
        &fixture,
        &format!("/tasks/{id}/plan-source"),
        json!({"revision":t["revision"]}),
    );
    let plan = saved(
        &fixture,
        &format!("/tasks/{id}/implementation-plans"),
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":t["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Operator plan","scope":["Keep behavior"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Evidence"],"criteria":[0],"validation":["Native fixture"]}]}}),
    );
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && v.workspace["intake"][&id]["plans"].is_array()
    });
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    let shown = screen_text(cx, h);
    contains(&shown, "Plan 1 · Awaiting decision");
    contains(&shown, "Reject plan");
    contains(
        &shown,
        "No agent will revise a rejected plan. Use Edit as a new plan version to write the next one.",
    );
    absent(&shown, "Request revision");
    absent(&shown, "Proposed");
    let path = format!("/implementation-plans/{}/decision", string(&plan, "id"));
    type_into(cx, h, &v, &path, "reason", "Wrong approach");
    click(cx, h, "plan-reject");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && v.workspace["intake"][&id]["plans"][0]["decision"].is_object()
    });
    let error = cx.read(|cx| v.read(cx).error.clone());
    assert!(error.is_empty(), "{error}");
    assert_eq!(
        cx.read(|cx| v.read(cx).notice.clone()),
        "Plan 1 rejected. No agent will revise it."
    );
    let shown = screen_text(cx, h);
    contains(&shown, "Plan rejected");
    contains(&shown, "Plan 1 · Rejected");
    absent(&shown, "Needs revalidation");
}

#[gpui::test]
fn prepare_a_plan_names_its_prerequisites_until_the_editor_is_available(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let path = fixture.root.path().join("checkout");
    checkout(&path);
    let repo = saved(
        &fixture,
        "/repositories",
        json!({"name":"Plan checkout","path":path,"targetBranch":"main"}),
    );
    let t = saved(
        &fixture,
        "/tasks",
        json!({"title":"Fresh ticket","description":""}),
    );
    let id = string(&t, "id");
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    let shown = screen_text(cx, h);
    contains(
        &shown,
        "To write a plan here, choose a project below, then select Refresh repository basis.",
    );
    absent(&shown, "prepare a plan here");
    click(
        cx,
        h,
        SharedString::from(format!("assign-{}", string(&repo, "id"))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.workspace, "tasks")[0]["repositoryId"] == repo["id"]
    });
    let shown = screen_text(cx, h);
    contains(
        &shown,
        "Select Refresh repository basis to pin the source a plan is written against.",
    );
    absent(&shown, "choose a project below");
    click(cx, h, "refresh-plan-source");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && v.workspace["intake"][&id]["source"].is_object()
    });
    let shown = screen_text(cx, h);
    absent(&shown, "choose a project below");
    absent(&shown, "Select Refresh repository basis to pin");
    contains(&shown, "Write a plan");
}

#[gpui::test]
fn plan_editor_names_package_and_field_and_stays_quiet_until_edited(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let id = ready_ticket(&fixture, &["First", "Second", "Third"]);
    let t = task(&fixture, &id);
    saved(
        &fixture,
        &format!("/tasks/{id}/plan-source"),
        json!({"revision":t["revision"]}),
    );
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && v.workspace["intake"][&id]["source"].is_object()
    });
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    click(cx, h, "edit-implementation-plan");
    let shown = screen_text(cx, h);
    absent(&shown, "Expected a bounded text list.");
    absent(&shown, "Scope: add at least one item.");

    click(cx, h, "add-plan-package");
    let editor = format!("/tasks/{id}/implementation-plans");
    let new_id = cx.read(|cx| {
        let v = v.read(cx);
        let row = v.collaboration.forms[&editor].rows.last().unwrap().clone();
        v.collaboration.forms[&format!("{editor}/packages/{row}")]
            .fields
            .text("packageId", cx)
    });
    assert_eq!(new_id, "package-2");
    type_into(
        cx,
        h,
        &v,
        &format!("{editor}/packages/0"),
        "criteria",
        "1, 4",
    );
    let shown = screen_text(cx, h);
    contains(&shown, "Scope: add at least one item.");
    contains(&shown, "Work package 2 · Name: add a name.");
    contains(
        &shown,
        "Work package 2 · Expected outputs: add at least one line.",
    );
    contains(&shown, "Work package 1 · Criterion 4 does not exist (1–3).");
    absent(&shown, "Expected a bounded text list.");

    // A second added package takes the next free number.
    click(cx, h, "add-plan-package");
    let next = cx.read(|cx| {
        let v = v.read(cx);
        let row = v.collaboration.forms[&editor].rows.last().unwrap().clone();
        v.collaboration.forms[&format!("{editor}/packages/{row}")]
            .fields
            .text("packageId", cx)
    });
    assert_eq!(next, "package-3");
}

#[test]
fn editor_problems_cover_plan_level_rules() {
    use super::intake::{PackageDraft, editor_problems};
    let package = |id: &str, deps: &[&str], criteria: &str| PackageDraft {
        id: id.into(),
        name: "Work".into(),
        depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
        outputs: vec!["Out".into()],
        criteria: criteria.into(),
        validation: vec!["Check".into()],
    };
    let messages = |goal: &str, packages: &[PackageDraft]| {
        editor_problems(goal, &["Scope".into()], packages, 2)
            .into_iter()
            .map(|p| p.message)
            .collect::<Vec<_>>()
    };
    assert!(messages("Goal", &[package("a", &[], "1"), package("b", &["a"], "2")]).is_empty());
    assert_eq!(
        messages("", &[package("a", &[], "1, 2")]),
        ["Goal: add a goal."]
    );
    assert_eq!(
        messages("Goal", &[package("a", &[], "1")]),
        ["Criterion 2 is not covered by any work package."]
    );
    assert_eq!(
        messages("Goal", &[package("a", &[], "1"), package("a", &[], "2")]),
        ["Work package 2 · Package ID: a is already used by work package 1."]
    );
    assert_eq!(
        messages("Goal", &[package("a", &["zz"], "1, x, 1, 2")]),
        [
            "Work package 1 · Depends on: no work package has ID zz.",
            "Work package 1 · Criterion numbers: x is not a number.",
            "Work package 1 · Criterion 1 is listed twice.",
        ]
    );
    assert_eq!(
        messages(
            "Goal",
            &[package("a", &["b"], "1"), package("b", &["a"], "2")]
        ),
        ["Work package dependencies form a cycle."]
    );
}
