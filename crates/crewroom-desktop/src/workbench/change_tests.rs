//! What changed at Jira re-acceptance and requirement adoption (FOR-30, M11 and M13).
use super::test_support::{Fixture, accept_plan, click, navigate, open, wait};
use super::*;
use crewroom_core::store::Store;
use gpui::AnyWindowHandle;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;
use std::path::Path;

fn id(value: &Value) -> String {
    string(value, "id")
}
fn repository(store: &Store, root: &Path) -> Value {
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
    store
        .create_repository(&json!({"name":"Change checkout","path":path,"targetBranch":"main"}))
        .unwrap()
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
fn jira(title: &str, description: &str) -> Value {
    json!({"siteUrl":"https://fixture.atlassian.net","issues":[{"id":"1","key":"SIM-1","fields":{"summary":title,"description":description,"status":{"name":"To Do"}}}]})
}

#[test]
fn criteria_changes_name_removed_and_added_lines() {
    let captured = [json!("Keep A"), json!("Keep B"), json!("Drop C")];
    let accepted = [json!("Keep A"), json!("Keep B"), json!("Add D")];
    assert_eq!(
        collaboration::criteria_changes(&captured, &accepted),
        vec!["Removed criterion: Drop C", "Added criterion: Add D"]
    );
    assert_eq!(
        collaboration::criteria_changes(&captured[..2], &[json!("Keep B"), json!("Keep A")]),
        vec!["Criteria reordered."]
    );
    assert!(collaboration::criteria_changes(&captured, &captured).is_empty());
}

#[gpui::test]
fn criteria_only_change_on_a_queued_run_shows_the_criteria_difference(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let fixture = Fixture::seeded(false, |s, root| {
        let repo = repository(s, root);
        task = s
            .create_task(
                &json!({"title":"Adopt criteria","description":"","repositoryId":repo["id"]}),
            )
            .unwrap();
        task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Explain the work","criteria":["Explain the result","Keep evidence"]})).unwrap();
        accept_plan(s, &id(&task));
        run = s.queue(&id(&task), &json!({"revision":2})).unwrap();
    });
    let (status, value) = fixture.post(
        &format!("/tasks/{}/refinement", id(&task)),
        json!({"revision":2,"sourceRevision":1,"brief":"Explain the work","criteria":["Explain the result with sources","Keep evidence"]}),
    );
    assert_eq!(status, 200, "{value}");
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == task["id"] && t["revision"] == 3)
            && records(&v.workspace, "runs")
                .iter()
                .any(|r| r["id"] == run["id"])
    });
    click(cx, h, SharedString::from(format!("ticket-{}", id(&task))));
    navigate(cx, h, "Brief");
    let shown = labels(cx, h);
    contains(&shown, "Brief unchanged.");
    contains(&shown, "Removed criterion: Explain the result");
    contains(&shown, "Added criterion: Explain the result with sources");
    absent(&shown, "Captured brief:");
    absent(&shown, "criterion: Keep evidence");
}

#[gpui::test]
fn summary_only_jira_change_shows_previous_and_current_title_then_source_only_adoption(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let fixture = Fixture::seeded(false, |s, root| {
        let repo = repository(s, root);
        task = s
            .import_jira(&jira("Original title", "Add numbers"))
            .unwrap()[0]
            .clone();
        task = s
            .assign_repository(&id(&task), &json!({"repositoryId":repo["id"],"revision":1}))
            .unwrap();
        task=s.refine(&id(&task),&json!({"revision":task["revision"],"sourceRevision":1,"brief":"Handle zero","criteria":["0 + 0 = 0"]})).unwrap();
        accept_plan(s, &id(&task));
        run = s
            .queue(&id(&task), &json!({"revision":task["revision"]}))
            .unwrap();
        s.import_jira(&jira("Renamed title", "Add numbers"))
            .unwrap();
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == task["id"] && t["needsReacceptance"] == true)
    });
    click(cx, h, SharedString::from(format!("ticket-{}", id(&task))));
    navigate(cx, h, "Brief");
    let shown = labels(cx, h);
    contains(&shown, "Changed in Jira: title.");
    contains(&shown, "Previous title: Original title");
    contains(&shown, "Current title: Renamed title");
    absent(&shown, "Previous description:");
    absent(&shown, "Compare the source");

    click(cx, h, "save-brief");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == task["id"] && t["needsReacceptance"] == false)
    });
    let shown = labels(cx, h);
    absent(&shown, "Changed in Jira:");
    assert_eq!(
        fixture.get(&format!("/runs/{}/runtime", id(&run)))["run"]["taskSnapshot"]["sourceRevision"],
        1
    );
    contains(
        &shown,
        "Brief and criteria unchanged; Jira source revision 1 → 2.",
    );
}
