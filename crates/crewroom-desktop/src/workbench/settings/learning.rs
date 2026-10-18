use super::*;
impl Workbench {
    pub(in crate::workbench) fn current_role_learning_draft(
        &self,
        id: &str,
        cx: &App,
    ) -> Option<Value> {
        let (target, mode) = self.settings.learning.as_ref()?;
        if target != id {
            return None;
        }
        let fields = &self
            .collaboration
            .forms
            .get(&format!("role-learning-{id}-{mode}"))?
            .fields;
        let mut role = records(&self.workspace, "roles")
            .into_iter()
            .find(|role| role["id"] == id)?;
        let change = fields.text("change", cx);
        role["instructions"] = json!(if mode == "teach" {
            format!("{}\n\n{}", string(&role, "instructions"), change.trim())
        } else {
            change
        });
        role["revision"] = json!(role["revision"].as_u64().unwrap_or(1));
        Some(role)
    }

    pub(super) fn role_learning(
        &mut self,
        role: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = string(role, "id");
        let mut panel=section("Improve this role").child(hint("Proposals require a separate decision in Learning. Active assignments keep their captured instructions."));
        let mut buttons = div().flex().flex_wrap().gap_2();
        for (mode, label) in [("ask", "Ask for a change"), ("teach", "Teach this role")] {
            let target = id.clone();
            buttons = buttons.child(
                Button::new(SharedString::from(format!("role-{mode}")))
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.learning = Some((target.clone(), mode.into()));
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(buttons);
        if let Some((target, mode)) = &self.settings.learning
            && target == &id
        {
            let mode = mode.clone();
            let key = format!("role-learning-{id}-{mode}");
            self.collaboration.form(
                &key,
                &json!({"change":if mode=="ask" {role["instructions"].clone()} else {json!("")}}),
                &[("change", true), ("rationale", true)],
                window,
                cx,
            );
            let fields = &self.collaboration.forms[&key].fields;
            panel = panel
                .child(hint(if mode == "ask" {
                    "Edit the exact replacement instructions to propose."
                } else {
                    "This guidance will be appended to the current instructions if approved."
                }))
                .child(fields.render(
                    "role-learning",
                    "change",
                    if mode == "ask" {
                        "Proposed instructions"
                    } else {
                        "Guidance to add"
                    },
                    self.busy,
                ))
                .child(fields.render(
                    "role-learning",
                    "rationale",
                    "Reason and operator evidence",
                    self.busy,
                ));
            let change = fields.text("change", cx);
            let rationale = fields.text("rationale", cx);
            let replacement = if mode == "teach" {
                format!("{}\n\n{}", string(role, "instructions"), change.trim())
            } else {
                change.clone()
            };
            let mut comparison_draft = role.clone();
            comparison_draft["revision"] = json!(role["revision"].as_u64().unwrap_or(1));
            comparison_draft["instructions"] = json!(replacement);
            panel = panel.child(self.comparison_entry(
                "role-learning",
                "role",
                &id,
                &comparison_draft,
                !change.trim().is_empty(),
                None,
                cx,
            ));
            let body = json!({"roleId":id,"revision":role["revision"].as_u64().unwrap_or(1),"mode":mode,"change":replacement,"rationale":rationale});
            panel = panel.child(
                Button::new("propose-role-learning")
                    .label("Submit proposal for decision")
                    .disabled(
                        self.blocked() || change.trim().is_empty() || rationale.trim().is_empty(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send("/learning".into(), body.clone(), true, cx)
                    })),
            );
        }
        panel = panel.child(
            Button::new("role-history")
                .label(if self.settings.history_open {
                    "Hide revision history"
                } else {
                    "Show revision history"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.history_open = !this.settings.history_open;
                    cx.notify();
                })),
        );
        if self.settings.history_open {
            let history = crewroom_core::role_history::history(role);
            for (i, entry) in history.iter().enumerate().rev() {
                let mut row = div()
                    .id(SharedString::from(format!(
                        "role-history-{}",
                        entry["revision"]
                    )))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(crate::accessibility::text(format!(
                        "Revision {} · {} · {}",
                        entry["revision"],
                        string(entry, "origin"),
                        entry["at"]
                            .as_str()
                            .unwrap_or("earlier history unavailable")
                    )));
                if let Some(proposal) = entry["proposalId"].as_str() {
                    row = row.child(hint(format!("Proposal: {proposal}")));
                }
                for diff in crewroom_core::role_history::diff(
                    i.checked_sub(1).map(|j| &history[j]["definition"]),
                    &entry["definition"],
                ) {
                    row = row.child(
                        div()
                            .id(SharedString::from(format!(
                                "diff-{}",
                                string(&diff, "field")
                            )))
                            .child(hint(format!(
                                "{}\nBefore: {}\nAfter: {}",
                                string(&diff, "field"),
                                diff["before"],
                                diff["after"]
                            ))),
                    );
                }
                panel = panel.child(row);
            }
        }
        panel
    }
    pub(super) fn specialist_drafting(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panel = section("New specialist").child(
            Button::new("describe-specialist")
                .label("Draft from a description")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.draft_open = !this.settings.draft_open;
                    cx.notify();
                })),
        );
        let available = self.workspace["capabilities"]["execution"] == true;
        if !available {
            panel = panel.child(hint(
                "Runner drafting is unavailable. Create agent role opens the manual form.",
            ));
        }
        if self.settings.draft_open {
            let fields = self.settings.draft_description.get_or_insert_with(|| {
                Fields::new(&json!({}), &[("description", true)], window, cx)
            });
            let description = fields.text("description", cx);
            panel=panel.child(fields.render("specialist-draft","description","Describe the specialty and when to use it",self.busy))
                .child(hint("Starts one recorded coordinator with a five-minute active limit. Its proposal stays editable; no role is saved automatically."))
                .child(Button::new("start-role-draft").label("Start drafting process").disabled(self.blocked()||!available||description.trim().is_empty()).on_click(cx.listener(move |this,_,_,cx|this.send("/role-drafts".into(),json!({"description":description}),true,cx))));
        }
        for run in records(&self.workspace, "runs")
            .into_iter()
            .filter(|r| r["roleDraft"].is_object())
            .rev()
        {
            let rid = string(&run, "id");
            let tid = string(&run, "taskId");
            let mut row = div()
                .id(SharedString::from(format!("role-draft-{rid}")))
                .flex()
                .flex_col()
                .gap_2()
                .child(crate::accessibility::text(format!(
                    "{} · {}",
                    string(&run["roleDraft"], "description"),
                    string(&run, "status")
                )))
                .child(hint(run["attentionReason"].as_str().unwrap_or(
                    "The recorded session retains process status, output and proposal.",
                )))
                .child(
                    Button::new(SharedString::from(format!("open-role-draft-{rid}")))
                        .label("Open drafting session")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(tid.clone());
                            this.panel = Panel::Ticket;
                            cx.notify();
                        })),
                );
            if let Some(proposal) = records(&self.workspace, "refinementProposals")
                .into_iter()
                .find(|p| p["id"] == run["roleDraft"]["proposalId"] && p["roleDraft"].is_object())
            {
                let pid = string(&proposal, "id");
                let key = format!("{NEW}-draft-{pid}");
                if records(&self.workspace, "roles")
                    .iter()
                    .any(|r| r["id"] == proposal["roleDraft"]["id"])
                {
                    row = row.child(hint(
                        "Role saved. Its revision history records this proposal.",
                    ));
                } else {
                    row = row.child(
                        Button::new(SharedString::from(format!("review-role-draft-{rid}")))
                            .label("Review and edit role proposal")
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.settings.roles.contains_key(&key) {
                                    let mut value = proposal["roleDraft"].clone();
                                    value["draftProposalId"] = json!(pid);
                                    let mut draft = RoleDraft::new(&value, window, cx);
                                    draft.original = Value::Null;
                                    this.settings.roles.insert(key.clone(), draft);
                                }
                                this.settings.role = key.clone();
                                this.settings.role_open = true;
                                cx.notify();
                            })),
                    );
                }
            }
            panel = panel.child(row);
        }
        panel
    }
}
