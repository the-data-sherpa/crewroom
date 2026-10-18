//! Production-service control driver used by the isolated process acceptance gate.
use super::*;
use crate::workbench::test_support::{activate_by_keyboard, open_connection, wait_for};
use std::{cell::Cell, time::Duration};

#[gpui::test]
#[ignore = "Requires the private disposable plan created by test-native-resume.sh."]
fn native_external_operator_control(cx: &mut gpui::TestAppContext) {
    let path =
        std::env::var_os("CREWROOM_NATIVE_JOURNEY_PLAN").expect("Missing isolated fixture plan");
    let plan: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let (h, v, e) = open_connection(cx, PathBuf::from(plan["descriptor"].as_str().unwrap()));
    wait(cx, h, &v, &e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(
        cx,
        h,
        SharedString::from(format!("ticket-{}", plan["taskId"].as_str().unwrap())),
    );
    if plan["action"] != "queue" {
        wait(cx, h, &v, &e, |v, _| {
            v.observation.runtime_fresh && v.observation.result_fresh
        });
    }
    let saw_pausing = Cell::new(false);
    match plan["action"].as_str().unwrap() {
        "queue" => {
            navigate(cx, h, "Brief");
            click(cx, h, "queue-plan");
            wait(cx, h, &v, &e, |v, _| {
                !v.busy
                    && records(&v.workspace, "runs")
                        .iter()
                        .any(|r| r["taskId"] == plan["taskId"] && r["launch"].is_object())
            });
        }
        "insert" => {
            navigate(cx, h, "specialist-picker");
            activate_by_keyboard(cx, h, "specialist-role-tech-writer");
            let key = format!("/runs/{}/specialists", plan["runId"].as_str().unwrap());
            cx.update_window(h, |_, window, cx| {
                let input = v.read(cx).collaboration.forms[&key].fields.areas["input"].clone();
                input.update(cx, |input, cx| input.focus(window, cx));
                window.input(
                    "Investigate the pending insertion while current work is busy.",
                    cx,
                );
            })
            .unwrap();
            activate_by_keyboard(cx, h, "specialist-criterion:0");
            activate_by_keyboard(cx, h, "propose-specialist");
            wait(cx, h, &v, &e, |v, _| {
                !v.busy
                    && records(&v.observation.runtime, "teamProposals")
                        .iter()
                        .any(|p| p["status"] == "pending")
            });
        }
        "pause" => {
            click(cx, h, "pause-run");
            wait_for(cx, h, &v, &e, Duration::from_secs(50), |v, _| {
                let runtime = &v.observation.runtime;
                if runtime["run"]["status"] == "pausing"
                    && records(runtime, "attempts")
                        .iter()
                        .any(|a| a["ownership"] != "stopped")
                {
                    saw_pausing.set(true);
                }
                !v.busy && runtime["run"]["status"] == "paused"
            });
            assert!(
                saw_pausing.get(),
                "UI must expose pausing before owned work is confirmed stopped"
            );
        }
        "resume" => {
            click(cx, h, "resume-run");
            wait_for(cx, h, &v, &e, Duration::from_secs(50), |v, _| {
                !v.busy && v.observation.runtime["run"]["status"] == "running"
            });
        }
        "pause-pair" => {
            click(cx, h, "pause-run");
            wait_for(cx, h, &v, &e, Duration::from_secs(50), |v, _| {
                !v.busy && v.observation.runtime["run"]["status"] == "paused"
            });
        }
        "packages" => {
            navigate(cx, h, "Conversation");
            if let Some(width) = plan["width"].as_f64() {
                cx.simulate_window_resize(h, gpui::size(px(width as f32), px(844.)));
            }
            let labels = shown_labels(cx, h);
            for expected in records(&plan, "labels").iter().filter_map(Value::as_str) {
                assert!(
                    labels.iter().any(|l| l.contains(expected)),
                    "{expected} missing: {labels:?}"
                );
            }
        }
        "inspect" => {
            navigate(cx, h, "Conversation");
            if let Some(status) = plan["status"].as_str() {
                wait(cx, h, &v, &e, |v, _| {
                    v.observation.runtime["run"]["status"] == status
                });
            }
        }
        "queue-inspect" => {
            if let Some(expected) = plan["runExplanation"].as_str() {
                let labels = shown_labels(cx, h);
                assert!(
                    labels.iter().any(|l| l.contains(expected)),
                    "{expected} missing: {labels:?}"
                );
            }
            click(cx, h, "queue");
            wait(cx, h, &v, &e, |v, _| {
                v.workspace["queue"]["slot"]["runId"] == plan["slotRunId"]
            });
            let labels = shown_labels(cx, h);
            if let Some(expected) = plan["holdText"].as_str() {
                assert!(
                    labels.iter().any(|l| l.contains(expected)),
                    "{expected} missing: {labels:?}"
                );
            }
            if let Some(id) = plan["waitingRunId"].as_str() {
                cx.read(|cx| {
                    assert!(
                        records(&v.read(cx).workspace["queue"], "plans")
                            .iter()
                            .any(|p| p["id"] == id)
                    )
                });
            }
        }
        "reconcile-merge" => {
            navigate(cx, h, "Results");
            click(cx, h, "reconcile-publication");
            wait(cx, h, &v, &e, |v, _| {
                !v.busy && v.observation.result["publication"]["record"]["merged"].is_object()
            });
            let labels = shown_labels(cx, h);
            assert!(labels.iter().any(|l| l.contains("Merged")), "{labels:?}");
        }
        action => panic!("Unknown native fixture action {action}"),
    }
    let result=cx.read(|cx|{
        let v=v.read(cx);
        json!({"action":plan["action"],"error":v.error,"runtime":v.observation.runtime,"workspace":v.workspace,"sawPausingWithUnstoppedOwnership":saw_pausing.get()})
    });
    assert_eq!(result["error"], "");
    std::fs::write(
        plan["resultPath"].as_str().unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}

fn shown_labels(cx: &mut gpui::TestAppContext, h: gpui::AnyWindowHandle) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|n| n.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}
