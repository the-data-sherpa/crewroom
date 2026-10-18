use super::results::{short, usage_numbers, value};
use super::*;
impl Workbench {
    pub(super) fn history_panel(
        &mut self,
        task: &Value,
        runs: &[Value],
        run: Option<&Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panel = section("Run history");
        let task_id = string(task, "id");
        if runs.is_empty() {
            panel = panel.child(hint(
                "No runs recorded. Refinements remain saved independently.",
            ));
        }
        for item in runs.iter().rev() {
            let id = string(item, "id");
            let tid = task_id.clone();
            let selected = run.is_some_and(|r| r["id"] == item["id"]);
            let mut row = section(format!(
                "{} · brief revision {}",
                crewroom_core::lifecycle::run_label(item),
                value(&item["taskRevision"])
            ))
            .id(SharedString::from(format!(
                "history-run-row-{}",
                string(item, "id")
            )))
            .child(hint(format!(
                "Run {id} · created {}",
                value(&item["createdAt"])
            )))
            .child(
                Button::new(SharedString::from(format!("history-run-{id}")))
                    .label(if selected {
                        "Selected run"
                    } else {
                        "Inspect run"
                    })
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_runs.insert(tid.clone(), id.clone());
                        cx.notify();
                    })),
            );
            if let Some(branch) = item["continuedFrom"]["branch"].as_str() {
                row = row.child(hint(format!(
                    "Continued retained branch {branch} from {}",
                    value(&item["continuedFrom"]["runId"])
                )));
            }
            if item["position"].is_number() {
                let id = string(item, "id");
                row = row.child(
                    Button::new(SharedString::from(format!("remove-{id}")))
                        .label("Cancel queued plan…")
                        .disabled(self.blocked())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.confirm_queued_plan_cancel(&id, window, cx)
                        })),
                );
            }
            panel = panel.child(row);
        }
        if let Some(run) = run {
            panel = panel.child(self.attempt_history(run, cx));
            let captured = &self.observation.result["repository"];
            if captured.is_object() {
                panel = panel.child(section("Run repository · captured at preparation").child(
                    hint(format!(
                        "{} · {}\nTarget {} · branch prefix {}",
                        value(&captured["name"]),
                        value(&captured["path"]),
                        value(&captured["targetBranch"]),
                        value(&captured["branchPrefix"])
                    )),
                ));
            }
        }
        let mut revisions = section("Accepted requirement history");
        for revision in records(&self.workspace, "requirementRevisions")
            .into_iter()
            .filter(|r| r["taskId"] == task_id)
            .rev()
        {
            let mut row = section(format!(
                "Accepted revision {} · source revision {}",
                value(&revision["revision"]),
                value(&revision["sourceRevision"])
            ))
            .id(SharedString::from(format!(
                "requirement-revision-{}",
                value(&revision["revision"])
            )))
            .child(hint(format!(
                "Accepted {} · origin {}",
                value(&revision["createdAt"]),
                value(&revision["origin"])
            )))
            .child(crate::accessibility::text(value(&revision["brief"])));
            for (i, criterion) in records(&revision, "criteria").iter().enumerate() {
                row = row.child(
                    hint(format!("Criterion {i}: {}", value(criterion)))
                        .id(SharedString::from(format!("criterion-{i}"))),
                );
            }
            if let Some(source) = revision.get("source") {
                row = row.child(hint(format!("Captured source: {source}")));
            }
            revisions = revisions.child(row);
        }
        panel.child(revisions).child(self.detail_panel(window, cx))
    }
    fn attempt_history(&mut self, run: &Value, cx: &mut Context<Self>) -> Div {
        let history = &self.observation.runtime["history"];
        let attempts = records(history, "attempts");
        let mut panel = section("Attempt history");
        if !self.observation.runtime_fresh {
            panel=panel.child(hint("Run observation unavailable or stale. Retained attempt states are recorded history, not a new process observation."));
        }
        if history.is_null() {
            return panel.child(hint("Attempt history is unavailable."));
        }
        if attempts.is_empty() {
            return panel.child(hint(
                "No agent attempts recorded. A queued plan is not an executing agent.",
            ));
        }
        let mut choices = div().flex().flex_wrap().gap_2();
        for item in &attempts {
            let id = string(item, "attemptId");
            let state = crate::agent_state::of(item).short;
            choices = choices.child(
                Button::new(SharedString::from(format!("history-attempt-{id}")))
                    .min_w_0()
                    .max_w_full()
                    .label(format!(
                        "{} · attempt {} · {} · {state}",
                        value(&item["name"]),
                        value(&item["generation"]),
                        if !item["retiredAt"].is_null() {
                            "retired"
                        } else if item["current"] == true {
                            "current"
                        } else {
                            "prior"
                        }
                    ))
                    .selected(self.observation.selected_attempt.as_deref() == Some(&id))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.observation.selected_attempt = Some(id.clone());
                        this.observation.detail = None;
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(choices);
        let selected = self
            .observation
            .selected_attempt
            .as_ref()
            .and_then(|id| attempts.iter().find(|a| a["attemptId"] == *id));
        let Some(item) = selected else {
            if self.observation.selected_attempt.is_some() {
                panel = panel.child(hint(
                    "That attempt is unavailable in this run. Select an available attempt.",
                ));
            }
            return panel;
        };
        let mut detail=section(format!("{} · attempt {} details",value(&item["name"]),value(&item["generation"]))).child(hint(format!("Read-only {} attempt. Inspecting it does not start or resume an agent.",if item["current"]==true{"current"}else{"prior"})))
            .child(Button::new("back-to-current-run").label("Back to run conversation").on_click(cx.listener(|this,_,_,cx|{this.observation.selected_attempt=None;this.observation.detail=None;this.tab="Conversation";cx.notify();})))
            .child(hint(format!("Model: {} · {}\nRequested {} · {}\nState: {} · ownership {}\nAdmitted {} · start verified {} · stop confirmed {}",value(&item["model"]["resolved"]["selector"]),value(&item["model"]["resolved"]["effort"]),value(&item["model"]["requested"]["model"]),value(&item["model"]["requested"]["effort"]),crate::agent_state::of(item).text,value(&item["ownership"]),value(&item["createdAt"]),value(&item["startedAt"]),value(&item["stoppedAt"]))));
        detail = detail.child(super::model_details::details(&item["model"]));
        if item["runnerKind"].is_string() {
            detail = detail
                .child(hint(format!(
                    "Runner observation: {} · {} · Model: {} · Session: {}",
                    value(&item["runnerKind"]),
                    value(&item["runnerVersion"]),
                    value(&item["observedModel"]),
                    value(&item["sessionId"])
                )))
                .child(hint(format!(
                    "Observed runner profile: {}",
                    value(&item["runnerProfile"])
                )));
        }
        if !item["model"]["fallbackFrom"].is_null() {
            detail = detail.child(hint(format!(
                "Fallback from {}",
                value(&item["model"]["fallbackFrom"])
            )));
        }
        if !item["retiredAt"].is_null() {
            detail = detail.child(hint(format!(
                "Assignment retired {}: {}",
                value(&item["retiredAt"]),
                value(&item["retirementReason"])
            )));
        }
        detail = detail.child(crate::accessibility::text("Recorded cause"));
        let causes = records(item, "causes");
        if causes.is_empty() {
            detail = detail.child(hint("No failure or stop recorded."));
        }
        for (index, cause) in causes.iter().enumerate() {
            detail =
                detail.child(hint(value(cause)).id(SharedString::from(format!("cause-{index}"))));
        }
        detail = detail.child(hint(format!("Next: {}", value(&item["nextAction"]))));
        let usage = records(&history["usage"], "rows");
        if let Some(usage) = usage.iter().find(|r| r["attemptId"] == item["attemptId"]) {
            detail = detail.child(usage_numbers(usage));
            for (index, reason) in records(usage, "unavailableReasons").iter().enumerate() {
                detail = detail.child(
                    hint(value(reason)).id(SharedString::from(format!("usage-reason-{index}"))),
                );
            }
        } else {
            detail = detail.child(hint(
                "Usage unavailable. Missing records are not zero usage.",
            ));
        }
        detail=detail.child(hint(format!("Usage source: {}",value(&history["usage"]["source"])))).child(crate::accessibility::text("Evidence produced")).child(hint("These records cover their named revisions. They do not establish current readiness."));
        let evidence = records(item, "evidence");
        if evidence.is_empty() {
            detail = detail.child(hint("No evidence recorded for this attempt."));
        }
        for evidence in evidence {
            let mut row = section(value(&evidence["label"]))
                .id(SharedString::from(format!(
                    "attempt-evidence-{}",
                    value(&evidence["id"])
                )))
                .child(hint(format!(
                    "{} · recorded {}",
                    value(&evidence["revision"]),
                    value(&evidence["at"])
                )))
                .child(hint(evidence["detail"].as_str().unwrap_or("").to_owned()));
            if evidence["artifact"].is_object() {
                row = row
                    .child(hint(format!(
                        "Artifact SHA-256 {}",
                        value(&evidence["artifact"]["sha256"])
                    )))
                    .child(self.artifact_button(
                        &string(run, "id"),
                        &evidence["artifact"],
                        "View saved attempt artifact",
                        format!(
                            "history-evidence-{}-{}",
                            short(&item["attemptId"]),
                            value(&evidence["id"])
                        ),
                        cx,
                    ));
            }
            detail = detail.child(row);
        }
        panel.child(detail)
    }
}
