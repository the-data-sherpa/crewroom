use super::*;

impl Workbench {
    pub(in crate::workbench) fn insertion(
        &mut self,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = string(run, "id");
        let path = format!("/runs/{id}/specialists");
        self.collaboration
            .form(&path, &json!({}), &[("input", true)], window, cx);
        let roles: Vec<_> = records(&self.workspace, "roles")
            .into_iter()
            .filter(|r| {
                r["enabled"] == true
                    && !(run["purpose"] == "research"
                        && ["implementer", "reviewer"].contains(&r["id"].as_str().unwrap_or("")))
            })
            .collect();
        let assignments = records(&self.observation.runtime, "assignments");
        let proposals = records(&self.observation.runtime, "teamProposals");
        let mut team = if assignments.is_empty() {
            records(&run["launch"], "team")
        } else {
            assignments.clone()
        };
        if team.is_empty() {
            team.push(json!({"name":"Coordinator","coordinator":true}));
        }
        let reason = if ended(run) {
            Some("This run has ended. Specialist insertion is unavailable.")
        } else if self.workspace["capabilities"]["execution"] != true {
            Some("Runner integration is unavailable; specialist models cannot be validated.")
        } else if !self.observation.runtime_fresh {
            Some("Wait for a current run observation before proposing a specialist.")
        } else if proposals.iter().any(|p| p["status"] == "pending") {
            Some("Wait until the coordinator admits or rejects the pending proposal.")
        } else if team
            .iter()
            .filter(|a| a["coordinator"] != true && a["retiredAt"].is_null())
            .count()
            >= 3
        {
            Some(
                "Three specialists are assigned. The coordinator must retire a responsibility before another can be added.",
            )
        } else if roles.is_empty() {
            Some("No enabled specialist roles are available.")
        } else {
            None
        };
        let blocked = self.blocked() || reason.is_some();
        let form = self.collaboration.forms.get_mut(&path).unwrap();
        if form.role.is_empty() {
            form.role = roles.first().map(|r| string(r, "id")).unwrap_or_default();
        }
        let selected = form.role.clone();
        let choices = form.choices.clone();
        let input = form.fields.text("input", cx);
        let mut panel=section("Add a specialist").child(hint(reason.unwrap_or(if run["desiredState"]=="paused" || run["status"]=="paused" {"Save a proposal now. The coordinator decides after Resume, at a safe coordination point."}else{"Save a proposal. The coordinator decides at the next safe coordination point."})))
            .child(hint("Readiness waits until the coordinator closes the assignment. Changed code requires renewed checks and review."));
        let mut role_buttons = div().flex().flex_wrap().gap_2();
        for role in &roles {
            let role_id = string(role, "id");
            let key = path.clone();
            role_buttons = role_buttons.child(
                Button::new(SharedString::from(format!("specialist-role-{role_id}")))
                    .label(string(role, "name"))
                    .selected(selected == role_id)
                    .disabled(blocked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.collaboration.forms.get_mut(&key).unwrap().role = role_id.clone();
                        cx.notify();
                    })),
            );
        }
        panel = panel
            .child(crate::accessibility::text("Specialist role"))
            .child(role_buttons);
        if let Some(role) = roles.iter().find(|role| role["id"] == selected) {
            panel = panel.child(hint(string(role, "purpose")));
        }
        panel = panel.child(self.role_model_preview(&selected));
        panel = panel
            .child(
                Button::new("specialist-manage-agent-roles")
                    .label("Configure agent roles")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Panel::Roles;
                        cx.notify();
                    })),
            )
            .child(self.collaboration.forms[&path].fields.render(
                "specialist",
                "input",
                "Specialist input",
                blocked,
            ))
            .child(crate::accessibility::text("Dependencies (optional)"));
        for item in &team {
            let name = string(item, "name");
            let choice = format!("dependency:{name}");
            let key = path.clone();
            panel = panel.child(
                Checkbox::new(SharedString::from(format!("specialist-{choice}")))
                    .label(name)
                    .checked(choices.contains(&choice))
                    .disabled(blocked)
                    .on_click(cx.listener(move |this, checked, _, cx| {
                        let choices = &mut this.collaboration.forms.get_mut(&key).unwrap().choices;
                        if *checked {
                            choices.insert(choice.clone());
                        } else {
                            choices.remove(&choice);
                        }
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(crate::accessibility::text(
            "Accepted criteria (optional for coding)",
        ));
        let accepted = records(&run["taskSnapshot"], "criteria");
        for (index, criterion) in accepted.iter().enumerate() {
            let choice = format!("criterion:{index}");
            let key = path.clone();
            panel = panel.child(
                Checkbox::new(SharedString::from(format!("specialist-{choice}")))
                    .label(criterion.as_str().unwrap_or("").to_owned())
                    .checked(choices.contains(&choice))
                    .disabled(blocked)
                    .on_click(cx.listener(move |this, checked, _, cx| {
                        let choices = &mut this.collaboration.forms.get_mut(&key).unwrap().choices;
                        if *checked {
                            choices.insert(choice.clone());
                        } else {
                            choices.remove(&choice);
                        }
                        cx.notify();
                    })),
            );
        }
        let dependencies: Vec<_> = team
            .iter()
            .map(|a| string(a, "name"))
            .filter(|name| choices.contains(&format!("dependency:{name}")))
            .collect();
        let criteria: Vec<_> = (0..accepted.len())
            .filter(|i| choices.contains(&format!("criterion:{i}")))
            .collect();
        let mut body =
            json!({"roleId":selected,"input":input,"dependsOn":dependencies,"criteria":criteria});
        self.capture_model_preview(&mut body, &selected);
        panel = panel.child(
            Button::new("propose-specialist")
                .primary()
                .label("Propose specialist")
                .disabled(
                    blocked
                        || input.trim().is_empty()
                        || !roles.iter().any(|r| r["id"] == selected),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(path.clone(), body.clone(), true, cx)
                })),
        );
        for proposal in proposals.iter().rev() {
            let status = match proposal["status"].as_str().unwrap_or("") {
                "pending" => "Proposed · not started",
                "rejected" => "Rejected · not admitted",
                "applied" => {
                    let ids = records(proposal, "assignmentIds");
                    if !ids.is_empty()
                        && ids.iter().all(|id| {
                            assignments
                                .iter()
                                .any(|a| a["id"] == *id && !a["retiredAt"].is_null())
                        })
                    {
                        "Assignment closed · see current readiness"
                    } else {
                        "Assignment admitted · awaiting coordinator closure"
                    }
                }
                _ => "Proposal status unavailable",
            };
            let mut row = section(status)
                .id(SharedString::from(format!(
                    "team-proposal-{}",
                    string(proposal, "id")
                )))
                .child(hint(if proposal["origin"] == "operator" {
                    "You proposed this specialist."
                } else {
                    "The coordinator proposed this specialist."
                }));
            for (index, specialist) in records(proposal, "specialists").iter().enumerate() {
                let specialist_row = div()
                    .id(SharedString::from(format!("specialist-{index}")))
                    .child(crate::accessibility::text(format!(
                        "{} · {}",
                        string(specialist, "name"),
                        string(&specialist["role"], "name")
                    )))
                    .child(crate::accessibility::text(string(specialist, "input")))
                    .child(hint(format!(
                        "Depends on: {}",
                        text_list(&records(&specialist["brief"], "dependsOn"))
                    )));
                row = row.child(specialist_row);
            }
            if proposal["status"] == "pending" {
                row = row.child(hint("No process exists for this proposal yet."));
            }
            if let Some(reason) = proposal["reason"].as_str() {
                row = row.child(hint(reason.to_owned()));
            }
            panel = panel.child(row);
        }
        panel
    }
}
