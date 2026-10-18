use super::*;

fn refresh_label(item: &Value) -> &'static str {
    let workspace = &item["resolutionWorkspace"];
    if workspace["legacyOwnershipUnknown"] == true {
        "Legacy Git ownership unverified · operator inspection required"
    } else if !item["supersededBy"].is_null() {
        "Superseded · prior conflict retained"
    } else if item["state"] == "rebasing"
        || ["preparing", "continuing"].contains(&workspace["state"].as_str().unwrap_or(""))
    {
        "Git ownership unverified · operator inspection required"
    } else if item["holdReason"] == "interrupted_retry" {
        "Retry interrupted · operator decision required"
    } else if item["state"] == "completed"
        && item["fromBase"] == item["toBase"]
        && item["fromHead"] == item["toHead"]
    {
        "Current target verified · no rebase needed"
    } else if item["state"] == "conflicted" && workspace["state"] == "ready" {
        "Resolved · awaiting promotion"
    } else {
        match item["state"].as_str() {
            Some("completed") => "Rebased onto target",
            Some("conflicted") => "Conflicted · branch unchanged",
            Some("interrupted") => "Interrupted · branch unchanged",
            _ => "Recovery state unavailable",
        }
    }
}

impl Workbench {
    pub(super) fn recovery_panel(
        &mut self,
        run: &Value,
        result: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let run_id = string(run, "id");
        let mut panel = div().flex().flex_col().gap_4();
        let refreshes = records(result, "refreshes");
        let hold = refreshes
            .iter()
            .rev()
            .find(|i| i["state"] == "conflicted" && i["supersededBy"].is_null());
        let retry = hold.is_some_and(|i| {
            i["resolutionWorkspace"].is_null()
                || i["resolutionWorkspace"]["state"] == "operator_required"
        });
        let mut history = section("Target refresh history");
        if refreshes.is_empty() {
            history = history.child(hint("No target refresh recorded."));
        }
        for item in &refreshes {
            let state = item["resolutionWorkspace"]["state"].as_str().unwrap_or("");
            let mut row = section(refresh_label(item))
                .id(SharedString::from(format!(
                    "refresh-{}",
                    value(&item["requestId"])
                )))
                .child(hint(format!(
                    "Recorded {}\nBase {} → {} · head {} → {}",
                    value(&item["at"]),
                    value(&item["fromBase"]),
                    value(&item["toBase"]),
                    value(&item["fromHead"]),
                    value(&item["toHead"])
                )));
            if item["resolutionWorkspace"].is_object() {
                row = row.child(hint(format!(
                    "Retained isolated workspace: {}",
                    value(&item["resolutionWorkspace"]["path"])
                )));
            }
            if item["state"] == "rebasing"
                || ["preparing", "continuing", "promoting"].contains(&state)
                || item["resolutionWorkspace"]["legacyOwnershipUnknown"] == true
            {
                row=row.child(hint("Git ownership or promotion is unverified. Keep agents stopped and retain the branch and isolated workspace for inspection. The attempted target is not a verified candidate."));
            }
            if !records(item, "conflicts").is_empty() {
                row = row.child(hint(format!(
                    "{}: {}",
                    if state == "ready" {
                        "Resolved paths"
                    } else {
                        "Conflicts"
                    },
                    text_lines(&item["conflicts"])
                )));
            }
            if item["state"] == "conflicted" {
                row=row.child(hint(match state {
                "conflicted"=>"Resolution workspace retained. Resume explicitly lets the coordinator assign an in-scope specialist. The original branch and PR stay unchanged until promotion.",
                "operator_required"=>"Operator decision required: scope, project instructions, target or Git state changed. The original branch and PR remain unchanged.",
                "ready"=>"The isolated rebase is ready. Implementation completion must create a new candidate with its own checks and review.",
                ""=>"A reasoned retry is required to verify the current target. Earlier candidates do not establish current-target readiness.",
                _=>"Inspect the recorded ownership state before taking further action.",
            }));
            }
            if item["holdReason"] == "interrupted_retry" {
                row=row.child(hint("This retry stopped before target verification. Keep agents stopped and give a reason for a new attempt. No candidate was created by this retry."));
            }
            if item["state"] == "completed"
                && item["fromBase"] == item["toBase"]
                && item["fromHead"] == item["toHead"]
            {
                row=row.child(hint("The target was unchanged. The existing candidate was retained. This verification did not resume the run or change evidence gates."));
            }
            if !item["supersededBy"].is_null() {
                row=row.child(hint(format!("A newer attempt superseded this hold: {}. The prior conflict and any isolated workspace remain in history.",value(&item["supersededBy"]["reason"]))));
            }
            for completion in records(result, "completions")
                .iter()
                .filter(|c| c["conflictRefreshId"] == item["requestId"] && c["state"] == "rejected")
            {
                let r = &completion["rejection"];
                row=row.child(hint(format!("Conflict completion rejected: {} · {}\nTarget {} → {} · observed {}. Resolved files are retained; no candidate was created. This completion cannot be replayed.",value(&r["code"]),value(&r["reason"]),value(&r["expectedTarget"]),value(&r["observedTarget"]),value(&r["at"]))).id(SharedString::from(format!("completion-{}", value(&completion["id"])))));
            }
            history = history.child(row);
        }
        if result["refreshable"] == true {
            let path = format!("/runs/{run_id}/target-refresh");
            let key = format!(
                "{path}/{}",
                hold.map(|h| value(&h["requestId"]))
                    .unwrap_or_else(|| "current".into())
            );
            let mut payload = json!({});
            let mut reason_missing = false;
            if retry {
                self.collaboration
                    .form(&key, &json!({}), &[("reason", true)], window, cx);
                let form = &self.collaboration.forms[&key];
                let reason = form.fields.text("reason", cx);
                history = history.child(form.fields.render(
                    "target-retry",
                    "reason",
                    "Reason for a fresh target attempt",
                    self.busy,
                ));
                reason_missing = reason.trim().is_empty();
                payload["operatorDecision"] =
                    json!({"conflictRequestId":hold.unwrap()["requestId"],"reason":reason.trim()});
            }
            history=history.child(hint("Verifies the current target and rebases only when needed. A new candidate needs its own checks and review. Prior conflict workspaces remain in history."))
                .child(Button::new("target-refresh").label(if retry{"Retry from current target"}else{"Refresh from target"}).disabled(self.blocked()||!self.observation.result_fresh||!self.observation.runtime_fresh||reason_missing).on_click(cx.listener(move|this,_,_,cx|this.send(path.clone(),payload.clone(),true,cx))))
                .children(crate::workbench::blockers::line("target-refresh-blockers", if retry {"retry from target"} else {"refresh from target"}, &crate::workbench::blockers::target_refresh(self.observation.result_fresh, self.observation.runtime_fresh, reason_missing)));
        }
        panel = panel.child(history);
        let changes = records(result, "checkPolicyChanges");
        if !changes.is_empty() {
            let mut history = section("Check environment changes");
            for change in changes {
                history = history.child(
                    hint(format!(
                        "{} · policy revision {}\n{}\nImage {} → {}\nMemory {} → {} MiB",
                        value(&change["checkId"]),
                        value(&change["revision"]),
                        value(&change["reason"]),
                        value(&change["expectedImage"]),
                        value(&change["image"]),
                        value(&change["expectedMemoryMiB"]),
                        value(&change["memoryMiB"])
                    ))
                    .id(SharedString::from(format!(
                        "policy-change-{}",
                        value(&change["revision"])
                    ))),
                );
            }
            panel = panel.child(history);
        }
        if result["checkEnvironmentEditable"] == true {
            panel = panel.child(self.check_correction(run, result, window, cx));
        }
        panel
    }
    fn check_correction(
        &mut self,
        run: &Value,
        result: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let path = format!("/runs/{}/check-environment", string(run, "id"));
        let mut panel=section("Correct a check environment").child(hint("Only the image and memory limit can change. The check command and required gates stay fixed. Earlier receipts remain in history; affected checks and review must be repeated."));
        for definition in records(result, "requiredChecks") {
            let id = string(&definition, "id");
            let key = format!("{path}/{id}");
            self.collaboration.form(&key,&json!({"image":definition["image"],"memory":definition["memoryMiB"].as_u64().unwrap_or(1024),"reason":""}),&[("image",false),("memory",false),("reason",true)],window,cx);
            let form = &self.collaboration.forms[&key];
            let image = form.fields.text("image", cx);
            let memory = form.fields.text("memory", cx).trim().parse::<u64>().ok();
            let reason = form.fields.text("reason", cx);
            let original_image = form.original["image"].as_str().unwrap_or("");
            let original_memory = form.original["memory"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1024);
            let changed = image != original_image || memory != Some(original_memory);
            let payload = json!({"checkId":id,"expectedImage":original_image,"image":image,"expectedMemoryMiB":original_memory,"memoryMiB":memory,"reason":reason.trim()});
            let send_path = path.clone();
            let discard = key.clone();
            let mut row = section(format!("Check {id}"))
                .id(SharedString::from(format!("check-environment-{id}")))
                .child(hint(format!(
                    "Command: {} {}",
                    value(&definition["command"]),
                    value(&definition["args"])
                )));
            for (field, label) in [
                ("image", "Pinned image SHA-256 (sha256:…)"),
                ("memory", "Memory limit (MiB, 128–8192)"),
                ("reason", "Reason for environment correction"),
            ] {
                row = row.child(form.fields.render(
                    &format!("check-env-{id}"),
                    field,
                    label,
                    self.busy,
                ));
            }
            row = row
                .child(
                    Button::new(SharedString::from(format!("correct-check-{id}")))
                        .label("Save check environment")
                        .disabled(
                            self.blocked()
                                || !self.observation.result_fresh
                                || !self.observation.runtime_fresh
                                || !changed
                                || reason.trim().is_empty()
                                || image.trim().is_empty()
                                || !memory.is_some_and(|m| (128..=8192).contains(&m)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(send_path.clone(), payload.clone(), true, cx)
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("reload-check-{id}")))
                        .label("Discard draft and reload current environment")
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.collaboration.forms.remove(&discard);
                            cx.notify();
                        })),
                );
            panel = panel.child(row);
        }
        panel
    }
}
