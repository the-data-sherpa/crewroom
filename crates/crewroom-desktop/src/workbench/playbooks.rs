use super::*;
impl Workbench {
    pub(super) fn run_playbook(
        &mut self,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if matches!(run["purpose"].as_str(), Some("explore" | "grill")) {
            return div();
        }
        let id = string(run, "id");
        let selected = &run["playbook"];
        let mut panel = section(format!(
            "Playbook · {}",
            selected["name"].as_str().unwrap_or("None selected")
        ))
        .child(hint(
            "Advisory checklist · readiness still requires revision-specific evidence.",
        ));
        if selected.is_object() {
            panel = panel.child(hint(format!(
                "Copied catalog revision {} · selection {}",
                selected["revision"], selected["selectionVersion"]
            )));
        }
        let mut steps = div()
            .id("run-playbook-steps")
            .flex()
            .flex_wrap()
            .gap_2()
            .max_h(px(160.))
            .overflow_y_scroll();
        for step in records(selected, "steps") {
            steps = steps.child(
                div()
                    .id(SharedString::from(format!(
                        "playbook-step-{}",
                        string(&step, "id")
                    )))
                    .p_2()
                    .border_1()
                    .border_color(rgb(0x293642))
                    .rounded_md()
                    .child(crate::accessibility::text(format!(
                        "{} · {}{}",
                        string(&step, "state"),
                        string(&step, "title"),
                        step["reason"]
                            .as_str()
                            .map(|r| format!(" · {r}"))
                            .unwrap_or_default()
                    ))),
            );
        }
        panel = panel.child(steps);
        if let Some(message) = records(&self.workspace, "messages")
            .iter()
            .find(|m| m["id"] == run["playbookSteeringId"])
        {
            panel = panel.child(hint(format!(
                "Playbook steering: {}",
                string(message, "delivery")
            )));
        }
        let closed = matches!(
            run["status"].as_str(),
            Some("closed" | "completed" | "cancelled")
        ) || run["desiredState"] == "cancelled";
        if closed {
            return panel;
        }
        let proposal = &run["playbookProposal"];
        if proposal["status"] == "pending" {
            let path = format!("/runs/{id}/playbook");
            let body = json!({"proposalId":proposal["id"],"revision":run["playbookVersion"].as_u64().unwrap_or(0),"reason":"Operator accepted the coordinator playbook proposal."});
            panel = panel
                .child(hint(format!(
                    "Coordinator proposes {}: {}",
                    string(&proposal["playbook"], "name"),
                    string(proposal, "rationale")
                )))
                .child(
                    Button::new("accept-playbook-proposal")
                        .label("Use proposed playbook")
                        .disabled(self.blocked())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), body.clone(), true, cx)
                        })),
                );
        }
        let rid = id.clone();
        panel = panel.child(
            Button::new("change-run-playbook")
                .label("Change playbook")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.planning.playbook_open.insert(rid.clone()) {
                        this.planning.playbook_open.remove(&rid);
                    }
                    cx.notify();
                })),
        );
        if self.planning.playbook_open.contains(&id) {
            let key = format!("/runs/{id}/playbook");
            self.collaboration
                .form(&key, &json!({}), &[("reason", true)], window, cx);
            let fields = &self.collaboration.forms[&key].fields;
            let reason = fields.text("reason", cx);
            panel = panel.child(fields.render(
                "run-playbook",
                "reason",
                "Reason for changing the checklist",
                self.busy,
            ));
            let mut catalog = records(&self.workspace, "playbooks");
            catalog.insert(0, json!({"id":"","name":"No playbook"}));
            for p in catalog {
                let pid = string(&p, "id");
                let body = json!({"playbookId":pid,"revision":run["playbookVersion"].as_u64().unwrap_or(0),"reason":reason});
                let path = key.clone();
                panel = panel.child(
                    Button::new(SharedString::from(format!("select-run-playbook-{pid}")))
                        .label(string(&p, "name"))
                        .disabled(self.blocked() || reason.trim().is_empty())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), body.clone(), true, cx)
                        })),
                );
            }
        }
        panel
    }
}
