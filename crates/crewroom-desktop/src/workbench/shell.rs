//! Session navigation and the separate-facts header shared by every ticket view.
//! Jira status, workflow state, agent activity and readiness are never merged.
use super::*;
use gpui_kit::Stateful;

pub(super) const ENDED: [&str; 3] = ["cancelled", "closed", "completed"];
const KEYBOARD_ROUTES: &str = if cfg!(target_os = "macos") {
    "Cmd+1 tickets · Cmd+2 session · Cmd+3 Needs you"
} else {
    "Ctrl+1 tickets · Ctrl+2 session · Ctrl+3 Needs you"
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Group {
    Active,
    Queued,
    Other,
}

pub(super) fn chip(id: String, value: impl Into<SharedString>, tone: u32) -> Div {
    div()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if tone == 0x9eacb9 { 0x293642 } else { tone }))
        .text_xs()
        .text_color(rgb(tone))
        .child(crate::accessibility::named(id, value))
}

impl Workbench {
    /// The run a ticket view shows: the operator's selection, else the unfinished run, else the latest.
    pub(super) fn current_run(&self, task_id: &str) -> Option<Value> {
        let runs: Vec<_> = records(&self.workspace, "runs")
            .into_iter()
            .filter(|r| r["taskId"] == task_id)
            .collect();
        self.selected_runs
            .get(task_id)
            .and_then(|id| runs.iter().find(|r| r["id"] == *id))
            .or_else(|| {
                runs.iter()
                    .find(|r| !ENDED.contains(&r["status"].as_str().unwrap_or("")))
            })
            .or_else(|| runs.last())
            .cloned()
    }

