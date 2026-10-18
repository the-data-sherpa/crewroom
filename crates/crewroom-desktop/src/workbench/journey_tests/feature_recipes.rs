//! Narrow FOR-12 GPUI actions against a caller-owned production daemon.
//! Setup and provider scripting live in feature-recipes.test.ts; this driver
//! records rendered native labels alongside the observed durable projection.
use super::*;
use crate::workbench::test_support::{open_connection, wait_for};
use std::time::Duration;

fn labels(cx: &mut gpui::TestAppContext, h: gpui::AnyWindowHandle) -> Vec<String> {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|n| n.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}

fn contains_label(labels: &[String], expected: &str) {
    assert!(
        labels.iter().any(|label| label.contains(expected)),
        "Expected rendered label {expected:?}: {labels:?}"
    );
}

#[gpui::test]
#[ignore = "Requires the private FOR-12 fixture plan; use scripts/verify-feature.py."]
fn native_feature_recipe_control(cx: &mut gpui::TestAppContext) {
    let path = std::env::var_os("CREWROOM_NATIVE_JOURNEY_PLAN").expect("Missing fixture plan");
    let plan: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let (h, v, e) = open_connection(cx, PathBuf::from(plan["descriptor"].as_str().unwrap()));
    wait(cx, h, &v, &e, |v, _| {
        v.connected
            && records(&v.workspace, "tasks")
                .iter()
                .any(|task| task["id"] == plan["taskId"])
    });
    let ticket = format!("ticket-{}", plan["taskId"].as_str().unwrap());
    click(cx, h, SharedString::from(ticket.clone()));
    wait(cx, h, &v, &e, |v, _| {
        v.connected
            && v.observation.runtime_fresh
            && v.observation.result_fresh
            && v.observation.runtime["run"]["id"] == plan["runId"]
    });
    let before = cx.read(|cx| {
        let v = v.read(cx);
        json!({"runtime":v.observation.runtime,"result":v.observation.result})
    });
    let mut trace = vec![json!({"control":ticket,"action":"click","connected":true})];
    let mut intermediate = Value::Null;
    match plan["kind"].as_str().unwrap() {
        "store-steering" | "inspect-steering" => {
            navigate(cx, h, "Conversation");
            trace.push(json!({"control":"Conversation","action":"navigate"}));
            if plan["kind"] == "store-steering" {
                let run = plan["runId"].as_str().unwrap();
                cx.update_window(h, |_, window, cx| {
                    let input = v.read(cx).steering[run].clone();
                    input.update(cx, |input, cx| input.focus(window, cx));
                    window.input(plan["body"].as_str().unwrap(), cx);
                })
                .unwrap();
                trace
                    .push(json!({"control":"steering-input","action":"input","body":plan["body"]}));
                click(cx, h, "save-steering");
                trace.push(json!({"control":"save-steering","action":"click"}));
            }
            wait(cx, h, &v, &e, |v, _| {
                !v.busy
                    && records(&v.workspace, "messages").iter().any(|message| {
                        message["runId"] == plan["runId"]
                            && message["body"] == plan["body"]
                            && message["delivery"] == plan["delivery"]
                            && (plan["messageId"].is_null() || message["id"] == plan["messageId"])
                    })
            });
            let shown = labels(cx, h);
            contains_label(&shown, plan["body"].as_str().unwrap());
            contains_label(
                &shown,
                match plan["delivery"].as_str().unwrap() {
                    "pending" => "Stored · Receipt unconfirmed",
                    "received" => "Receipt confirmed",
                    "applied" => "Applied to plan",
                    other => panic!("Unsupported recipe delivery state {other}"),
                },
            );
        }
        "pause" => {
            click(cx, h, "pause-run");
            trace.push(json!({"control":"pause-run","action":"click"}));
            wait_for(cx, h, &v, &e, Duration::from_secs(50), |v, _| {
                v.observation.runtime["run"]["status"] == "pausing"
                    && records(&v.observation.runtime, "attempts")
                        .iter()
                        .any(|attempt| attempt["ownership"] != "stopped")
            });
            let shown = labels(cx, h);
            contains_label(&shown, "Pausing");
            intermediate =
                cx.read(|cx| json!({"runtime":v.read(cx).observation.runtime,"labels":shown}));
            wait_for(cx, h, &v, &e, Duration::from_secs(50), |v, _| {
                !v.busy
                    && v.observation.runtime["run"]["status"] == "paused"
                    && records(&v.observation.runtime, "attempts")
                        .iter()
                        .all(|attempt| attempt["ownership"] == "stopped")
            });
        }
        "resume" => {
            assert_eq!(before["runtime"]["run"]["status"], "paused");
            click(cx, h, "resume-run");
            trace.push(json!({"control":"resume-run","action":"click"}));
            wait_for(cx, h, &v, &e, Duration::from_secs(50), |v, _| {
                !v.busy && v.observation.runtime["run"]["status"] == "running"
            });
        }
        "history" => {
            navigate(cx, h, "History");
            trace.push(json!({"control":"History","action":"navigate"}));
            let attempt = plan["attemptId"].as_str().unwrap();
            let control = format!("history-attempt-{attempt}");
            click(cx, h, SharedString::from(control.clone()));
            trace.push(json!({"control":control,"action":"click"}));
            wait(cx, h, &v, &e, |v, _| {
                v.observation.selected_attempt.as_deref() == Some(attempt)
            });
            intermediate = json!({"labels":labels(cx,h),"selectedAttempt":attempt});
            click(cx, h, "back-to-current-run");
            trace.push(json!({"control":"back-to-current-run","action":"click"}));
            wait(cx, h, &v, &e, |v, _| {
                v.observation.selected_attempt.is_none()
                    && v.observation.runtime["run"]["id"] == plan["runId"]
            });
        }
        "results" | "target-refresh" => {
            navigate(cx, h, "Results");
            trace.push(json!({"control":"Results","action":"navigate"}));
            if plan["kind"] == "target-refresh" {
                assert_eq!(before["runtime"]["run"]["status"], "paused");
                click(cx, h, "target-refresh");
                trace.push(json!({"control":"target-refresh","action":"click"}));
            }
            wait(cx, h, &v, &e, |v, _| {
                !v.busy
                    && v.observation.result_fresh
                    && v.observation.result["assessment"]["ready"] == plan["ready"]
                    && (plan["kind"] != "target-refresh"
                        || records(&v.observation.result, "candidates").len() == 2
                            && records(&v.observation.result, "refreshes")
                                .iter()
                                .any(|refresh| refresh["state"] == "completed"))
            });
            let shown = labels(cx, h);
            contains_label(
                &shown,
                if plan["ready"] == true {
                    "Locally validated"
                } else {
                    "Validation incomplete"
                },
            );
            if plan["historicalCandidateId"].is_string() {
                contains_label(&shown, "Not run");
                contains_label(&shown, "No independent review recorded for this revision.");
                intermediate = json!({"currentCandidateLabels":shown});
                let control = format!(
                    "candidate-{}",
                    plan["historicalCandidateId"].as_str().unwrap()
                );
                click(cx, h, SharedString::from(control.clone()));
                trace.push(json!({"control":control,"action":"click"}));
                let shown = labels(cx, h);
                contains_label(&shown, "Required · Passed");
                contains_label(&shown, "Independent review passed");
                // Old success remains visibly historical while readiness stays false.
                contains_label(&shown, "Validation incomplete");
            }
        }
        other => panic!("Unknown feature-recipe native action {other}"),
    }
    let shown = labels(cx, h);
    let result = cx.read(|cx| {
        let v = v.read(cx);
        json!({"action":plan["action"],"kind":plan["kind"],"error":v.error,
            "connected":v.connected,"runtimeFresh":v.observation.runtime_fresh,
            "resultFresh":v.observation.result_fresh,"before":before,
            "runtime":v.observation.runtime,"result":v.observation.result,
            "messages":records(&v.workspace,"messages"),"intermediate":intermediate,
            "trace":trace,"labels":shown})
    });
    assert_eq!(result["error"], "");
    std::fs::write(
        plan["resultPath"].as_str().unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
