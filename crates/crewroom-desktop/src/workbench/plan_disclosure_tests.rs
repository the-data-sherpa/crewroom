//! Plan acceptance and requirement edits disclose their side effects before commitment (FOR-26).
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
fn absent(labels: &[String], unexpected: &str) {
    assert!(
        !labels.iter().any(|l| l.contains(unexpected)),
        "Unexpected {unexpected:?} in {labels:#?}"
    );
}
fn task(f: &Fixture) -> Value {
    f.get("/workspace")["tasks"][0].clone()
}
/// One imported ticket with an accepted brief, a target repository and a pinned plan source.
fn ticket(f: &Fixture, repositories: usize) -> (String, Vec<Value>) {
    let repos = (0..repositories)
        .map(|i| {
            let path = f.root.path().join(format!("checkout-{i}"));
            checkout(&path);
            saved(
                f,
                "/repositories",
                json!({"name":format!("Checkout {i}"),"path":path,"targetBranch":"main"}),
            )
        })
        .collect::<Vec<_>>();
    saved(
        f,
        "/import/jira",
        json!({"siteUrl":"https://example.atlassian.net","issues":[{"id":"1","key":"FOR-1","fields":{"summary":"Disclose","description":"Source","status":{"name":"To Do"},"updated":"2026-10-04T12:00:00.000+0000"}}]}),
    );
    let t = task(f);
    let id = string(&t, "id");
    saved(
        f,
        &format!("/tasks/{id}/repository"),
        json!({"repositoryId":repos[0]["id"],"revision":t["revision"]}),
    );
    let t = task(f);
    saved(
        f,
        &format!("/tasks/{id}/refinement"),
        json!({"revision":t["revision"],"sourceRevision":t["sourceRevision"],"brief":"Keep behavior","criteria":["Retain evidence"]}),
    );
    (id, repos)
}
fn plan(f: &Fixture, id: &str, requirements: Option<Value>) -> (Value, Value) {
    let t = task(f);
    let source = saved(
        f,
        &format!("/tasks/{id}/plan-source"),
        json!({"revision":t["revision"]}),
    );
    let covered = requirements
        .as_ref()
        .map_or(1, |r| r["criteria"].as_array().unwrap().len());
    let mut body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":t["revision"],"sourceId":source["id"],"content":{"schemaVersion":1,"goal":"Disclosure fixture","scope":["Keep behavior"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"fixture","name":"Fixture work","dependsOn":[],"outputs":["Evidence"],"criteria":(0..covered).collect::<Vec<_>>(),"validation":["Native fixture"]}]}});
    if let Some(r) = requirements {
        body["requirements"] = r;
    }
    if let Some(previous) = records(&f.get("/workspace")["intake"][id], "plans").last() {
        body["predecessorId"] = previous["id"].clone();
    }
    let plan = saved(f, &format!("/tasks/{id}/implementation-plans"), body);
    (plan, source)
}
fn decide(f: &Fixture, plan: &Value, source: &Value, decision: &str) {
    let mut body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":task(f)["revision"],"sourceId":source["id"],"decision":decision});
    if decision == "reject" {
        body["reason"] = json!("Narrow the brief");
    }
    saved(
        f,
        &format!("/implementation-plans/{}/decision", string(plan, "id")),
        body,
    );
}
fn show(
    cx: &mut gpui::TestAppContext,
    f: &Fixture,
    id: &str,
) -> (
    AnyWindowHandle,
    Entity<Workbench>,
    async_channel::Receiver<Event>,
) {
    let (h, v, events) = open(cx, f);
    let id_ = id.to_owned();
    wait(cx, h, &v, &events, move |v, _| {
        v.connected && v.workspace["intake"][&id_].is_object()
    });
    click(cx, h, SharedString::from(format!("ticket-{id}")));
    (h, v, events)
}

#[gpui::test]
fn plan_with_requirements_discloses_the_brief_replacement_beside_accept(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (id, _) = ticket(&fixture, 1);
    let revision = task(&fixture)["revision"].as_u64().unwrap();
    let (first, source) = plan(
        &fixture,
        &id,
        Some(json!({"brief":"Narrowed behavior","criteria":["Keep evidence","Name the revision"]})),
    );
    let (h, v, events) = show(cx, &fixture, &id);
    let shown = labels(cx, h);
    contains(
        &shown,
        &format!(
            "Accepting also replaces your accepted brief and 1 criterion with this plan's proposed requirements (revision {revision} → {}).",
            revision + 1
        ),
    );
    contains(&shown, "Current requirements");
    contains(&shown, "Keep behavior");
    contains(&shown, "Retain evidence");
    contains(&shown, "Proposed requirements");
    contains(&shown, "Narrowed behavior");
    contains(&shown, "Keep evidence\nName the revision");
    absent(&shown, "adopted");
    drop((v, events));

    // A rejected plan, now in history, still reads as a proposal.
    decide(&fixture, &first, &source, "reject");
    let (second, source) = plan(
        &fixture,
        &id,
        Some(json!({"brief":"Accepted behavior","criteria":["One"]})),
    );
    decide(&fixture, &second, &source, "accept");
    let (h, _v, _events) = show(cx, &fixture, &id);
    let shown = labels(cx, h);
    contains(&shown, "Plan 1 · Rejected");
    contains(&shown, "Proposed requirements");
    contains(&shown, "Requirements adopted with acceptance");
    absent(&shown, "Accepting also replaces");
}

#[gpui::test]
fn plan_without_requirements_keeps_the_existing_accept_hint(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (id, _) = ticket(&fixture, 1);
    plan(&fixture, &id, None);
    let (h, _v, _events) = show(cx, &fixture, &id);
    let shown = labels(cx, h);
    contains(
        &shown,
        "Accepting saves the plan. Queue remains a separate action.",
    );
    absent(&shown, "Accepting also replaces");
    absent(&shown, "Proposed requirements");
}

#[gpui::test]
fn editing_an_accepted_brief_or_target_warns_before_setting_the_plan_aside(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (id, _) = ticket(&fixture, 2);
    let set_aside = "Saving changed requirements sets accepted plan 1 aside. Queue will start scoping again unless you accept a new plan version.";
    let retarget = "Using another repository sets accepted plan 1 aside. Queue will start scoping again unless you accept a new plan version.";
    {
        // No accepted plan yet: neither control has a plan to set aside.
        let (h, v, _events) = show(cx, &fixture, &id);
        let shown = labels(cx, h);
        absent(&shown, "sets accepted plan");
        drop(v);
    }
    fixture.accept_plan(&id);
    let (h, v, events) = show(cx, &fixture, &id);
    let shown = labels(cx, h);
    contains(&shown, retarget);
    absent(&shown, set_aside);
    cx.update_window(h, |_, window, cx| {
        let brief = v.read(cx).drafts[&id].brief.clone();
        brief.update(cx, |i, cx| i.focus(window, cx));
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.input("Reworded behavior", cx);
    })
    .unwrap();
    contains(&labels(cx, h), set_aside);
    // The warning precedes the change; afterwards the panel reports the held plan.
    click(cx, h, "save-brief");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.workspace, "tasks")[0]["brief"] == "Reworded behavior"
    });
    let shown = labels(cx, h);
    absent(&shown, set_aside);
    absent(&shown, retarget);
}