    pub(super) fn workflow_label(task: &Value, run: Option<&Value>) -> &'static str {
        run.map(crewroom_core::lifecycle::run_label).unwrap_or(
            if task["needsReacceptance"] == true {
                "Source changed · review brief"
            } else {
                "Refining"
            },
        )
    }

    /// Recorded agent processes for a run, in the shared state vocabulary, with the tone of the
    /// state that most needs the operator. Queued plans have none, and "admitted" is "Not started".
    pub(super) fn activity_summary(&self, run: Option<&Value>) -> (String, String, u32) {
        let rows: Vec<_> = records(&self.workspace, "activity")
            .into_iter()
            .filter(|a| run.is_some_and(|r| a["runId"] == r["id"]))
            .collect();
        let live: Vec<_> = rows.iter().filter(|a| a["state"] != "exited").collect();
        if live.is_empty() {
            return (
                "No agent processes".into(),
                if rows.is_empty() {
                    "Queued plans are not executing agents".into()
                } else {
                    format!("{} stopped", rows.len())
                },
                crate::agent_state::NEUTRAL,
            );
        }
        // Count every state, so one working agent never hides another that needs attention.
        use crate::agent_state::{FAILED, NEUTRAL, WAITING, WARNING, WORKING};
        let labels: Vec<_> = live.iter().map(|a| crate::agent_state::of(a)).collect();
        let mut counts: Vec<(String, usize)> = vec![];
        for label in &labels {
            match counts.iter_mut().find(|(short, _)| *short == label.short) {
                Some((_, n)) => *n += 1,
                None => counts.push((label.short.clone(), 1)),
            }
        }
        let tone = [FAILED, WARNING, WORKING, WAITING]
            .into_iter()
            .find(|tone| labels.iter().any(|l| l.tone == *tone))
            .unwrap_or(NEUTRAL);
        (
            counts
                .iter()
                .map(|(short, n)| format!("{n} {}", short.to_lowercase()))
                .collect::<Vec<_>>()
                .join(" · "),
            live.iter()
                .zip(&labels)
                .map(|(a, label)| format!("{}: {}", string(a, "assignmentName"), label.text))
                .collect::<Vec<_>>()
                .join(" · "),
            tone,
        )
    }

    pub(super) fn group(run: Option<&Value>) -> Group {
        match run.and_then(|r| r["status"].as_str()) {
            Some("awaiting_runner") => Group::Queued,
            Some(status) if !ENDED.contains(&status) => Group::Active,
            _ => Group::Other,
        }
    }

    pub(super) fn ticket_row(
        &self,
        task: &Value,
        run: Option<&Value>,
        project: Option<&Value>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = string(task, "id");
        let select = id.clone();
        let select_project = project
            .filter(|p| p["retiredAt"].is_null())
            .map(|p| string(p, "id"));
        let selected = self.sidebar_current().as_deref() == Some(&format!("ticket-{id}"));
        let workflow = Self::workflow_label(task, run);
        let (activity, _, activity_tone) = self.activity_summary(run);
        let mut key = format!(
            "{} · {}",
            string(task, "key"),
            ticket_source::kind_tag(task)
        );
        if let Some(position) = run
            .filter(|r| r["status"] == "awaiting_runner")
            .and_then(|r| r["position"].as_u64())
        {
            key = format!("{key} · #{} in queue", position + 1);
        }
        div()
            .id(SharedString::from(format!("row-{id}")))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected { 0x3c6273 } else { 0x0e171e }))
            .bg(if selected {
                rgb(0x223b46)
            } else {
                rgb(0x0e171e)
            })
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9eacb9))
                    .child(crate::accessibility::named(format!("row-key-{id}"), key)),
            )
            .child(
                Button::new(SharedString::from(format!("ticket-{id}")))
                    .ghost()
                    .label(string(task, "title"))
                    .selected(selected)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = Some(select.clone());
                        if select_project.is_some() {
                            this.set_current_project(select_project.clone());
                        }
                        this.panel = Panel::Ticket;
                        this.navigation_open = false;
                        // The next Tab continues in the session instead of the remaining rows.
                        window.focus(&this.session_focus, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(chip(
                        format!("row-source-{id}"),
                        // Source status is the provider's fact, labelled with its name.
                        ticket_source::status_label(task),
                        0x9eacb9,
                    ))
                    .child(chip(
                        format!("row-workflow-{id}"),
                        workflow,
                        if workflow.starts_with("Needs attention") {
                            0xffb0a2
                        } else if workflow.starts_with("Paused")
                            || workflow == "Queued"
                            || workflow.starts_with("Source")
                        {
                            0xf1bf73
                        } else {
                            0x9eacb9
                        },
                    ))
                    .child(chip(format!("row-activity-{id}"), activity, activity_tone)),
            )
            .children(self.row_action(task, run, project, cx))
            .child(self.shelf_controls(task, run, cx))
    }

    /// Start for a ticket in an active project with no unfinished run; Choose a project for a
    /// ticket without one, which opens the ticket's project picker.
    fn row_action(
        &self,
        task: &Value,
        run: Option<&Value>,
        project: Option<&Value>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let id = string(task, "id");
        if project.is_none() && task["internal"] != true && Self::group(run) == Group::Other {
            return Some(
                div().flex().child(
                    Button::new(SharedString::from(format!("choose-project-{id}")))
                        .ghost()
                        .label("Choose a project")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(id.clone());
                            this.panel = Panel::Ticket;
                            this.tab = "Brief";
                            this.navigation_open = false;
                            cx.notify();
                        })),
                ),
            );
        }
        if !self.startable(task, run) {
            return None;
        }
        let name = project.map(|p| string(p, "name")).unwrap_or_default();
        Some(
            div().flex().child(
                Button::new(SharedString::from(format!("start-{id}")))
                    .ghost()
                    .label("Start")
                    .accessibility_label(format!("Start {} in {name}", string(task, "key")))
                    .disabled(self.blocked())
                    .on_click(cx.listener(move |this, _, _, cx| this.start_ticket(&id, cx))),
            ),
        )
    }

    /// The one sidebar control that names the current panel. Every control's selected state comes
    /// from here, so a panel never shows two or none. Before a ticket is chosen the ticket panel
    /// shows the Overview home screen, so Overview is current.
    pub(super) fn sidebar_current(&self) -> Option<String> {
        Some(match self.panel {
            Panel::Ticket => match &self.selected {
                Some(id) => format!("ticket-{id}"),
                None => "overview".into(),
            },
            Panel::Overview => "overview".into(),
            Panel::New => "new-ticket".into(),
            Panel::Queue => "queue".into(),
            Panel::NeedsYou => "needs-you".into(),
            Panel::Roles
            | Panel::Models
            | Panel::Runner
            | Panel::Principles
            | Panel::Playbooks
            | Panel::Repositories
            | Panel::Learning
            | Panel::Import => "settings".into(),
        })
    }

    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let current = self.sidebar_current();
        let mut tickets = div()
            .id("ticket-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        let loaded = self.workspace_loaded();
        if !loaded {
            tickets = tickets.child(named_hint("workspace-waiting", WORKSPACE_WAITING));
        } else {
            // One waiting state; nothing is known to be empty before the first snapshot.
            let queued = records(&self.workspace, "runs")
                .iter()
                .filter(|r| r["status"] == "awaiting_runner")
                .count();
            tickets = tickets.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        // Named after the page it opens.
                        Button::new("queue")
                            .ghost()
                            .label("Coding queue")
                            .selected(current.as_deref() == Some("queue"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Queue;
                                this.navigation_open = false;
                                cx.notify();
                            })),
                    )
                    .when(queued > 0, |row| {
                        row.child(named_hint("queue-count", format!("{queued} queued")).text_xs())
                    }),
            );
            if queued == 0 {
                tickets = tickets.child(
                    named_hint(
                        "queue-empty",
                        "No queued plans. Queue order is local, not the ticket source's rank.",
                    )
                    .text_xs(),
                );
            }
            tickets = tickets.child(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(rgb(0x9eacb9))
                    .child(crate::accessibility::named("projects-heading", "PROJECTS")),
            );
            if self.ordered_projects().is_empty() {
                tickets = tickets
                    .child(named_hint("projects-empty", "No projects yet.").text_xs())
                    .child(
                        div().flex().child(
                            Button::new("sidebar-add-project")
                                .label("Add a project")
                                .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
                        ),
                    );
            }
            tickets = tickets.children(self.project_groups(cx));
        }
        div()
            .id("sidebar")
            .w(px(288.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x0e171e))
            .border_r_1()
            .border_color(rgb(0x293642))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child(crate::accessibility::text("Crewroom")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if self.connected {
                                rgb(0x84d7bc)
                            } else {
                                rgb(0xf1bf73)
                            })
                            .child(crate::accessibility::text(if self.connected {
                                "Connected"
                            } else {
                                "Reconnecting"
                            })),
                    ),
            )
            .child(self.overview_entry(cx))
            .child(self.needs_entry(cx))
            // Outside the scrolling list, so Undo stays in view after a shelf action.
            .children(self.undo_bar(cx))
            .child(crate::accessibility::focus_region(
                tickets,
                &self.tickets_focus,
            ))
            .child(named_hint("keyboard-routes", KEYBOARD_ROUTES).text_xs())
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(0x293642))
                    .child(
                        Button::new("new-ticket")
                            .label("New ticket")
                            .selected(current.as_deref() == Some("new-ticket"))
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_project = this.current_project();
                                this.panel = Panel::New;
                                this.navigation_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("settings")
                            .label("⚙ Settings")
                            .selected(current.as_deref() == Some("settings"))
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Roles;
                                this.navigation_open = false;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// Keyboard routes past the ticket list; the narrow layout shows tickets in its overlay.
    pub(super) fn focus_tickets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.viewport_size().width < px(900.) {
            self.navigation_open = true;
        }
        window.focus(&self.tickets_focus, cx);
        cx.notify();
    }
    pub(super) fn focus_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation_open = false;
        window.focus(&self.session_focus, cx);
        cx.notify();
    }
    pub(super) fn show_needs_you(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Panel::NeedsYou;
        self.focus_session(window, cx);
    }

    /// The entry for pending decisions, with the open count from the service projection. It sits
    /// in the sidebar, and in the narrow header so pending decisions never hide behind navigation.
    pub(super) fn needs_entry(&self, cx: &mut Context<Self>) -> Div {
        let count = self.needs_you().map(|items| items.len());
        div().child(
            Button::new("needs-you")
                .ghost()
                .w_full()
                .selected(self.sidebar_current().as_deref() == Some("needs-you"))
                .label(match count {
                    Some(n) => format!("Needs you · {n}"),
                    None => "Needs you · unavailable".into(),
                })
                .when(count.is_some_and(|n| n > 0), |b| b.primary())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.panel = Panel::NeedsYou;
                    this.navigation_open = false;
                    cx.notify();
                })),
        )
    }

    /// Settings sections share one sub-navigation; their element IDs are unchanged.
    pub(super) fn settings_nav(&self, cx: &mut Context<Self>) -> Div {
        let mut nav = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .pb_2()
            .border_b_1()
            .border_color(rgb(0x293642));
        for (id, label, panel) in [
            ("roles", "Agent roles", Panel::Roles),
            ("models", "Models & budget", Panel::Models),
            ("runner", "Runner", Panel::Runner),
            ("principles", "Principles", Panel::Principles),
            ("playbooks", "Playbooks", Panel::Playbooks),
            ("repositories", "Projects", Panel::Repositories),
            ("learning", "Learning", Panel::Learning),
            ("import-jira", "Integrations", Panel::Import),
        ] {
            nav = nav.child(
                Button::new(id)
                    .ghost()
                    .label(label)
                    .selected(self.panel == panel)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.panel = panel;
                        cx.notify();
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::accessibility::text("Settings")),
            )
            .child(nav)
    }

    fn readiness(&self, run: Option<&Value>) -> (String, String, u32) {
        let Some(run) = run else {
            return (
                "No candidate".into(),
                "Nothing to validate yet".into(),
                0xe2e8ed,
            );
        };
        if let Some(error) = self
            .observation
            .errors
            .get("result")
            .filter(|_| self.observation.id.as_deref() == run["id"].as_str())
        {
            return (
                "Evidence not current".into(),
                format!("Evidence unavailable: {error}"),
                0x9eacb9,
            );
        }
        if self.observation.id.as_deref() != run["id"].as_str() || !self.observation.result_fresh {
            return (
                "Evidence not current".into(),
                "Waiting for the evidence observation".into(),
                0x9eacb9,
            );
        }
        let result = &self.observation.result;
        if run["purpose"] == "research" {
            return (
                "Research".into(),
                "Review citations and coverage in Results".into(),
                0xe2e8ed,
            );
        }
        if result["assessment"]["ready"] == true {
            return (
                "Locally validated".into(),
                super::results::assessed_revision(result)
                    .map(|r| format!("Checks and independent review cover {r}"))
                    .unwrap_or_else(|| {
                        "Checks and independent review cover the candidate revision".into()
                    }),
                0x84d7bc,
            );
        }
        let blockers: Vec<_> = result["assessment"]["blockers"]
            .as_array()
            .map(|b| {
                b.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        (
            "Not ready".into(),
            blockers
                .first()
                .cloned()
                .unwrap_or_else(|| "No validated candidate revision".into()),
            0xf1bf73,
        )
    }

    /// Four separate facts. None of them is derived from another.
    pub(super) fn facts_bar(&self, task: &Value, run: Option<&Value>) -> Div {
        let provider = ticket_source::provider(task);
        let (activity, detail, _) = self.activity_summary(run);
        let (ready, why, tone) = self.readiness(run);
        let cell = |id: &str, label: &str, value: String, small: String, tone: u32, first: bool| {
            div()
                .id(SharedString::from(format!("fact-cell-{id}")))
                .flex_1()
                .min_w(px(150.))
                .flex()
                .flex_col()
                .px_3()
                .py_2()
                .when(!first, |d| d.border_l_1().border_color(rgb(0x293642)))
                .child(hint(label.to_uppercase()).text_xs())
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(tone))
                        .child(crate::accessibility::named(format!("fact-{id}"), value)),
                )
                .child(hint(small).text_xs())
        };
        div()
            .flex()
            .flex_wrap()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x293642))
            .bg(rgb(0x17212a))
            .child(cell(
                "source",
                // "Jira status" / "GitHub status"; a local ticket has no provider.
                &format!("{} status", ticket_source::source_name(task)),
                ticket_source::status_value(task),
                match provider {
                    Some(p) => format!("Imported {} field", p.name),
                    None => ticket_source::ticket_label(task),
                },
                0xe2e8ed,
                true,
            ))
            .child(cell(
                "workflow",
                "Workflow",
                Self::workflow_label(task, run).into(),
                run.map(|r| format!("{} run", r["purpose"].as_str().unwrap_or("coding")))
                    .unwrap_or_else(|| "No run queued".into()),
                0xe2e8ed,
                false,
            ))
            .child(cell(
                "activity",
                "Agent activity",
                activity,
                detail,
                0xe2e8ed,
                false,
            ))
            .child(cell("readiness", "Readiness", ready, why, tone, false))
    }
}
