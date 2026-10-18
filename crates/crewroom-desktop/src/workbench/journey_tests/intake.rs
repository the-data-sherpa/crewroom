//! One native import/refine/refresh/queue/steering sequence across an OS daemon restart.
use super::*;
use crate::workbench::test_support::open_connection;

#[gpui::test]
#[ignore = "Requires the isolated daemon plan created by test-native-resume.sh."]
fn native_external_intake_restart(cx: &mut gpui::TestAppContext) {
    let path = std::env::var_os("CREWROOM_NATIVE_JOURNEY_PLAN").unwrap();
    let plan: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let (h, v, e) = open_connection(cx, PathBuf::from(plan["descriptor"].as_str().unwrap()));
    wait(cx, h, &v, &e, |v, _| v.connected);
    let imported = |description| {
        json!({"siteUrl":"https://fixture.atlassian.net","issues":[{"id":"100","key":"SIM-1","fields":{"summary":"Fix addition","description":description,"status":{"name":"Backlog"}}}]}).to_string()
    };
    if plan["action"] == "intake" {
        navigate(cx, h, "import-jira");
        cx.update_window(h, |_, window, cx| {
            let input = v.read(cx).import.clone();
            input.update(cx, |i, cx| i.focus(window, cx));
            window.input(&imported("Original source"), cx);
        })
        .unwrap();
        click(cx, h, "save-import");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "tasks")
                    .iter()
                    .any(|t| t["key"] == "SIM-1")
        });
        let task = cx.read(|cx| {
            records(&v.read(cx).workspace, "tasks")
                .into_iter()
                .find(|t| t["key"] == "SIM-1")
                .unwrap()
        });
        click(cx, h, SharedString::from(format!("ticket-{}", id(&task))));
        click(
            cx,
            h,
            SharedString::from(format!("assign-{}", plan["repositoryId"].as_str().unwrap())),
        );
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "tasks")
                    .iter()
                    .any(|t| t["id"] == task["id"] && t["repositoryId"] == plan["repositoryId"])
        });
        cx.update_window(h, |_, window, cx| {
            let draft = &v.read(cx).drafts[&id(&task)];
            let brief = draft.brief.clone();
            let criteria = draft.criteria.clone();
            brief.update(cx, |i, cx| i.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input("Keep zero behavior correct", cx);
            criteria.update(cx, |i, cx| i.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input("0 + 0 returns 0", cx);
        })
        .unwrap();
        click(cx, h, "save-brief");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "tasks")
                    .iter()
                    .any(|t| t["id"] == task["id"] && t["brief"] == "Keep zero behavior correct")
        });
        navigate(cx, h, "import-jira");
        cx.update_window(h, |_, window, cx| {
            let input = v.read(cx).import.clone();
            input.update(cx, |i, cx| i.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input(&imported("Refreshed source"), cx);
        })
        .unwrap();
        click(cx, h, "save-import");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "tasks")
                    .iter()
                    .any(|t| t["id"] == task["id"] && t["sourceRevision"] == 2)
        });
        click(cx, h, SharedString::from(format!("ticket-{}", id(&task))));
        click(cx, h, "queue-plan");
        cx.read(|cx| {
            assert!(
                !records(&v.read(cx).workspace, "runs")
                    .iter()
                    .any(|r| r["taskId"] == task["id"]),
                "Refreshed requirements must be accepted before queueing"
            )
        });
        click(cx, h, "save-brief");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "tasks")
                    .iter()
                    .any(|t| t["id"] == task["id"] && t["needsReacceptance"] == false)
        });
        click(cx, h, "queue-plan");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "runs")
                    .iter()
                    .any(|r| r["taskId"] == task["id"])
        });
        wait(cx, h, &v, &e, |v, _| v.observation.runtime_fresh);
        let run = cx.read(|cx| v.read(cx).observation.runtime["run"].clone());
        navigate(cx, h, "Conversation");
        cx.update_window(h, |_, window, cx| {
            let input = v.read(cx).steering[&id(&run)].clone();
            input.update(cx, |i, cx| i.focus(window, cx));
            window.input("Keep the patch small.", cx);
            window.press("cmd-enter", cx);
        })
        .unwrap();
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && records(&v.workspace, "messages")
                    .iter()
                    .any(|m| m["runId"] == run["id"])
        });
    } else {
        assert_eq!(plan["action"], "intake-reopen");
        select(cx, h, &v, &e, &json!({"id":plan["taskId"]}));
    }
    let task = cx.read(|cx| {
        records(&v.read(cx).workspace, "tasks")
            .into_iter()
            .find(|t| t["key"] == "SIM-1")
            .unwrap()
    });
    let run = cx.read(|cx| {
        records(&v.read(cx).workspace, "runs")
            .into_iter()
            .find(|r| r["taskId"] == task["id"])
            .unwrap()
    });
    assert_eq!(task["description"], "Refreshed source");
    assert_eq!(task["brief"], "Keep zero behavior correct");
    assert_eq!(task["jiraStatus"], "Backlog");
    assert_eq!(task["criteria"], json!(["0 + 0 returns 0"]));
    assert_eq!(run["status"], "awaiting_runner");
    cx.read(|cx| assert!(records(&v.read(cx).observation.runtime, "attempts").is_empty()));
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    navigate(cx, h, "Conversation");
    let labels = cx
        .update_window(h, |_, window, cx| {
            window.render_frame(cx);
            let b = window.find("save-steering").bounds();
            assert!(b.origin.x >= px(0.) && b.right() <= px(390.));
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter_map(|n| n.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(
        labels.iter().any(|l| l.contains("Keep the patch small.")),
        "{labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|l| l.contains("Stored · Receipt unconfirmed")),
        "{labels:?}"
    );
    let result=cx.read(|cx|json!({"error":v.read(cx).error,"task":task,"run":run,"workspace":v.read(cx).workspace,"runtime":v.read(cx).observation.runtime,"labels":labels}));
    assert_eq!(result["error"], "");
    std::fs::write(
        plan["resultPath"].as_str().unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
