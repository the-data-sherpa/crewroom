//! "Needs you": the service's read projection of pending operator decisions.
//! Actions post to the same endpoints, with the same request keys and form drafts, as the
//! screens that already own them, so the service applies the same checks.
use super::*;
use gpui_kit::Stateful;

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "repair" => "Repair limit",
        "learning" => "Learning proposal",
        "requirements" => "Requirement change",
        "feedback" => "PR feedback",
        "publication" => "Publication outcome",
        "jira" => "Jira outcome",
        "github" => "GitHub issue update",
        "ownership" => "Process ownership",
        _ => "Decision",
    }
}

impl Workbench {
    /// `None` when the service has no execution layer and cannot project decisions.
    pub(super) fn needs_you(&self) -> Option<Vec<Value>> {
        self.workspace["needsYou"].as_array().cloned()
    }

    fn open_session(&mut self, task: &Value, tab: &'static str, cx: &mut Context<Self>) {
        let Some(id) = task.as_str() else { return };
        self.selected = Some(id.to_owned());
        self.panel = Panel::Ticket;
        self.tab = tab;
        self.navigation_open = false;
        cx.notify();
    }

    fn needs_reason(
        &mut self,
        path: &str,
        prefix: &str,
        label: &'static str,
        required: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Div, String) {
        self.collaboration
            .form(path, &json!({}), &[("reason", required)], window, cx);
        let form = &self.collaboration.forms[path];
        (
            form.fields.render(prefix, "reason", label, self.busy),
            form.fields.text("reason", cx).trim().to_owned(),
        )
    }

