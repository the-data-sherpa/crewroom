//! FOR-74: remaining subscription allowance per harness login, beside the
//! usage view and the model picker. Visibility only. A window or harness
//! without a reported value reads "unavailable" with its reason, never zero;
//! every reading names when it was observed.
use super::*;

fn time(v: &Value) -> Option<String> {
    v.as_i64()
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
}
fn percent(v: &Value) -> Option<String> {
    v.as_f64().map(|n| {
        if n.fract() == 0.0 {
            format!("{n:.0}%")
        } else {
            format!("{n:.1}%")
        }
    })
}

/// "5-hour · 64% left · resets 2026-10-19 09:19 UTC", or the reason a value
/// is unavailable.
pub(super) fn window_line(w: &Value) -> String {
    let remaining = percent(&w["remainingPercent"])
        .map(|p| format!("{p} left"))
        .unwrap_or_else(|| "remaining unavailable".into());
    let reset = time(&w["resetsAt"])
        .map(|t| format!("resets {t}"))
        .unwrap_or_else(|| "reset time unavailable".into());
    let mut line = format!("{} · {remaining} · {reset}", string(w, "label"));
    if let Some(reason) = w["reason"].as_str() {
        line = format!("{line} · {reason}");
    }
    line
}

/// "Codex · available · plan pro".
pub(super) fn heading(reading: &Value) -> String {
    let state = match reading["state"].as_str() {
        Some("available") => "available",
        Some("partial") => "partial",
        _ => "unavailable",
    };
    let mut line = format!("{} allowance · {state}", string(reading, "label"));
    if let Some(plan) = reading["plan"].as_str() {
        line = format!("{line} · plan {plan}");
    }
    line
}

pub(super) fn observed(reading: &Value) -> String {
    match time(&reading["observedAt"]) {
        Some(t) => format!(
            "Observed {t}{}",
            reading["source"]
                .as_str()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default()
        ),
        None => "Not observed yet".into(),
    }
}

/// The readings for every harness. `place` keeps element identities distinct
/// where the summary appears more than once.
pub(super) fn summary(allowance: &Value, place: &str) -> Div {
    let mut panel = div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap_2()
        .child(crate::accessibility::text("Subscription allowance"));
    if !allowance.is_object() {
        return panel.child(hint(
            "Allowance is unavailable from this service. Remaining allowance is unknown, not zero.",
        ));
    }
    panel = panel.child(hint(string(allowance, "note")));
    if allowance["state"] == "refreshing" {
        panel = panel.child(hint("Reading allowance…"));
    }
    for reading in records(allowance, "harnesses") {
        let harness = string(&reading, "harness");
        let mut row = div()
            .id(SharedString::from(format!("allowance-{place}-{harness}")))
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .child(crate::accessibility::text(heading(&reading)));
        if let Some(reason) = reading["reason"].as_str() {
            row = row.child(hint(reason.to_owned()));
        }
        for (index, window) in records(&reading, "windows").into_iter().enumerate() {
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "allowance-{place}-{harness}-window-{index}"
                    )))
                    .child(hint(window_line(&window))),
            );
        }
        for (index, note) in records(&reading, "notes").into_iter().enumerate() {
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "allowance-{place}-{harness}-note-{index}"
                    )))
                    .child(hint(note.as_str().unwrap_or("").to_owned())),
            );
        }
        panel = panel.child(row.child(hint(observed(&reading))));
    }
    panel
}

impl Workbench {
    /// Allowance beside the model picker, with its refresh control.
    pub(super) fn allowance_panel(&self, cx: &mut Context<Self>) -> Div {
        let allowance = &self.workspace["allowance"];
        let refreshing = allowance["state"] == "refreshing";
        summary(allowance, "models").child(
            div().child(
                Button::new("refresh-allowance")
                    .label(if refreshing {
                        "Reading allowance…"
                    } else {
                        "Refresh allowance"
                    })
                    .disabled(self.blocked() || refreshing || !allowance.is_object())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.send("/allowance/refresh".into(), json!({}), false, cx)
                    })),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_name_remaining_share_reset_and_observation() {
        let w = json!({"label":"5-hour","remainingPercent":64.0,"usedPercent":36.0,"resetsAt":1792401599});
        assert_eq!(
            window_line(&w),
            "5-hour · 64% left · resets 2026-10-19 09:19 UTC"
        );
        let missing = json!({"label":"Weekly","remainingPercent":null,"resetsAt":null,"reason":"Claude Code did not report this window."});
        assert_eq!(
            window_line(&missing),
            "Weekly · remaining unavailable · reset time unavailable · Claude Code did not report this window."
        );
        let reading = json!({"label":"Codex","state":"partial","plan":"pro","observedAt":1792401599,"source":"Codex app-server account/rateLimits/read"});
        assert_eq!(heading(&reading), "Codex allowance · partial · plan pro");
        assert_eq!(
            observed(&reading),
            "Observed 2026-10-19 09:19 UTC · Codex app-server account/rateLimits/read"
        );
        assert_eq!(observed(&json!({})), "Not observed yet");
    }
}
