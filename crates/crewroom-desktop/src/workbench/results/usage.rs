//! Usage grouped by billing route. API use shows a list-price estimate;
//! subscription use shows runs and tokens only; unknown routes show no cost.
//! Totals are per route and never combined.
use super::{text_lines, value};
use crate::workbench::*;

/// Heading for one billing-route group.
pub(in crate::workbench) fn route_heading(billing: &Value) -> &'static str {
    match billing.as_str() {
        Some("api") => "API · estimated cost",
        Some("subscription") => "Subscription · runs and tokens",
        _ => "Unknown billing route",
    }
}
fn route_name(billing: &Value) -> &'static str {
    match billing.as_str() {
        Some("api") => "API",
        Some("subscription") => "subscription",
        _ => "unknown billing",
    }
}
fn count(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_f64().map(|n| n as u64))
        .unwrap_or(0)
}

impl Workbench {
    pub(in crate::workbench) fn usage_panel(&self, usage: &Value) -> Div {
        let mut panel = section("Recorded usage");
        if usage.is_null() {
            return panel.child(hint(
                "Usage observation is unavailable. Missing reports are not zero usage.",
            ));
        }
        panel = panel
            .child(crate::accessibility::text(
                "Usage by billing route · never totalled across routes",
            ))
            .child(hint(string(usage, "note")))
            .child(hint(format!("Source: {}", string(usage, "source"))));
        let routes = records(usage, "routes");
        if routes.is_empty() {
            panel = panel.child(hint("No assignment has been admitted for this run."));
        }
        for group in routes {
            panel = panel.child(route_group(&group));
        }
        for row in records(usage, "rows") {
            panel = panel.child(
                section(format!(
                    "{} · attempt {} · {} · {}",
                    value(&row["name"]),
                    value(&row["generation"]),
                    value(&row["model"]),
                    route_name(&row["billing"])
                ))
                .id(SharedString::from(format!(
                    "usage-attempt-{}",
                    value(&row["attemptId"])
                )))
                .child(usage_numbers(&row))
                .child(hint(text_lines(&row["unavailableReasons"]))),
            );
        }
        panel
    }
}

fn route_group(group: &Value) -> gpui_kit::Stateful<Div> {
    let billing = group["billing"].as_str().unwrap_or("unknown").to_owned();
    let attempts = count(&group["attempts"]);
    let mut view = section(route_heading(&group["billing"]))
        .id(SharedString::from(format!("usage-route-{billing}")))
        .child(hint(format!(
            "{} started attempt(s) across {} assignment(s)",
            attempts,
            count(&group["assignments"])
        )));
    if attempts > 0 && count(&group["tokenAttempts"]) < attempts {
        view = view.child(hint(format!(
            "Tokens reported for {} of {} attempt(s); the rest are unavailable, not zero.",
            count(&group["tokenAttempts"]),
            attempts
        )));
    }
    let unrecorded = count(&group["unrecordedAttempts"]);
    if unrecorded > 0 {
        view = view.child(hint(format!(
            "{unrecorded} attempt(s) were assigned before billing routes were recorded."
        )));
    }
    view.child(usage_numbers(group))
}

pub(in crate::workbench) fn usage_numbers(row: &Value) -> Div {
    let label = if row["awaitingStart"] == true {
        "Awaiting start"
    } else {
        match row["state"].as_str() {
            Some("actual") => "Provider-reported usage",
            Some("partial") => "Partial provider usage",
            _ => "Usage unavailable",
        }
    };
    let mut view = div().child(crate::accessibility::text(label));
    if !row["tokens"].is_null() {
        view = view.child(hint(format!(
            "Input {} · output {} · cache read {} · cache write {} · reasoning {} · total {}",
            value(&row["tokens"]["input"]),
            value(&row["tokens"]["output"]),
            value(&row["tokens"]["cacheRead"]),
            value(&row["tokens"]["cacheWrite"]),
            value(&row["tokens"]["reasoning"]),
            value(&row["tokens"]["total"])
        )));
    }
    let requests = match row["requestCounts"].as_str() {
        Some("unavailable") => "Provider request counts unavailable (not zero)".into(),
        state => format!(
            "Reported responses: {} / {} provider requests{}",
            value(&row["actualResponses"]),
            value(&row["providerRequests"]),
            if state == Some("partial") {
                " · partial counts; native requests unavailable"
            } else {
                ""
            }
        ),
    };
    view = view.child(hint(requests));
    match row["billing"].as_str() {
        Some("api") => {
            let partial = row["state"] != "actual" || row["costResponses"] != row["responses"];
            view.child(hint(format!(
                "Estimated cost (runner list price, not provider billing): {}{}",
                row["costUsd"]
                    .as_f64()
                    .map(|n| format!("${n:.6}"))
                    .unwrap_or_else(|| "unavailable, not zero".into()),
                if partial && row["costUsd"].is_number() {
                    " · partial"
                } else {
                    ""
                }
            )))
            .child(hint(
                if row["costBasis"] == "cumulative-list-price-estimate" {
                    "Includes cumulative list-price estimates".into()
                } else if row["requestCounts"] == "unavailable" {
                    "Response cost coverage unavailable".into()
                } else {
                    format!(
                        "Runner pricing reports: {} / {} responses",
                        value(&row["costResponses"]),
                        value(&row["responses"])
                    )
                },
            ))
        }
        Some("subscription") => {
            view = view.child(hint("Subscription use · no dollar figure is shown"));
            if row["estimateWithheld"] == true {
                view = view.child(hint(
                    "The harness reported a list-price estimate; it is not a subscription charge and is not shown.",
                ));
            }
            view
        }
        _ => {
            view = view.child(hint("Billing route unknown · no cost estimate is shown"));
            if row.get("billingRecorded").is_some() && row["billingRecorded"] != true {
                view = view.child(hint(
                    "Billing route not recorded: assigned before billing routes were detected.",
                ));
            }
            view
        }
    }
}
