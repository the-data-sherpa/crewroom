use super::*;

impl Workbench {
    pub(in crate::workbench) fn refinement_controls(
        &mut self,
        task: &Value,
        run: Option<&Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if run.is_some_and(|r| r["intakeId"].is_string() && r["purpose"] == "grill") {
            return div();
        }
        let task_id = string(task, "id");
        let open = records(&self.workspace, "runs")
            .iter()
            .any(|r| r["taskId"] == task["id"] && !ended(r));
        let dirty = self.brief_dirty(task, cx);
        let runner = self.workspace["capabilities"]["execution"] == true;
        let mut panel = section("Explore, Grill and research");
        if !open {
            panel = panel.child(hint("Explore develops the request. Grill challenges its assumptions. Both keep decisions separate until you accept a proposal."));
            let mut actions = div().flex().flex_wrap().gap_2();
            for (purpose, label) in [("explore", "Explore"), ("grill", "Grill")] {
                let id = task_id.clone();
                let revision = task["revision"].clone();
                actions = actions.child(
                    Button::new(purpose)
                        .label(label)
                        .disabled(
                            self.blocked() || dirty || task["needsReacceptance"] == true || !runner,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                format!("/tasks/{id}/refinement-runs"),
                                json!({"revision":revision,"purpose":purpose}),
                                true,
                                cx,
                            )
                        })),
                );
            }
            if !runner {
                panel = panel.child(hint(
                    "Runner not configured. Explore and Grill cannot start.",
                ));
            }
            let retired = records(&self.workspace, "repositories")
                .iter()
                .any(|r| r["id"] == task["repositoryId"] && !r["retiredAt"].is_null());
            let blocked = dirty
                || task["needsReacceptance"] == true
                || string(task, "brief").trim().is_empty()
                || records(task, "criteria").is_empty()
                || retired;
            let id = task_id.clone();
            let revision = task["revision"].clone();
            actions = actions.child(
                Button::new("queue-research")
                    .label("Queue research")
                    .disabled(self.blocked() || blocked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(
                            format!("/tasks/{id}/research-runs"),
                            json!({"revision":revision}),
                            true,
                            cx,
                        )
                    })),
            );
            panel=panel.child(actions).child(hint("Research needs an accepted brief and criteria. A repository is optional; a selected repository must be active. Queueing does not start an agent."));
        }
        if let Some(run) = run {
            let id = string(run, "id");
            let purpose = run["purpose"].as_str().unwrap_or("coding");
            if ["explore", "grill"].contains(&purpose) && !ended(run) {
                let start = id.clone();
                if run["status"] == "awaiting_runner"
                    && records(&self.observation.runtime, "assignments").is_empty()
                {
                    panel = panel.child(
                        Button::new("start-refinement")
                            .label("Start refinement coordinator")
                            .disabled(self.blocked() || !runner || !self.observation.runtime_fresh)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(format!("/runs/{start}/start"), json!({}), false, cx)
                            })),
                    );
                }
                // Before the coordinator is admitted there is no agent to stop.
                let started = !records(&self.observation.runtime, "assignments").is_empty();
                panel=panel.child(Button::new("close-refinement").label("Close refinement run…").disabled(self.blocked())
                    .on_click(cx.listener(move |this,_,window,cx|this.confirm_send(Confirmation{title:"Close this refinement run?",description:format!("{} Proposals and notes are kept. A closed run cannot reopen.",if started {"The agent stops."} else {"No agent has started."}),keep:"Keep running",confirm:"Close run"},format!("/runs/{id}/close"),json!({}),false,window,cx))))
                    .child(hint("Closing stops this refinement run and retains its proposals and notes. Accepting a proposal is a separate decision."));
            } else if purpose == "research"
                && run["status"] == "awaiting_runner"
                && !ended(run)
                && records(&self.observation.runtime, "assignments").is_empty()
            {
                let path = format!("/runs/{id}/start");
                self.collaboration.form(&path, &json!({}), &[], window, cx);
                let roles: Vec<_> = records(&self.workspace, "roles")
                    .into_iter()
                    .filter(|r| {
                        r["enabled"] == true
                            && !["implementer", "reviewer"]
                                .contains(&r["id"].as_str().unwrap_or(""))
                    })
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
                let role = form.role.clone();
                let mut choices = div().flex().flex_wrap().gap_2();
                for item in &roles {
                    let role_id = string(item, "id");
                    let key = path.clone();
                    choices = choices.child(
                        Button::new(SharedString::from(format!("research-role-{role_id}")))
                            .label(string(item, "name"))
                            .selected(role == role_id)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.collaboration.forms.get_mut(&key).unwrap().role =
                                    role_id.clone();
                                cx.notify();
                            })),
                    );
                }
                let input=json!({"brief":run["taskSnapshot"]["brief"],"criteria":run["taskSnapshot"]["criteria"]}).to_string();
                panel=panel.child(crate::accessibility::text("Research coordinator role")).child(choices).child(Button::new("start-research").primary().label("Start research coordinator")
                    .disabled(self.blocked() || !runner || !self.observation.runtime_fresh || !roles.iter().any(|r|r["id"]==role))
                    .on_click(cx.listener(move |this,_,_,cx|this.send(path.clone(),json!({"team":[{"name":"Coordinator","roleId":role,"coordinator":true,"input":input}]}),false,cx))))
                    .child(hint(if runner {"Starting admits the selected coordinator for this research run."}else{"Runner not configured. The research plan remains queued."}));
            }
        }
        panel
    }

    pub(in crate::workbench) fn adoption(
        &mut self,
        task: &Value,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let snapshot = &run["taskSnapshot"];
        if (ended(run) && run["status"] != "completed")
            || (snapshot["brief"] == task["brief"]
                && snapshot["criteria"] == task["criteria"]
                && snapshot["sourceRevision"] == task["sourceRevision"])
        {
            return div();
        }
        let path = format!("/runs/{}/adopt-requirements", string(run, "id"));
        self.collaboration
            .form(&path, &json!({}), &[("reason", false)], window, cx);
        let form = &self.collaboration.forms[&path];
        let reason = form.fields.text("reason", cx);
        let claimed = records(&self.observation.runtime, "attempts")
            .iter()
            .any(|a| a["state"] != "admitted");
        let blockers = crate::workbench::blockers::adopt(
            self.observation.runtime_fresh,
            self.brief_dirty(task, cx),
            crate::workbench::blockers::reacceptance(task),
            claimed && reason.trim().is_empty(),
        );
        let blocked = self.blocked() || !blockers.is_empty();
        let revision = task["revision"].clone();
        let mut body = json!({"revision":revision});
        if !reason.trim().is_empty() {
            body["reason"] = json!(reason.trim());
        }
        section("Requirement adoption")
            .child(hint(format!("This run retains revision {}. Accepted revision {} awaits your decision.",run["taskRevision"],task["revision"])))
            .children(adoption_changes(snapshot, task).into_iter().enumerate().map(|(i, line)| div().id(("adoption-change", i)).child(hint(line))))
            .child(hint(if claimed {"Explain the change before adopting it into active work. Changed scope can require new checks and review."}else{"Queued work can adopt the accepted revision. Saving a brief alone does not change this run."}))
            .child(form.fields.render("adoption","reason","Adoption reason",self.busy))
            .child(Button::new("adopt-requirements").primary().label(format!("Adopt revision {revision}")).disabled(blocked)
                .on_click(cx.listener(move |this,_,_,cx|this.send(path.clone(),body.clone(),true,cx))))
            .children(crate::workbench::blockers::line("adoption-blockers", "adopt", &blockers))
    }

    pub(in crate::workbench) fn refinement_proposals(
        &mut self,
        task: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let proposals: Vec<_> = records(&self.workspace, "refinementProposals")
            .into_iter()
            .filter(|p| p["taskId"] == task["id"])
            .collect();
        let notes: Vec<_> = records(&self.workspace, "notes")
            .into_iter()
            .filter(|n| n["taskId"] == task["id"])
            .collect();
        if proposals.is_empty() && notes.is_empty() {
            return div();
        }
        let mut panel = section("Explore & Grill history");
        for proposal in proposals.iter().rev() {
            if proposal["roleDraft"].is_object() {
                panel=panel.child(hint("A specialist role proposal is recorded. Review and edit it in Settings → Agent roles."));
                continue;
            }
            let id = string(proposal, "id");
            let path = format!("/refinement-proposals/{id}/decision");
            let mut row = section(format!("Proposal · {}", string(proposal, "status")))
                .id(SharedString::from(format!("refinement-proposal-{id}")))
                .child(hint(format!(
                    "Accepted base {} · source revision {}",
                    proposal["baseRevision"], proposal["sourceRevision"]
                )))
                .child(crate::accessibility::text(string(proposal, "brief")))
                .child(crate::accessibility::text(text_list(&records(
                    proposal, "criteria",
                ))))
                .child(hint(format!(
                    "Decisions: {}",
                    text_list(&records(proposal, "decisions"))
                )))
                .child(hint(format!(
                    "Unresolved questions: {}",
                    text_list(&records(proposal, "unresolvedQuestions"))
                )));
            if proposal["status"] == "pending" {
                self.collaboration.form(
                    &path,
                    proposal,
                    &[("brief", true), ("criteria", true), ("reason", false)],
                    window,
                    cx,
                );
                let form = &self.collaboration.forms[&path];
                let edit = form.edit;
                let key = path.clone();
                row = row.child(
                    Checkbox::new(SharedString::from(format!("edit-{id}")))
                        .label("Edit before accepting")
                        .checked(edit)
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, checked, _, cx| {
                            this.collaboration.forms.get_mut(&key).unwrap().edit = *checked;
                            cx.notify();
                        })),
                );
                if edit {
                    row = row
                        .child(
                            form.fields
                                .render(&id, "brief", "Proposed brief", self.busy),
                        )
                        .child(
                            form.fields
                                .render(&id, "criteria", "Proposed criteria", self.busy),
                        );
                }
                let reason = form.fields.text("reason", cx);
                let brief = form.fields.text("brief", cx);
                let criteria = criteria(form.fields.text("criteria", cx));
                row = row.child(form.fields.render(
                    &id,
                    "reason",
                    "Decision reason (required to discard)",
                    self.busy,
                ));
                let stale = task["revision"] != proposal["baseRevision"]
                    || task["sourceRevision"] != proposal["sourceRevision"]
                    || task["needsReacceptance"] == true;
                if stale {
                    row = row.child(hint(
                        "Proposal base is stale. Request a new proposal before accepting.",
                    ));
                }
                let mut controls = div().flex().flex_wrap().gap_2();
                for (decision, label) in [
                    ("accept", "Accept proposal"),
                    ("discard", "Discard proposal"),
                ] {
                    let mut body = json!({"decision":decision});
                    if !reason.trim().is_empty() {
                        body["reason"] = json!(reason.trim());
                    }
                    if decision == "accept" && edit {
                        body["brief"] = json!(brief);
                        body["criteria"] = json!(criteria);
                    }
                    let key = path.clone();
                    let disabled = self.blocked()
                        || if decision == "discard" {
                            reason.trim().is_empty()
                        } else {
                            stale
                                || self.brief_dirty(task, cx)
                                || (edit && (brief.trim().is_empty() || criteria.is_empty()))
                        };
                    controls = controls.child(
                        Button::new(SharedString::from(format!("{decision}-{id}")))
                            .label(label)
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(key.clone(), body.clone(), true, cx)
                            })),
                    );
                }
                row = row.child(controls);
            } else {
                row = row.child(hint(format!(
                    "Decision revision {} · {}",
                    proposal["decision"]["revision"],
                    string(&proposal["decision"], "reason")
                )));
                row = row.child(self.decided_draft(&path, cx));
            }
            panel = panel.child(row);
        }
        let mut saved = section("Saved notes");
        for note in notes.iter().rev() {
            saved = saved.child(
                hint(format!(
                    "{} · revision {} · {}",
                    string(note, "mode"),
                    note["revision"],
                    string(note, "body")
                ))
                .id(SharedString::from(format!(
                    "saved-note-{}",
                    string(note, "id")
                ))),
            );
        }
        panel.child(saved)
    }
}

/// What adopting moves the run from and to: the brief, each changed criterion, and
/// the source revision (named for its provider), so a criteria-only or source-only change is visible.
fn adoption_changes(snapshot: &Value, task: &Value) -> Vec<String> {
    let mut lines = vec![];
    let brief_changed = snapshot["brief"] != task["brief"];
    if brief_changed {
        lines.push(format!("Captured brief: {}", string(snapshot, "brief")));
        lines.push(format!("Accepted brief: {}", string(task, "brief")));
    }
    let criteria =
        super::criteria_changes(&records(snapshot, "criteria"), &records(task, "criteria"));
    let source = (snapshot["sourceRevision"] != task["sourceRevision"]).then(|| {
        format!(
            "{} source revision {} → {}",
            crewroom_core::ticket_source::source_name(task),
            snapshot["sourceRevision"],
            task["sourceRevision"]
        )
    });
    match (brief_changed || !criteria.is_empty(), source) {
        (false, Some(source)) => lines.push(format!("Brief and criteria unchanged; {source}.")),
        (changed, source) => {
            if changed && !brief_changed {
                lines.push("Brief unchanged.".into());
            }
            lines.extend(criteria);
            lines.extend(source.map(|s| format!("{s}.")));
        }
    }
    lines
}
