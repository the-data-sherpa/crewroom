//! Native controls against a separately running production daemon. The driver
//! provides scripted model/Mail services and owns every disposable process.
use super::*;
use crate::workbench::test_support::{click, navigate, open_connection, wait, wait_for};
use gpui::test::TestWindowExt;
use gpui_kit as gpui;
use std::{path::PathBuf, time::Duration};

fn edit_text(cx: &mut gpui::TestAppContext, h: gpui::AnyWindowHandle, previous: &str, text: &str) {
    // Textarea's accessibility ID is not its GPUI ElementId. Observe the actual
    // input carrying the known value, then use its registered pointer target.
    let id = cx
        .update_window(h, |_, window, cx| {
            window.render_frame(cx);
            let fields = gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter(|node| node.value() == Some(previous))
                .collect::<Vec<_>>();
            assert_eq!(
                fields.len(),
                1,
                "Expected one input with the current draft value"
            );
            fields[0].path().last().unwrap().clone()
        })
        .unwrap();
    click(cx, h, id);
    cx.update_window(h, |_, window, cx| {
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.press("right", cx);
        // GPUI's test input renders every typed character. Exercise the real
        // append keystrokes without retyping a long unchanged role catalog.
        window.input(text.strip_prefix(previous).unwrap(), cx);
        window.render_frame(cx);
    })
    .unwrap();
}
fn selected(v: &Workbench) -> Value {
    v.comparisons.selected.clone().unwrap_or(Value::Null)
}
fn stopped(v: &Value) -> bool {
    records(v, "cells").iter().all(|cell| {
        !cell["runId"].is_string()
            || records(cell, "attempts")
                .iter()
                .all(|attempt| attempt["state"] == "admitted" || attempt["ownership"] == "stopped")
    })
}

