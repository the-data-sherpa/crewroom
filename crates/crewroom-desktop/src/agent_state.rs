//! One plain-language vocabulary for agent attempt states, shared by the agents panel,
//! the agent wall, History attempt rows and the sidebar activity chip.
//! Agent activity is its own fact: it never stands in for Jira status, workflow state or a check result.

/// Healthy activity.
pub const WORKING: u32 = 0x84d7bc;
/// Settled, waiting for input.
pub const WAITING: u32 = 0x7faec5;
/// Recorded but not active.
pub const NEUTRAL: u32 = 0x9eacb9;
/// The operator should look: ownership or observation is in doubt.
pub const WARNING: u32 = 0xf1bf73;
/// The attempt failed.
pub const FAILED: u32 = 0xffb0a2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateLabel {
    /// The word a count or button uses, e.g. "Needs attention".
    pub short: String,
    /// The full label, e.g. "Needs attention: Process exited".
    pub text: String,
    pub tone: u32,
}

/// Map a recorded attempt state, plus its recorded ownership reason, to the shared label.
/// Unknown values show their raw token instead of a neutral word.
pub fn label(state: &str, reason: Option<&str>) -> StateLabel {
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    let (short, tone) = match state {
        "admitted" => ("Not started", NEUTRAL),
        "starting" => ("Starting", WAITING),
        "executing" => ("Working", WORKING),
        "idle" => ("Waiting", WAITING),
        "exited" => ("Stopped", NEUTRAL),
        "attention_required" => ("Needs attention", WARNING),
        "failed" => ("Failed", FAILED),
        // Presence facts the service reports in place of a state it could not confirm.
        "ownership_unconfirmed" => ("Ownership unconfirmed", WARNING),
        "observation_degraded" => ("Observation degraded", WARNING),
        raw => {
            let text = format!("State: {}", if raw.is_empty() { "missing" } else { raw });
            return StateLabel {
                short: text.clone(),
                text,
                tone: WARNING,
            };
        }
    };
    let text = match state {
        "idle" => "Waiting · not a check result".to_owned(),
        "attention_required" => format!(
            "Needs attention: {}",
            reason.unwrap_or("no ownership reason recorded")
        ),
        "failed" => reason.map_or_else(|| "Failed".to_owned(), |r| format!("Failed: {r}")),
        _ => short.to_owned(),
    };
    StateLabel {
        short: short.to_owned(),
        text,
        tone,
    }
}

/// Label an attempt record by its `state` and `ownershipReason` fields.
pub fn of(attempt: &serde_json::Value) -> StateLabel {
    label(
        attempt["state"].as_str().unwrap_or(""),
        attempt["ownershipReason"].as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_attempt_state_has_one_plain_label() {
        let cases = [
            ("admitted", "Not started", "Not started", NEUTRAL),
            ("starting", "Starting", "Starting", WAITING),
            ("executing", "Working", "Working", WORKING),
            ("idle", "Waiting", "Waiting · not a check result", WAITING),
            ("exited", "Stopped", "Stopped", NEUTRAL),
            (
                "attention_required",
                "Needs attention",
                "Needs attention: Process exited without a stop request",
                WARNING,
            ),
            (
                "failed",
                "Failed",
                "Failed: Process exited without a stop request",
                FAILED,
            ),
            ("unknown", "State: unknown", "State: unknown", WARNING),
        ];
        for (state, short, text, tone) in cases {
            let got = label(state, Some("Process exited without a stop request"));
            assert_eq!(
                (got.short.as_str(), got.text.as_str(), got.tone),
                (short, text, tone),
                "{state}"
            );
        }
    }

    #[test]
    fn attention_without_a_reason_says_so_and_unrecognised_tokens_stay_raw() {
        assert_eq!(
            label("attention_required", None).text,
            "Needs attention: no ownership reason recorded"
        );
        assert_eq!(label("failed", Some(" ")).text, "Failed");
        assert_eq!(
            label("paused_by_runner", None).text,
            "State: paused_by_runner"
        );
        assert_eq!(label("", None).text, "State: missing");
        assert_eq!(label("ownership_unconfirmed", None).tone, WARNING);
        assert_eq!(
            label("observation_degraded", None).short,
            "Observation degraded"
        );
    }
}
