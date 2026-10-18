//! Session navigation and the separate-facts header shared by every ticket view.
//! Jira status, workflow state, agent activity and readiness are never merged.
use super::*;
use gpui_kit::Stateful;

const ENDED: [&str; 3] = ["cancelled", "closed", "completed"];
const KEYBOARD_ROUTES: &str = if cfg!(target_os = "macos") {
    "Cmd+1 tickets · Cmd+2 session · Cmd+3 Needs you"
} else {
    "Ctrl+1 tickets · Ctrl+2 session · Ctrl+3 Needs you"
};
type Rows = Vec<(Value, Option<Value>)>;

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Active,
    Queued,
    Other,
}

fn chip(id: String, value: impl Into<SharedString>, tone: u32) -> Div {
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

    fn group(run: Option<&Value>) -> Group {
        match run.and_then(|r| r["status"].as_str()) {
            Some("awaiting_runner") => Group::Queued,
            Some(status) if !ENDED.contains(&status) => Group::Active,
            _ => Group::Other,
        }
    }

    fn ticket_row(
        &self,
        task: &Value,
        run: Option<&Value>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = string(task, "id");
        let select = id.clone();
        let selected = self.sidebar_current().as_deref() == Some(&format!("ticket-{id}"));
        let local = task["source"] != "jira";
        let workflow = Self::workflow_label(task, run);
        let (activity, _, activity_tone) = self.activity_summary(run);
        let mut key = format!(
            "{} · {}",
            string(task, "key"),
            if local { "local" } else { "jira" }
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
                        format!("row-jira-{id}"),
                        if local {
                            "Not linked".to_owned()
                        } else {
                            format!("Jira {}", string(task, "jiraStatus"))
                        },
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
    }

    /// The one sidebar control that names the current panel. Every control's selected state comes
    /// from here, so a panel never shows two or none; it is `None` only before a ticket is chosen.
    pub(super) fn sidebar_current(&self) -> Option<String> {
        Some(match self.panel {
            Panel::Ticket => format!("ticket-{}", self.selected.as_ref()?),
            Panel::New => "new-ticket".into(),
            Panel::Queue => "queue".into(),
            Panel::NeedsYou => "needs-you".into(),
            Panel::Roles
            | Panel::Models
            | Panel::Principles
            | Panel::Playbooks
            | Panel::Repositories
            | Panel::Learning
            | Panel::Import => "settings".into(),
        })
    }

    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let current = self.sidebar_current();
        let mut groups: [(Group, &str, Rows); 3] = [
            (Group::Active, "ACTIVE RUNS", vec![]),
            (Group::Queued, "QUEUE", vec![]),
            (Group::Other, "OTHER TICKETS", vec![]),
        ];
        for task in records(&self.workspace, "tasks") {
            let run = self.current_run(&string(&task, "id"));
            let group = Self::group(run.as_ref());
            groups
                .iter_mut()
                .find(|g| g.0 == group)
                .unwrap()
                .2
                .push((task, run));
        }
        groups[1].2.sort_by_key(|(_, r)| {
            r.as_ref()
                .and_then(|r| r["position"].as_u64())
                .unwrap_or(u64::MAX)
        });
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
        } else if records(&self.workspace, "tasks").is_empty() {
            tickets = tickets.child(hint("Create a ticket or import a Jira snapshot to begin."));
        }
        // One waiting state; no group is known to be empty before the first snapshot.
        for (group, title, rows) in groups.iter().filter(|_| loaded) {
            if rows.is_empty() && *group != Group::Queued {
                continue;
            }
            let mut heading = div().flex().items_center().justify_between().pt_2().child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9eacb9))
                    .child(crate::accessibility::named(
                        format!("group-{}", title.to_lowercase().replace(' ', "-")),
                        *title,
                    )),
            );
            if *group == Group::Queued {
                // Named after the page it opens.
                heading = heading.child(
                    Button::new("queue")
                        .ghost()
                        .label("Coding queue")
                        .selected(current.as_deref() == Some("queue"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel = Panel::Queue;
                            this.navigation_open = false;
                            cx.notify();
                        })),
                );
            }
            tickets = tickets.child(heading);
            if rows.is_empty() {
                tickets = tickets.child(
                    named_hint(
                        "queue-empty",
                        "No queued plans. Queue order is local, not Jira rank.",
                    )
                    .text_xs(),
                );
            }
            for (task, run) in rows {
                tickets = tickets.child(self.ticket_row(task, run.as_ref(), cx));
            }
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
            .child(self.needs_entry(cx))
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
            ("principles", "Principles", Panel::Principles),
            ("playbooks", "Playbooks", Panel::Playbooks),
            ("repositories", "Repositories", Panel::Repositories),
            ("learning", "Learning", Panel::Learning),
            ("import-jira", "Jira", Panel::Import),
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
        let local = task["source"] != "jira";
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
                "jira",
                "Jira status",
                if local {
                    "Not linked".into()
                } else {
                    string(task, "jiraStatus")
                },
                if local {
                    "Local ticket".into()
                } else {
                    "Imported Jira field".into()
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