#[gpui::test]
#[ignore = "Requires the private production-daemon plan from for14-native.test.ts"]
fn native_external_instruction_comparison(cx: &mut gpui::TestAppContext) {
    let path = std::env::var_os("CREWROOM_NATIVE_JOURNEY_PLAN").unwrap();
    let plan: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let kind = plan["kind"].as_str().unwrap();
    let stage = |message: &str| eprintln!("FOR-14 native {kind}: {message}");
    stage("connecting");
    let target_id = plan["targetId"].as_str().unwrap();
    let catalog = match kind {
        "role" => "roles",
        "principle" => "principles",
        "playbook" => "playbooks",
        _ => panic!("Unsupported test target"),
    };
    let field = match kind {
        "role" => "instructions",
        "principle" => "text",
        _ => "description",
    };
    let (h, v, e) = open_connection(cx, PathBuf::from(plan["descriptor"].as_str().unwrap()));
    wait(cx, h, &v, &e, |v, _| v.connected);
    let initial = cx.read(|cx| {
        records(&v.read(cx).workspace, catalog)
            .into_iter()
            .find(|r| r["id"] == target_id)
            .unwrap()
    });
    let base = initial["revision"].as_u64().unwrap_or(1);
    let proposed = format!(
        "{}\nRetain the observed verification limits.",
        initial[field].as_str().unwrap()
    );
    stage("editing exact draft");
    navigate(cx, h, catalog);
    click(
        cx,
        h,
        SharedString::from(format!("edit-{kind}-{target_id}")),
    );
    edit_text(cx, h, initial[field].as_str().unwrap(), &proposed);
    cx.read(|cx| {
        assert_eq!(
            v.read(cx)
                .settings
                .comparison_draft(kind, target_id, cx)
                .unwrap()[field],
            proposed
        );
    });
    click(cx, h, SharedString::from(format!("compare-{kind}")));
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.comparisons.preflight.is_some()
    });
    stage("preflight received");
    let preflight = cx.read(|cx| v.read(cx).comparisons.preflight.clone().unwrap());
    assert_eq!(preflight["canStart"], true, "{preflight}");
    assert_eq!(preflight["manifest"]["mode"], "scripted");
    assert_eq!(preflight["manifest"]["target"]["proposed"][field], proposed);
    cx.read(|cx| {
        let view = v.read(cx);
        assert!(
            view.comparisons.selected.is_none(),
            "Preflight must not create or start a comparison"
        );
        assert_eq!(
            records(&view.workspace, catalog)
                .into_iter()
                .find(|r| r["id"] == target_id)
                .unwrap(),
            initial
        );
    });
    click(cx, h, "start-comparison");
    wait_for(cx, h, &v, &e, Duration::from_secs(45), |v, _| {
        !v.busy && selected(v)["desiredState"] == "running"
    });
    if kind == "role" {
        wait_for(cx, h, &v, &e, Duration::from_secs(45), |v, _| {
            records(&selected(v), "cells").iter().any(|cell| {
                let attempts = records(cell, "attempts");
                attempts.len() == 2
                    && attempts
                        .iter()
                        .all(|a| a["state"] == "idle" && a["ownership"] == "held")
            })
        });
    }
    stage("comparison running");
    let started = cx.read(|cx| selected(v.read(cx)));
    let comparison_id = string(&started, "id");
    assert_eq!(records(&started, "cells").len(), 16);
    assert!(started["adoption"].is_null());
    click(cx, h, "comparison-pause");
    wait_for(cx, h, &v, &e, Duration::from_secs(45), |v, _| {
        !v.busy && selected(v)["state"] == "paused" && stopped(&selected(v))
    });
    stage("paused and stopped");
    let paused = cx.read(|cx| selected(v.read(cx)));
    click(cx, h, "comparison-cancel");
    click(cx, h, "ok");
    wait_for(cx, h, &v, &e, Duration::from_secs(45), |v, _| {
        let value = selected(v);
        !v.busy
            && value["state"] == "cancelled"
            && stopped(&value)
            && records(&value, "cells")
                .iter()
                .all(|cell| !cell["runId"].is_string() || cell["result"].is_object())
    });
    stage("cancelled and finalized");
    let cancelled = cx.read(|cx| selected(v.read(cx)));
    click(cx, h, "close-comparison");
    click(cx, h, SharedString::from(format!("save-{kind}")));
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && records(&v.workspace, catalog)
                .iter()
                .any(|r| r["id"] == target_id && r["revision"] == base + 1)
            && v.comparisons
                .history
                .iter()
                .any(|c| c["id"] == comparison_id && c["adoption"]["adoptedRevision"] == base + 1)
    });
    click(
        cx,
        h,
        SharedString::from(format!("comparison-history-{kind}")),
    );
    wait(cx, h, &v, &e, |v, _| {
        v.comparisons
            .history
            .iter()
            .any(|c| c["id"] == comparison_id && c["adoption"].is_object())
    });
    click(
        cx,
        h,
        SharedString::from(format!("open-comparison-{comparison_id}")),
    );
    stage("exact draft adopted");
    let adopted = cx.read(|cx| selected(v.read(cx)));
    assert_eq!(adopted["manifestHash"], started["manifestHash"]);
    assert_eq!(adopted["manifest"], started["manifest"]);
    assert_eq!(
        adopted["adoption"]["proposedHash"],
        preflight["manifest"]["target"]["proposedHash"]
    );
    assert_eq!(
        adopted["coverage"]["observed"], 0,
        "Cancelled scripted work supplies no behavioral acceptance"
    );
    click(cx, h, "close-comparison");
    let ordinary = format!("{proposed}\nAn ordinary save remains optional and available.");
    edit_text(cx, h, &proposed, &ordinary);
    click(cx, h, SharedString::from(format!("save-{kind}")));
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && records(&v.workspace, catalog)
                .iter()
                .any(|r| r["id"] == target_id && r["revision"] == base + 2 && r[field] == ordinary)
    });
    click(
        cx,
        h,
        SharedString::from(format!("comparison-history-{kind}")),
    );
    wait(cx, h, &v, &e, |v, _| v.comparisons.pending_reads.is_empty());
    click(
        cx,
        h,
        SharedString::from(format!("open-comparison-{comparison_id}")),
    );
    stage("ordinary save preserves history");
    let retained = cx.read(|cx| selected(v.read(cx)));
    assert_eq!(retained["manifest"], started["manifest"]);
    assert_eq!(retained["adoption"], adopted["adoption"]);
    assert_eq!(retained["cells"], adopted["cells"]);
    let labels = cx
        .update_window(h, |_, window, cx| {
            window.render_frame(cx);
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter_map(|node| node.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(
        labels
            .iter()
            .any(|label| label.contains("Adoption association"))
    );
    let result = cx.read(|cx| json!({"error":v.read(cx).error,"comparisonError":v.read(cx).comparisons.error,"kind":kind,"initial":initial,"preflight":preflight,"started":started,"paused":paused,"cancelled":cancelled,"adopted":adopted,"retained":retained,"workspace":v.read(cx).workspace,"labels":labels}));
    assert_eq!(result["error"], "");
    assert_eq!(result["comparisonError"], "");
    std::fs::write(
        plan["resultPath"].as_str().unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