    fn needs_item(
        &mut self,
        item: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = string(item, "id");
        let run = string(item, "runId");
        let kind = string(item, "kind");
        let task = records(&self.workspace, "tasks")
            .into_iter()
            .find(|t| t["id"] == item["taskId"]);
        let key = task
            .as_ref()
            .map(|t| string(t, "key"))
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "No ticket".into());
        let mut card = div()
            .id(SharedString::from(format!("needs-{id}")))
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(rgb(0x17212a))
            .border_1()
            .border_l_4()
            .border_color(rgb(match kind.as_str() {
                "repair" | "ownership" | "publication" | "jira" | "github" => 0xf1bf73,
                "learning" => 0x9fb7ff,
                _ => 0x84d7bc,
            }))
            .child(hint(format!("{} · {key}", kind_label(&kind))).text_xs())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::accessibility::named(
                        format!("needs-title-{id}"),
                        string(item, "title"),
                    )),
            );
        if let Some(detail) = item["detail"].as_str().filter(|d| !d.is_empty()) {
            card = card.child(crate::accessibility::text(detail.to_owned()));
        }
        let mut actions = div().flex().flex_wrap().gap_2();
        let blocked = self.blocked();
        match kind.as_str() {
            "repair" => {
                let path = format!(
                    "/runs/{run}/assignments/{}/repair-allowance",
                    string(item, "assignmentId")
                );
                let (field, reason) = self.needs_reason(
                    &path,
                    &format!("needs-repair-{id}"),
                    "Reason for one additional cycle",
                    true,
                    window,
                    cx,
                );
                card = card.child(field);
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-allow-{id}")))
                        .primary()
                        .label("Allow one more cycle")
                        .disabled(blocked || reason.is_empty())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({"reason":reason}), true, cx)
                        })),
                );
            }
            "learning" => {
                let path = format!("/learning/{}/decision", string(item, "learningId"));
                card = card
                    .child(hint("Proposed text"))
                    .child(crate::accessibility::text(string(item, "change")));
                if item["retry"] == true {
                    let mut body = item["decision"].clone();
                    body["decision"] = json!("approve");
                    actions = actions.child(
                        Button::new(SharedString::from(format!("needs-retry-{id}")))
                            .primary()
                            .label("Retry saving the approved version")
                            .disabled(blocked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(path.clone(), body.clone(), false, cx)
                            })),
                    );
                } else {
                    let (field, reason) = self.needs_reason(
                        &path,
                        &format!("needs-learning-{id}"),
                        "Decision reason (recorded with the decision)",
                        true,
                        window,
                        cx,
                    );
                    card = card.child(field).child(hint(
                        "Approval creates a new version for future assignments. Existing assignments keep their captured instructions.",
                    ));
                    for (decision, label) in [("approve", "Approve"), ("reject", "Reject")] {
                        let path = path.clone();
                        let reason = reason.clone();
                        let mut button =
                            Button::new(SharedString::from(format!("needs-{decision}-{id}")))
                                .label(label)
                                .disabled(blocked || reason.is_empty())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send(
                                        path.clone(),
                                        json!({"decision":decision,"reason":reason}),
                                        true,
                                        cx,
                                    )
                                }));
                        if decision == "approve" {
                            button = button.primary();
                        }
                        actions = actions.child(button);
                    }
                }
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-roles-{id}")))
                        .ghost()
                        .label("Open learning history")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel = Panel::Learning;
                            cx.notify();
                        })),
                );
            }
            "requirements" if item["stage"] == "adopt" => {
                let path = format!("/runs/{run}/adopt-requirements");
                let required = item["reasonRequired"] == true;
                let (field, reason) = self.needs_reason(
                    &path,
                    &format!("needs-adopt-{id}"),
                    "Adoption reason",
                    false,
                    window,
                    cx,
                );
                let dirty = task.as_ref().is_some_and(|t| self.brief_dirty(t, cx));
                card = card.child(hint(format!(
                    "The run retains revision {}. {}",
                    item["retainedRevision"],
                    if required {
                        "Active work has started: explain the change before adopting it. Changed scope can require new checks and review."
                    } else {
                        "Queued work can adopt the accepted revision."
                    }
                )));
                if dirty {
                    card = card.child(hint(
                        "This ticket has an unsaved brief draft. Save or discard it before adopting.",
                    ));
                }
                card = card.child(field);
                let mut body = json!({"revision":item["revision"]});
                if !reason.is_empty() {
                    body["reason"] = json!(reason);
                }
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-adopt-{id}")))
                        .primary()
                        .label(format!("Adopt revision {}", item["revision"]))
                        .disabled(blocked || dirty || (required && reason.is_empty()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), body.clone(), true, cx)
                        })),
                );
            }
            "requirements" => {
                let task_id = item["taskId"].clone();
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-brief-{id}")))
                        .primary()
                        .label("Review brief")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_session(&task_id, "Brief", cx)
                        })),
                );
            }
            "feedback" => {
                let path = format!("/runs/{run}/resume");
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-resume-{id}")))
                        .primary()
                        .label("Resume run")
                        .disabled(blocked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({}), false, cx)
                        })),
                );
                card = card.child(hint(
                    "Resume waits for the coding slot. Comments are listed in the session's Results.",
                ));
            }
            "publication" | "jira" => {
                let available = item["available"] == true;
                let path = if kind == "jira" {
                    "/jira/reconcile".to_owned()
                } else {
                    format!("/runs/{run}/publication/reconcile")
                };
                if !available {
                    card = card.child(hint(if kind == "jira" {
                        "Live Jira is not configured, so this outcome cannot be reconciled here."
                    } else {
                        "PR publication is unavailable for this repository, so this outcome cannot be reconciled here."
                    }));
                }
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-reconcile-{id}")))
                        .primary()
                        .label(if kind == "jira" {
                            "Reconcile Jira"
                        } else {
                            "Reconcile publication"
                        })
                        .disabled(blocked || !available)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({}), false, cx)
                        })),
                );
            }
            "github" => {
                // Retry re-reads the issue first, so an applied update is not repeated.
                let path = format!(
                    "/github/delivery/{}/retry",
                    item["updateId"].as_str().unwrap_or("")
                );
                actions = actions.child(
                    Button::new(SharedString::from(format!("needs-retry-{id}")))
                        .primary()
                        .label("Retry GitHub update")
                        .disabled(blocked || !item["updateId"].is_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({}), false, cx)
                        })),
                );
            }
            _ => {}
        }
        if item["taskId"].is_string() {
            let task_id = item["taskId"].clone();
            // PR comments are listed in the session's Results, so feedback opens there.
            let (tab, label) = match kind.as_str() {
                "ownership" => ("History", "Open session"),
                "feedback" => ("Results", "Open PR comments"),
                _ => ("Conversation", "Open session"),
            };
            actions = actions.child(
                Button::new(SharedString::from(format!("needs-open-{id}")))
                    .ghost()
                    .label(label)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_session(&task_id, tab, cx)),
                    ),
            );
        }
        card.child(actions)
    }

    pub(super) fn needs_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut page = div().flex().flex_col().gap_4().child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(crate::accessibility::text("Needs you")),
                )
                .child(hint(
                    "Every decision the workflow is waiting on, from recorded state. Nothing here proceeds without you.",
                )),
        );
        if !self.workspace_loaded() {
            return page.child(
                section("Waiting for the workflow service").child(named_hint(
                    "needs-waiting",
                    "Tickets, queue and decisions appear once it connects.",
                )),
            );
        }
        let Some(items) = self.needs_you() else {
            return page.child(
                section("Decisions unavailable").child(named_hint(
                    "needs-unavailable",
                    "The workflow service is running without its execution layer, so pending decisions cannot be listed. Each ticket's own screens still show them.",
                )),
            );
        };
        if items.is_empty() {
            return page.child(section("Nothing needs you").child(named_hint(
                "needs-empty",
                "Repair limits, learning proposals, requirement changes, PR feedback and uncertain outcomes appear here.",
            )));
        }
        for item in &items {
            page = page.child(self.needs_item(item, window, cx));
        }
        page
    }
}
