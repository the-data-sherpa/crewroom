//! Exact lifecycle rules from the accepted service. State changes do not prove process exit.
use crate::{Error, Result, timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Transition {
    Account {
        #[serde(default)]
        waiting: bool,
    },
    Start,
    BeginPause {
        reason: Option<String>,
    },
    FinishPause {
        #[serde(default)]
        unresolved: bool,
    },
    Desire {
        #[serde(rename = "desiredState")]
        desired_state: String,
    },
    Continue {
        #[serde(rename = "controlRevision")]
        control_revision: u64,
    },
    ReviewRegressed,
    ReviewRestored,
    Attention {
        reason: String,
    },
    SetLimit {
        #[serde(rename = "activeLimitMs")]
        active_limit_ms: u64,
    },
    AgentStart,
    SourceChanged {
        reason: String,
        active: bool,
    },
    Close,
    CompleteResearch,
}

fn remove(run: &mut Value, key: &str) {
    run.as_object_mut().expect("run object").remove(key);
}
fn has(run: &Value, key: &str) -> bool {
    run.get(key).is_some_and(|v| !v.is_null())
}
fn is(run: &Value, key: &str, value: &str) -> bool {
    run[key].as_str() == Some(value)
}
fn one_of(run: &Value, key: &str, values: &[&str]) -> bool {
    values.iter().any(|v| is(run, key, v))
}
fn conflict(message: &str) -> Error {
    Error::Conflict(message.into())
}

fn charge(run: &mut Value, now: i64) {
    if !has(run, "activeMs") {
        run["activeMs"] = json!(0);
    }
    if !has(run, "activeLimitMs") {
        run["activeLimitMs"] = json!(3_600_000);
    }
    if let Some(since) = run["activeSince"].as_i64() {
        let at = now.max(since);
        run["activeMs"] = json!(run["activeMs"].as_i64().unwrap_or(0) + at - since);
        run["activeSince"] = json!(at);
    }
    if is(run, "status", "running") && exhausted(run) {
        run["status"] = json!("pausing");
        run["pauseReason"] = json!("budget");
    }
}
fn exhausted(run: &Value) -> bool {
    run["activeMs"].as_u64().unwrap_or(0) >= run["activeLimitMs"].as_u64().unwrap_or(3_600_000)
}
fn resume_clock(run: &mut Value, now: i64) {
    if !has(run, "activeWait") {
        return;
    }
    remove(run, "activeWait");
    if one_of(run, "status", &["running", "pausing", "attention_required"]) {
        run["activeSince"] = json!(now);
    }
}
fn launch_started(run: &mut Value, now: i64) {
    if run["purpose"].as_str().unwrap_or("coding") == "coding"
        && has(run, "schedule")
        && !has(run, "launchCompletedAt")
    {
        run["launchCompletedAt"] = json!(timestamp(now));
    }
}

pub fn transition(previous: &Value, action: &Transition, now: i64) -> Result<Value> {
    if !previous.is_object() {
        return Err(Error::Invalid("Run must be an object.".into()));
    }
    let mut run = previous.clone();
    if one_of(&run, "status", &["closed", "completed"]) {
        return if matches!(action, Transition::Account { .. }) {
            Ok(run)
        } else {
            Err(conflict("This run is already complete."))
        };
    }
    match action {
        Transition::CompleteResearch => {
            if !is(&run, "purpose", "research")
                || !is(&run, "status", "paused")
                || !is(&run, "desiredState", "running")
            {
                return Err(conflict(
                    "Stop and verify the research team before completing.",
                ));
            }
            run["status"] = json!("completed");
            run["desiredState"] = json!("cancelled");
            remove(&mut run, "activeSince");
        }
        Transition::Close => {
            if !one_of(&run, "purpose", &["explore", "grill"]) {
                return Err(conflict("Only a refinement run can be closed."));
            }
            if !one_of(&run, "status", &["paused", "cancelled", "awaiting_runner"]) {
                return Err(conflict("Stop the coordinator before closing refinement."));
            }
            run["status"] = json!("closed");
            run["desiredState"] = json!("cancelled");
            remove(&mut run, "activeSince");
        }
        Transition::Account { waiting } => {
            if !waiting {
                resume_clock(&mut run, now);
            }
            charge(&mut run, now);
            if *waiting && is(&run, "status", "running") && !has(&run, "activeWait") {
                run["activeWait"] = json!({"kind": "hosted-ci", "since": now});
                remove(&mut run, "activeSince");
            }
        }
        Transition::Desire { desired_state } => {
            if !["running", "paused", "cancelled"].contains(&desired_state.as_str()) {
                return Err(Error::Invalid("Invalid desired state.".into()));
            }
            if is(&run, "desiredState", "cancelled") && desired_state != "cancelled" {
                return Err(conflict(
                    "This run is cancelled. Its retained work requires a new explicit continuation.",
                ));
            }
            if !is(&run, "desiredState", desired_state) {
                run["desiredState"] = json!(desired_state);
                run["controlRevision"] = json!(run["controlRevision"].as_u64().unwrap_or(0) + 1);
            }
        }
        Transition::ReviewRegressed | Transition::ReviewRestored => {
            let (from, to) = if matches!(action, Transition::ReviewRegressed) {
                ("review", "validation")
            } else {
                ("validation", "review")
            };
            if is(&run, "status", "paused")
                && is(&run, "desiredState", "paused")
                && is(&run, "pauseReason", from)
            {
                run["pauseReason"] = json!(to);
            }
        }
        Transition::Continue { control_revision } => {
            if !is(&run, "status", "paused")
                || !is(&run, "desiredState", "running")
                || run["controlRevision"].as_u64() != Some(*control_revision)
            {
                return Err(conflict("A newer control request superseded continuation."));
            }
            run["status"] = json!("awaiting_runner");
            remove(&mut run, "attentionReason");
        }
        Transition::Attention { reason } => {
            resume_clock(&mut run, now);
            charge(&mut run, now);
            run["status"] = json!("attention_required");
            run["attentionReason"] = json!(reason);
        }
        Transition::Start => {
            resume_clock(&mut run, now);
            charge(&mut run, now);
            if exhausted(&run) {
                return Err(conflict(
                    "The active execution budget is exhausted. Extend it explicitly before resuming.",
                ));
            }
            if is(&run, "desiredState", "cancelled")
                || !one_of(&run, "status", &["awaiting_runner", "paused", "running"])
            {
                return Err(conflict("Run is not available for execution."));
            }
            if !has(&run, "startedAt") {
                run["startedAt"] = json!(timestamp(now));
            }
            if !has(&run, "activeSince") {
                run["activeSince"] = json!(now);
            }
            run["status"] = json!("running");
            remove(&mut run, "pauseReason");
            launch_started(&mut run, now);
        }
        Transition::SetLimit { active_limit_ms } => {
            charge(&mut run, now);
            run["activeLimitMs"] = json!(active_limit_ms);
            charge(&mut run, now);
        }
        Transition::BeginPause { reason } => {
            resume_clock(&mut run, now);
            charge(&mut run, now);
            if let Some(reason) = reason {
                run["pauseReason"] = json!(reason);
            } else if !is(&run, "status", "pausing") {
                run["pauseReason"] = json!("operator");
            }
            run["status"] = json!("pausing");
        }
        Transition::FinishPause { unresolved } => {
            if *unresolved {
                resume_clock(&mut run, now);
            }
            charge(&mut run, now);
            run["status"] = json!(if *unresolved {
                "attention_required"
            } else if is(&run, "desiredState", "cancelled") {
                "cancelled"
            } else {
                "paused"
            });
            if *unresolved {
                run["attentionReason"] =
                    json!("Process ownership is unresolved. Replacement work is blocked.");
            } else {
                remove(&mut run, "activeSince");
                remove(&mut run, "activeWait");
                remove(&mut run, "attentionReason");
            }
        }
        Transition::AgentStart => {
            resume_clock(&mut run, now);
            if is(&run, "status", "awaiting_runner") {
                if !has(&run, "startedAt") {
                    run["startedAt"] = json!(timestamp(now));
                }
                run["status"] = json!("running");
                if !has(&run, "activeSince") {
                    run["activeSince"] = json!(now);
                }
            }
            launch_started(&mut run, now);
        }
        Transition::SourceChanged { reason, active } => {
            if !is(&run, "desiredState", "cancelled") {
                run["sourceConflict"] = json!(reason);
                run["attentionReason"] = json!(reason);
                run["desiredState"] = json!("paused");
                run["pauseReason"] = json!("source");
                run["controlRevision"] = json!(run["controlRevision"].as_u64().unwrap_or(0) + 1);
                run["status"] = json!(if !active && has(previous, "position") {
                    "awaiting_runner"
                } else if *active {
                    "pausing"
                } else {
                    "attention_required"
                });
            }
        }
    }
    if has(&run, "activeWait") && !is(&run, "status", "running") {
        resume_clock(&mut run, now);
    }
    Ok(run)
}

pub fn run_label(run: &Value) -> &'static str {
    let status = run["status"].as_str().unwrap_or("");
    if is(run, "desiredState", "cancelled") && status == "pausing" {
        return "Cancelling · confirming stop";
    }
    if is(run, "desiredState", "cancelled") && status == "attention_required" {
        return "Cancellation needs attention";
    }
    if status == "attention_required" {
        return if has(run, "resumeRequest") {
            "Needs attention · Resume queued"
        } else {
            "Needs attention"
        };
    }
    if run["resumeRequest"]["state"] == "queued" {
        return "Paused · Resume queued";
    }
    if run["resumeRequest"]["state"] == "starting"
        || (status == "paused" && is(run, "desiredState", "running"))
    {
        return "Resuming";
    }
    if has(run, "readinessHold") {
        return if run["readinessHold"]["state"] == "held" {
            "Paused · readiness unconfirmed"
        } else {
            "Paused · waiting for coding slot"
        };
    }
    match (status, run["pauseReason"].as_str().unwrap_or("")) {
        ("paused", "validation") => "Paused · validation needs attention",
        ("paused", "review") => "Paused · awaiting human review",
        ("pausing", "review") => "Pausing · awaiting human review",
        ("paused", "merged") => "Paused · PR merged",
        ("awaiting_runner", _) if has(run, "position") => "Queued",
        ("awaiting_runner", _) => "Waiting for runner",
        ("running", _) => "Coordinating",
        ("pausing", _) => "Pausing",
        ("paused", _) => "Paused",
        ("cancelled", _) => "Cancelled",
        ("closed", _) => "Closed",
        ("completed", _) => "Research completed",
        _ => "Workflow state unavailable",
    }
}
