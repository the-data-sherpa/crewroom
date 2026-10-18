//! Explicit operator transitions. A successful request is never an execution receipt.
use super::*;
fn terminal(run: &Value) -> bool {
    run["desiredState"] == "cancelled"
        || ["closed", "completed", "cancelled"].contains(&run["status"].as_str().unwrap_or(""))
}
/// A queued plan has not been admitted, so no agent exists to stop.
pub(super) const QUEUED_PLAN_CANCEL: &str = "No agent has started. Its history is kept and it cannot resume. Queue it again from the Brief.";
/// Copy for a confirmation that guards an irreversible transition.
pub(super) struct Confirmation {
    pub(super) title: &'static str,
    pub(super) description: String,
    pub(super) keep: &'static str,
    pub(super) confirm: &'static str,
}
impl Workbench {
    /// Irreversible transitions confirm first; the keep option sends nothing.
    pub(super) fn confirm_send(
        &self,
        copy: Confirmation,
        path: String,
        body: Value,
        keyed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        let description = SharedString::from(copy.description);
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (weak, path, body) = (weak.clone(), path.clone(), body.clone());
            dialog
                .title(crate::accessibility::text(copy.title))
                .description(crate::accessibility::text(description.clone()))
                .show_cancel(true)
                .cancel_text(copy.keep)
                .ok_text(copy.confirm)
                .on_ok(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.send(path.clone(), body.clone(), keyed, cx)
                    });
                    true
                })
        });
    }
    /// Removing a queued plan cancels its run; the service refuses it after admission.
    pub(super) fn confirm_queued_plan_cancel(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_send(
            Confirmation {
                title: "Cancel this queued plan?",
                description: QUEUED_PLAN_CANCEL.into(),
                keep: "Keep plan",
                confirm: "Cancel queued plan",
            },
            format!("/queue/{id}/remove"),
            json!({}),
            true,
            window,
            cx,
        );
    }
    pub(super) fn run_controls(
        &mut self,
        task: &Value,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = string(run, "id");
        let status = run["status"].as_str().unwrap_or("");
        let purpose = run["purpose"].as_str().unwrap_or("coding");
        let runtime = self.observation.runtime.clone();
        let assignments = records(&runtime, "assignments");
        let blocked = self.blocked() || !self.observation.runtime_fresh || runtime["busy"] == true;
        let runner = self.workspace["capabilities"]["execution"] == true;
        let used = run["activeMs"].as_u64().unwrap_or(0);
        let limit = run["activeLimitMs"].as_u64().unwrap_or(3_600_000);
        let mut panel = section("Run controls").child(hint(format!(
            "Recorded active time: {}m {}s / {}m",
            used / 60_000,
            used % 60_000 / 1000,
            limit / 60_000
        )));
        if run["activeWait"]["kind"] == "hosted-ci" {
            panel=panel.child(hint("Waiting for provider-confirmed CI; this wait is excluded from active execution time."));
        }
        if !self.observation.runtime_fresh {
            panel = panel.child(hint(
                "Run observation is not current. Reconnect before changing execution.",
            ));
        }
        if let Some(error) = runtime["error"].as_str() {
            panel=panel.child(hint(format!("Last controller observation: {error}. Inspect current process state before deciding whether it is resolved.")));
        }
        for (key, label) in [
            ("attentionReason", "Needs attention"),
            ("feedbackResumeHold", "Feedback hold"),
            ("launchHold", "Launch hold"),
        ] {
            if let Some(value) = run.get(key).filter(|v| !v.is_null()) {
                let reason = value.as_str().unwrap_or_else(|| {
                    value["reason"]
                        .as_str()
                        .unwrap_or("Inspect saved run history.")
                });
                panel = panel.child(
                    hint(format!("{label}: {reason}"))
                        .id(SharedString::from(format!("run-hold-{key}"))),
                );
            }
        }
        for (index, (condition, explanation)) in [
            (
                run["pauseReason"] == "budget",
                "Active-time limit reached. Increase the total limit before Resume.",
            ),
            (
                run["pauseReason"] == "review" && run["readinessHold"].is_null(),
                "Paused for human review. Resume explicitly continues work; current evidence is shown in Results.",
            ),
            (
                run["readinessHold"]["state"] == "held",
                "Readiness is no longer confirmed. This paused run holds the coding slot until you explicitly Resume.",
            ),
            (
                run["readinessHold"]["state"] == "pending",
                "Readiness is no longer confirmed. This run remains paused while ownership and the coding slot are reconciled.",
            ),
            (
                run["resumeRequest"]["state"] == "queued",
                "Resume is queued. Agents wait for a safe coding slot and confirmed prior ownership.",
            ),
            (
                run["pauseReason"] == "validation",
                "Current validation no longer supports review readiness. Resume explicitly asks agents to reconcile it.",
            ),
            (
                run["desiredState"] == "cancelled",
                "This run cannot resume. Its changes, messages, PR and evidence remain in history.",
            ),
        ].into_iter().enumerate() {
            if condition {
                panel = panel.child(hint(explanation).id(SharedString::from(format!("run-explanation-{index}"))));
            }
        }
        let stopping_blocked = !self.connected || !self.observation.runtime_fresh;
        let can_pause = ([
            "running",
            "awaiting_runner",
            "pausing",
            "attention_required",
        ]
        .contains(&status)
            || self
                .pending_commands
                .contains(&format!("/runs/{id}/resume")))
            && !assignments.is_empty()
            && !terminal(run);
        let can_resume = !terminal(run)
            && ["paused", "attention_required"].contains(&status)
            && (!assignments.is_empty()
                || purpose == "coding"
                    && (!run["schedule"].is_null() || run["slotRelease"]["reason"] == "migration"));
        let mut buttons = div().flex().flex_wrap().gap_2();
        if can_pause {
            let path = format!("/runs/{id}/pause");
            buttons = buttons.child(
                Button::new("pause-run")
                    .label(if status == "pausing" {
                        "Pausing…"
                    } else {
                        "Pause run"
                    })
                    .disabled(
                        stopping_blocked
                            || !runner
                            || status == "pausing"
                            || self.pending_commands.contains(&path),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(path.clone(), json!({}), false, cx)
                    })),
            );
        }
        if can_resume {
            let path = format!("/runs/{id}/resume");
            let control_revision = run["controlRevision"].as_u64().unwrap_or(0);
            let pending = !run["resumeRequest"].is_null();
            buttons = buttons.child(
                Button::new("resume-run")
                    .primary()
                    .label(if pending {
                        "Resume pending"
                    } else {
                        "Resume run"
                    })
                    .disabled(blocked || !runner || used >= limit || pending)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(
                            path.clone(),
                            json!({"controlRevision":control_revision}),
                            false,
                            cx,
                        )
                    })),
            );
        }
        if !terminal(run) && !["explore", "grill"].contains(&purpose) {
            let run_id = id.clone();
            buttons = buttons.child(
                Button::new("cancel-run")
                    .label("Cancel run…")
                    .disabled(
                        stopping_blocked
                            || self.pending_commands.contains(&format!("/runs/{id}/cancel")),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let queued = records(&this.workspace, "runs")
                            .iter()
                            .any(|r| r["id"] == run_id && r["position"].is_number());
                        let copy = if queued {
                            Confirmation {
                                title: "Cancel this queued plan?",
                                description: QUEUED_PLAN_CANCEL.into(),
                                keep: "Keep plan",
                                confirm: "Cancel queued plan",
                            }
                        } else {
                            Confirmation {
                                title: "Cancel this run?",
                                description: "Agents stop. Changes, evidence and any PR are kept. The cancelled run cannot resume; retained work can use a linked successor when eligible.".into(),
                                keep: "Keep run",
                                confirm: "Confirm cancel",
                            }
                        };
                        this.confirm_send(
                            copy,
                            format!("/runs/{run_id}/cancel"),
                            json!({}),
                            queued,
                            window,
                            cx,
                        );
                    })),
            );
        }
        if (can_pause || can_resume) && !runner {
            panel = panel.child(hint(
                "Runner not configured. Execution controls are unavailable.",
            ));
        }
        panel = panel.child(buttons);
        if !terminal(run) && (run["pauseReason"] == "budget" || used >= limit) {
            let path = format!("/runs/{id}/budget");
            self.collaboration.form(
                &path,
                &json!({"minutes":limit.div_ceil(60_000)+30}),
                &[("minutes", false)],
                window,
                cx,
            );
            let form = &self.collaboration.forms[&path];
            let milliseconds = form
                .fields
                .text("minutes", cx)
                .trim()
                .parse::<u64>()
                .ok()
                .and_then(|n| n.checked_mul(60_000))
                .filter(|n| *n > used && *n <= 9_007_199_254_740_991);
            panel=panel.child(form.fields.render("active-limit","minutes","Total active-time limit (minutes)",self.busy)).child(hint("Choose a total limit greater than the time already used. Saving does not resume the run."))
                .child(Button::new("set-active-limit").label("Set total limit").disabled(blocked||milliseconds.is_none()).on_click(cx.listener(move|this,_,_,cx|{if let Some(n)=milliseconds {this.send(path.clone(),json!({"activeLimitMs":n}),false,cx)}})));
        }
        panel
            .child(self.continuation(task, run, window, cx))
            .child(self.repair_controls(run, &assignments, window, cx))
    }
    fn continuation(
        &mut self,
        task: &Value,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panel = div();
        let task_id = string(task, "id");
        for (other, label) in [
            (
                run["continuedFrom"]["runId"].as_str(),
                "View cancelled predecessor",
            ),
            (run["continuedBy"].as_str(), "View linked successor"),
        ] {
            if let Some(other) = other {
                let other = other.to_owned();
                let task = task_id.clone();
                panel=panel.child(hint("Continuation keeps prior evidence in its original run. Each successor needs fresh validation.").id(SharedString::from(format!("continuation-{}", other)))).child(Button::new(SharedString::from(format!("linked-run-{other}"))).label(label).on_click(cx.listener(move|this,_,_,cx|{this.selected_runs.insert(task.clone(),other.clone());cx.notify();})));
            }
        }
        if run["desiredState"] != "cancelled"
            || run["purpose"].as_str().unwrap_or("coding") != "coding"
            || run["continuedBy"].is_string()
        {
            return panel;
        }
        let id = string(run, "id");
        let retained = self.workspace["continuations"][&id].clone();
        let mut body = section("Continue retained work");
        if let Some(branch) = retained["branch"].as_str() {
            body = body.child(hint(format!(
                "Retained branch {branch} · head {}",
                retained["head"].as_str().unwrap_or("unknown")
            )));
        } else {
            return panel.child(body.child(hint(
                "No owned branch is available. Queue a new run from the accepted brief.",
            )));
        }
        let path = format!("/runs/{id}/continue-later");
        self.collaboration.form(&path, &json!({}), &[], window, cx);
        let roles: Vec<_> = records(&self.workspace, "roles")
            .into_iter()
            .filter(|r| r["enabled"] == true)
            .collect();
        let form = self.collaboration.forms.get_mut(&path).unwrap();
        if form.role.is_empty() {
            form.role = roles
                .iter()
                .find(|r| r["id"] == "coordinator")
                .or(roles.first())
                .map(|r| string(r, "id"))
                .unwrap_or_default();
        }
        let selected = form.role.clone();
        let mut choices = div().flex().flex_wrap().gap_2();
        for role in &roles {
            let key = path.clone();
            let role_id = string(role, "id");
            choices = choices.child(
                Button::new(SharedString::from(format!("continuation-role-{role_id}")))
                    .label(string(role, "name"))
                    .selected(selected == role_id)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.collaboration.forms.get_mut(&key).unwrap().role = role_id.clone();
                        cx.notify();
                    })),
            );
        }
        let reason = retained["reason"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                if self.brief_dirty(task, cx) {
                    Some("Save or discard brief edits before continuing.".into())
                } else if task["needsReacceptance"] == true {
                    Some("Accept the changed source requirements before continuing.".into())
                } else {
                    None
                }
            });
        if let Some(reason) = &reason {
            body = body.child(hint(reason.clone()));
        }
        let disabled = self.blocked()
            || !self.observation.runtime_fresh
            || reason.is_some()
            || !roles.iter().any(|r| r["id"] == selected);
        let payload = json!({"revision":task["revision"],"roleId":selected});
        panel.child(body.child(hint("The new linked run keeps the owned branch and any PR. The cancelled run stays closed, with its original evidence.")).child(crate::accessibility::text("Coordinator role for successor")).child(choices).child(Button::new("continue-later").primary().label("Continue later").disabled(disabled).on_click(cx.listener(move|this,_,_,cx|this.send(path.clone(),payload.clone(),true,cx)))))
    }
    fn repair_controls(
        &mut self,
        run: &Value,
        assignments: &[Value],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panel = div();
        for a in assignments {
            let allowances = records(a, "repairAllowances");
            let limit = 2 + allowances.len() as u64;
            let used = a["automaticCycles"].as_u64().unwrap_or(0);
            if used < limit || !a["retiredAt"].is_null() || terminal(run) {
                continue;
            }
            let id = string(a, "id");
            let path = format!(
                "/runs/{}/assignments/{id}/repair-allowance",
                string(run, "id")
            );
            self.collaboration
                .form(&path, &json!({}), &[("reason", true)], window, cx);
            let form = &self.collaboration.forms[&path];
            let reason = form.fields.text("reason", cx);
            let mut row=section(format!("Repair allowance · {}",string(a,"name"))).id(SharedString::from(format!("repair-allowance-{id}"))).child(hint(format!("Automatic cycles used: {used}/{limit}. Further automatic repair is blocked until you allow it.")));
            for (index, item) in allowances.iter().enumerate() {
                row = row.child(
                    hint(format!(
                        "{} · {}",
                        string(item, "grantedAt"),
                        string(item, "reason")
                    ))
                    .id(SharedString::from(format!("allowance-{index}"))),
                );
            }
            row = row
                .child(form.fields.render(
                    &format!("repair-{id}"),
                    "reason",
                    "Reason for one additional cycle",
                    self.busy,
                ))
                .child(
                    Button::new(SharedString::from(format!("allow-repair-{id}")))
                        .label("Allow one more cycle")
                        .disabled(
                            self.blocked()
                                || !self.observation.runtime_fresh
                                || reason.trim().is_empty(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({"reason":reason}), true, cx)
                        })),
                );
            panel = panel.child(row);
        }
        panel
    }
}
