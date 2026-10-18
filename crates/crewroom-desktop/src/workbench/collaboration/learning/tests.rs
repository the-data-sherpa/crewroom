//! Learning decisions over the real service. Fixture proposals are operator
//! records; no coordinator, model or process delivery is claimed.
use super::super::super::test_support::{Fixture, click, navigate, open, wait};
use super::*;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;

fn labels(cx: &mut gpui::TestAppContext, h: gpui::AnyWindowHandle) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}
fn reviewer(fixture: &Fixture) -> Value {
    fixture.get("/workspace")["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "reviewer")
        .unwrap()
        .clone()
}

#[test]
fn line_changes_mark_only_appended_lines_as_added() {
    let changes = line_changes(
        "Review the diff.\nCite evidence.",
        "Review the diff.\nCite evidence.\n\nName the revision.",
    );
    assert_eq!(
        changes,
        vec![
            LineChange::Unchanged(2),
            LineChange::Added("Name the revision.".into())
        ]
    );
    assert_eq!(
        line_changes("Keep.\nOld rule.\nTail.", "Keep.\nNew rule.\nTail."),
        vec![
            LineChange::Unchanged(1),
            LineChange::Removed("Old rule.".into()),
            LineChange::Added("New rule.".into()),
            LineChange::Unchanged(1),
        ]
    );
}

#[gpui::test]
fn native_teach_proposal_shows_appended_diff_and_flags_stale_before_any_click(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let role = reviewer(&fixture);
    let guidance = "Name the candidate revision in every verdict.";
    let (status, proposal) = fixture.post(
        "/learning",
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"roleId":"reviewer","revision":role["revision"].as_u64().unwrap_or(1),"mode":"teach","change":format!("{}\n\n{guidance}",string(&role,"instructions")),"rationale":"Operator correction"}),
    );
    assert_eq!(status, 200, "{proposal}");
    let pid = string(&proposal, "id");
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "learning").len() == 1
    });
    navigate(cx, h, "learning");
    let rendered = labels(cx, h);
    let added = rendered
        .iter()
        .filter(|l| l.starts_with("Added: "))
        .collect::<Vec<_>>();
    assert_eq!(added, vec![&format!("Added: {guidance}")], "{rendered:#?}");
    assert!(!rendered.iter().any(|l| l.starts_with("Removed: ")));
    assert!(
        !rendered.iter().any(|l| l == "Current version"),
        "The full current text is replaced by the diff"
    );
    assert!(!rendered.iter().any(|l| l.starts_with("Stale:")));
    let approve = format!("approve-learning-{pid}");
    let key = format!("/learning/{pid}/decision");
    cx.update_window(h, |_, window, cx| {
        let fields = &v.read(cx).collaboration.forms[&key].fields;
        if let Some(input) = fields.inputs.get("reason") {
            input
                .clone()
                .update(cx, |input, cx| input.focus(window, cx));
        } else {
            fields.areas["reason"]
                .clone()
                .update(cx, |input, cx| input.focus(window, cx));
        }
        window.input("Reviewed the appended line.", cx);
    })
    .unwrap();

    // A role save outside this proposal moves the role past its base revision.
    let mut edited = reviewer(&fixture);
    edited["instructions"] = json!(format!(
        "{}\nPrefer small diffs.",
        string(&edited, "instructions")
    ));
    edited["revision"] = json!(edited["revision"].as_u64().unwrap_or(1));
    let (status, saved) = fixture.post("/roles/reviewer", edited);
    assert_eq!(status, 200, "{saved}");
    let current = reviewer(&fixture)["revision"].as_u64().unwrap();
    assert_eq!(current, 2);
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "roles")
            .iter()
            .any(|r| r["id"] == "reviewer" && r["revision"] == current)
    });
    let rendered = labels(cx, h);
    assert!(
        rendered.contains(&format!(
            "Stale: role is now revision {current}. This proposal can't be applied."
        )),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|l| l == "Submit a new proposal from Agent roles.")
    );
    assert!(!rendered.iter().any(|l| l.contains("Ask the coordinator")));
    // Snapshots expose no button disabled flag; a disabled Approve is inert.
    click(cx, h, SharedString::from(approve));
    cx.read(|cx| {
        let v = v.read(cx);
        assert!(!v.busy && v.error.is_empty() && !v.pending_commands.contains(&key));
    });
    assert_eq!(
        fixture.get("/workspace")["learning"][0]["status"],
        "pending"
    );
}
