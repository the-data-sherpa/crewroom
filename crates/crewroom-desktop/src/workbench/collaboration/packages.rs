//! Read-only package and exchange history on the existing coordinating surface.
use super::*;

fn admission(member: &Value, assignments: &[Value], attempts: &[Value]) -> String {
    let Some(assignment) = assignments.iter().find(|a| a["name"] == member["name"]) else {
        return "Planned · No process".into();
    };
    if assignment["retiredAt"].is_string() {
        return "Retired · History retained".into();
    }
    let attempt = attempts
        .iter()
        .find(|a| a["id"] == assignment["currentAttemptId"]);
    match attempt {
        Some(a) if a["ownership"] == "held" => {
            format!("Admitted · {} · Ownership held", string(a, "state"))
        }
        Some(a) if a["ownership"] == "stopped" => "Admitted · Process stopped".into(),
        _ => "Admitted · Ownership unconfirmed".into(),
    }
}
impl Workbench {
    pub(in crate::workbench) fn package_panel(&self, run: &Value, cx: &Context<Self>) -> Div {
        let runtime = &self.observation.runtime;
        let state = &runtime["choreography"];
        if !state.is_object() {
            return if run["executionPolicy"] == 1 {
                section("Work packages").child(hint("The coordinator is preparing the pair and specialist stages. No package has write authority yet."))
            } else {
                div()
            };
        }
        let run_id = string(run, "id");
        let assignments = records(runtime, "assignments");
        let attempts = records(runtime, "attempts");
        let mut panel = section("Work packages").min_w_0().child(hint(format!(
            "Plan {} · Requirements {} · Coordination revision {}",
            crate::workbench::results::short(&state["binding"]["planId"]),
            state["binding"]["requirementRevision"],
            state["revision"]
        )));
        for blocker in records(runtime, "choreographyBlockers")
            .iter()
            .filter_map(Value::as_str)
        {
            panel = panel.child(hint(blocker.to_owned()));
        }
        for member in records(&state["schedule"], "assignments") {
            let packages = records(&member, "packages")
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            panel = panel.child(
                div()
                    .id(SharedString::from(format!(
                        "package-assignment-{}",
                        string(&member, "name")
                    )))
                    .min_w_0()
                    .child(crate::accessibility::text(format!(
                        "{} · {} · Stage {}",
                        string(&member, "name"),
                        string(&member, "responsibility"),
                        member["stage"]
                    )))
                    .child(hint(format!(
                        "{} · Packages: {packages}",
                        admission(&member, &assignments, &attempts)
                    )))
                    .child(hint(string(&member, "rationale"))),
            );
        }
        for package in records(state, "packages") {
            panel = panel.child(hint(format!(
                "{} · {} · {}",
                string(&package, "id"),
                string(&package, "state"),
                string(&package, "reason")
            )));
        }
        let current_writer = attempts
            .iter()
            .find(|a| a["id"] == runtime["packageWriter"]["attemptId"]);
        let writer_name = current_writer
            .and_then(|a| assignments.iter().find(|m| m["id"] == a["assignmentId"]))
            .map(|a| string(a, "name"));
        panel = panel.child(hint(match (writer_name, current_writer) {
            (Some(name), Some(attempt)) => format!(
                "Current writer: {name} · Ownership {}",
                string(attempt, "ownership")
            ),
            _ => "Current writer: None · No write lease".into(),
        }));
        for completion in records(runtime, "packageCompletions") {
            if completion["mode"] == "checkpoint" {
                let old_name = attempts
                    .iter()
                    .find(|a| a["id"] == completion["attemptId"])
                    .and_then(|a| assignments.iter().find(|m| m["id"] == a["assignmentId"]))
                    .map(|a| string(a, "name"))
                    .unwrap_or_else(|| "Prior writer".into());
                let transferred = records(runtime, "packageClaims")
                    .iter()
                    .any(|c| c["handoffId"] == completion["id"]);
                panel = panel.child(
                    div()
                        .id(SharedString::from(format!(
                            "package-handoff-{}",
                            string(&completion, "id")
                        )))
                        .min_w_0()
                        .child(hint(format!(
                            "Handoff {old_name} → {} · {}",
                            string(&completion, "successor"),
                            if transferred {
                                "Transferred · Successor claim recorded"
                            } else if completion["state"] == "completed" {
                                "Checkpoint saved · Successor still requires a writer claim"
                            } else {
                                "Requested · Stop and ownership verification pending"
                            }
                        )))
                        .child(hint(string(&completion, "summary"))),
                );
            }
        }
        for report in records(state, "reports") {
            let id = string(&report, "id");
            let mut row = div()
                .id(SharedString::from(format!("package-report-{id}")))
                .min_w_0()
                .child(hint(format!(
                    "{} · {} report · {}",
                    string(&report, "packageId"),
                    string(&report, "name"),
                    crate::workbench::results::short(&report["source"]["head"])
                )))
                .child(crate::accessibility::text(string(&report, "summary")));
            if report["documentation"].is_object() {
                row = row.child(hint(format!(
                    "Documentation: {} · {}",
                    string(&report["documentation"], "disposition").replace('_', " "),
                    string(&report["documentation"], "reason")
                )));
            }
            for (index, artifact) in records(&report, "snapshots").iter().enumerate() {
                row = row.child(self.artifact_button(
                    &run_id,
                    artifact,
                    &format!("Read {}", string(artifact, "path")),
                    format!("package-artifact-{id}-{index}"),
                    cx,
                ));
            }
            panel = panel.child(row);
        }
        for exchange in records(state, "exchanges") {
            let message = records(runtime, "messages")
                .into_iter()
                .find(|m| m["id"] == exchange["messageId"])
                .unwrap_or(Value::Null);
            panel = panel.child(
                div()
                    .id(SharedString::from(format!(
                        "package-exchange-{}",
                        string(&exchange, "id")
                    )))
                    .min_w_0()
                    .child(hint(format!(
                        "{} · {} · {} · {}",
                        string(&exchange, "packageId"),
                        string(&exchange, "kind"),
                        crate::workbench::results::short(&exchange["id"]),
                        super::conversation::delivery(
                            message["disposition"].as_str().unwrap_or("")
                        )
                    )))
                    .child(hint(format!(
                        "Challenge {} · Response {} · Source {}",
                        crate::workbench::results::short(&exchange["challengeId"]),
                        crate::workbench::results::short(&exchange["responseId"]),
                        crate::workbench::results::short(&exchange["source"]["head"])
                    )))
                    .child(crate::accessibility::text(string(&exchange, "body")))
                    .child(hint(format!(
                        "{} · {}",
                        string(&exchange, "artifact"),
                        string(&exchange, "proposedChange")
                    ))),
            );
        }
        panel
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planned_successor_is_never_presented_as_observed_writer() {
        let member = json!({"name":"Docs"});
        assert_eq!(admission(&member, &[], &[]), "Planned · No process");
        let a = json!({"id":"a","name":"Docs","currentAttemptId":"p"});
        assert_eq!(
            admission(&member, std::slice::from_ref(&a), &[]),
            "Admitted · Ownership unconfirmed"
        );
        assert_eq!(
            admission(&member, &[a], &[json!({"id":"p","ownership":"stopped"})]),
            "Admitted · Process stopped"
        );
    }
}
