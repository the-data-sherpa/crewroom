//! The Overview home screen (FOR-77): Needs you, Running, Landed and Next across every project,
//! each line labelled with its project. Every section always renders, with a sentence when it is
//! empty, so a quiet screen is never read as "all good".
//!
//! The no-progress flag is computed from observed events only (agent output, peer messages,
//! repository checkpoints and the run's start, as the service records them). It is separate from
//! source status, workflow state and agent activity, calls no model, and never says a run failed.
use super::navigation::OverviewPrefs;
use super::*;
use gpui_kit::Stateful;

/// Where an Overview action leads when it is not a direct command.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Target {
    Ticket(String, &'static str),
    NeedsYou,
    Integrations,
    Project(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Action {
    /// Posts to the endpoint the owning screen uses, so the service applies the same checks.
    Post {
        label: &'static str,
        path: String,
        unavailable: Option<&'static str>,
    },
    Open {
        label: &'static str,
        target: Target,
    },
}

/// One operator-actionable item. Lower impact sorts first.
#[derive(Clone, Debug)]
pub(super) struct NeedsItem {
    pub id: String,
    pub impact: u8,
    pub project: String,
    pub ticket: String,
    pub title: String,
    pub detail: String,
    pub action: Action,
}

/// Impact, highest first: unsafe or blocked execution, then unconfirmed delivery and broken
/// connections, then decisions that hold work, then feedback to resume, then proposals, and last
/// a run that has shown no progress for a long time.
fn impact(kind: &str) -> u8 {
    match kind {
        "ownership" | "attention" => 0,
        "publication" | "jira" | "github" | "credentials" => 1,
        "repair" | "requirements" | "conflict" | "questions" => 2,
        "plan" | "revalidation" => 3,
        "feedback" => 4,
        "learning" => 5,
        "no-progress" => 6,
        _ => 3,
    }
}

/// Whole minutes from an RFC 3339 time to `now_ms`, or `None` when the time is unreadable.
pub(super) fn minutes_since(at: &str, now_ms: i64) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(((now_ms - at.timestamp_millis()) / 60_000).max(0))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// "No progress for N min": the flag's wording, which never claims the run failed.
pub(super) fn no_progress(minutes: i64) -> String {
    format!("No progress for {minutes} min")
}

impl Workbench {
    pub(super) fn overview_prefs(&self) -> OverviewPrefs {
        self.navigation.saved().overview.bounded()
    }

    fn update_overview(&mut self, change: impl FnOnce(&mut OverviewPrefs)) {
        if let Err(error) = self.navigation.update_overview(change) {
            self.notice = format!(
                "Overview thresholds could not be saved for reopening: {error}. Workflow data is unaffected."
            );
            self.feedback_origin = Some(self.origin_at(feedback::Place::Top));
        }
    }

    /// The project a ticket targets, by name; a retired project is named as retired.
    fn project_label(&self, task_id: &Value) -> String {
        let task = records(&self.workspace, "tasks")
            .into_iter()
            .find(|t| t["id"] == *task_id);
        let Some(repository) = task.as_ref().and_then(|t| {
            records(&self.workspace, "repositories")
                .into_iter()
                .find(|r| r["id"] == t["repositoryId"])
        }) else {
            return "No project".into();
        };
        let name = string(&repository, "name");
        if repository["retiredAt"].is_null() {
            name
        } else {
            format!("{name} · Retired")
        }
    }

    fn ticket_key(&self, task_id: &Value) -> String {
        records(&self.workspace, "tasks")
            .into_iter()
            .find(|t| t["id"] == *task_id)
            .map(|t| string(&t, "key"))
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "No ticket".into())
    }

    /// Observed progress for a run: minutes since the latest event and what it was. `None` when
    /// the service reports no progress for the run (no execution layer, or a finished run).
    pub(super) fn run_progress(&self, run: &Value) -> Option<(Option<i64>, String)> {
        let progress = self.workspace["runProgress"].get(run["id"].as_str()?)?;
        let minutes = progress["lastAt"]
            .as_str()
            .and_then(|at| minutes_since(at, now_ms()));
        Some((minutes, string(progress, "source")))
    }

    /// Minutes without progress for a running run once it reaches `threshold`.
    fn quiet_for(&self, run: &Value, threshold: u32) -> Option<i64> {
        if run["status"] != "running" {
            return None;
        }
        let (minutes, _) = self.run_progress(run)?;
        minutes.filter(|m| *m >= i64::from(threshold))
    }

    /// Operator-actionable items across projects, highest impact first. Every item comes from
    /// recorded state; nothing here is a guess about what an agent is doing.
    pub(super) fn overview_needs(&self) -> Vec<NeedsItem> {
        let mut items = Vec::new();
        let decisions = self.needs_you().unwrap_or_default();
        for item in &decisions {
            let kind = string(item, "kind");
            let run = string(item, "runId");
            let task = item["taskId"].as_str().map(str::to_owned);
            let open = |label, tab| Action::Open {
                label,
                target: task
                    .clone()
                    .map(|t| Target::Ticket(t, tab))
                    .unwrap_or(Target::NeedsYou),
            };
            let action = match kind.as_str() {
                "feedback" => Action::Post {
                    label: "Resume run",
                    path: format!("/runs/{run}/resume"),
                    unavailable: None,
                },
                "publication" => Action::Post {
                    label: "Reconcile publication",
                    path: format!("/runs/{run}/publication/reconcile"),
                    unavailable: (item["available"] != true).then_some(
                        "PR publication is unavailable for this project, so this outcome cannot be reconciled here.",
                    ),
                },
                "jira" => Action::Post {
                    label: "Reconcile Jira",
                    path: "/jira/reconcile".into(),
                    unavailable: (item["available"] != true).then_some(
                        "Live Jira is not configured, so this outcome cannot be reconciled here.",
                    ),
                },
                "github" => Action::Post {
                    label: "Retry GitHub update",
                    path: format!(
                        "/github/delivery/{}/retry",
                        item["updateId"].as_str().unwrap_or("")
                    ),
                    unavailable: (!item["updateId"].is_string())
                        .then_some("This update has no retry receipt."),
                },
                "requirements" if item["stage"] == "brief" => open("Review brief", "Brief"),
                "ownership" => open("Inspect processes", "History"),
                // These need a recorded reason, which the Needs you screen collects.
                "repair" => Action::Open {
                    label: "Review repair limit",
                    target: Target::NeedsYou,
                },
                "requirements" => Action::Open {
                    label: "Review adoption",
                    target: Target::NeedsYou,
                },
                "learning" => Action::Open {
                    label: "Review proposal",
                    target: Target::NeedsYou,
                },
                _ => Action::Open {
                    label: "Decide",
                    target: Target::NeedsYou,
                },
            };
            items.push(NeedsItem {
                id: string(item, "id"),
                impact: impact(&kind),
                project: self.project_label(&item["taskId"]),
                ticket: self.ticket_key(&item["taskId"]),
                title: string(item, "title"),
                detail: string(item, "detail"),
                action,
            });
        }
        let prefs = self.overview_prefs();
        for run in records(&self.workspace, "runs") {
            let id = string(&run, "id");
            let task = string(&run, "taskId");
            // A run waiting on the operator that no decision above already names.
            if run["status"] == "attention_required"
                && !decisions
                    .iter()
                    .any(|d| d["runId"] == run["id"] && d["kind"] == "ownership")
            {
                let label = Self::workflow_label(&Value::Null, Some(&run));
                items.push(NeedsItem {
                    id: format!("attention-{id}"),
                    impact: impact("attention"),
                    project: self.project_label(&run["taskId"]),
                    ticket: self.ticket_key(&run["taskId"]),
                    title: label.into(),
                    detail: run["attentionReason"]
                        .as_str()
                        .unwrap_or("The run is waiting for you before it can continue.")
                        .into(),
                    action: Action::Open {
                        label: "Open session",
                        target: Target::Ticket(task.clone(), "Conversation"),
                    },
                });
            }
            if let Some(minutes) = self.quiet_for(&run, prefs.promote_minutes) {
                let (_, source) = self.run_progress(&run).unwrap_or_default();
                items.push(NeedsItem {
                    id: format!("no-progress-{id}"),
                    impact: impact("no-progress"),
                    project: self.project_label(&run["taskId"]),
                    ticket: self.ticket_key(&run["taskId"]),
                    title: no_progress(minutes),
                    detail: format!(
                        "No agent output, commits or messages observed for {minutes} min (last: {source}). This does not mean the run failed: open the session to look, steer or pause it."
                    ),
                    action: Action::Open {
                        label: "Open session",
                        target: Target::Ticket(task, "Conversation"),
                    },
                });
            }
        }
        if let Some(intakes) = self.workspace["intake"].as_object() {
            for (task_id, view) in intakes {
                let task = json!(task_id);
                let reaccept = records(&self.workspace, "tasks")
                    .iter()
                    .any(|t| t["id"] == task && t["needsReacceptance"] == true);
                let (kind, title, label) = match view["state"].as_str() {
                    Some("Plan review") => (
                        "plan",
                        "Implementation plan awaits your decision",
                        "Review plan",
                    ),
                    Some("Waiting for you") => (
                        "questions",
                        "The coordinator asked questions",
                        "Answer questions",
                    ),
                    // Re-acceptance of the brief comes first and has its own item.
                    Some("Needs revalidation") if !reaccept => (
                        "revalidation",
                        "Plan needs revalidation against the current source",
                        "Revalidate plan",
                    ),
                    _ => continue,
                };
                items.push(NeedsItem {
                    id: format!("{kind}-{task_id}"),
                    impact: impact(kind),
                    project: self.project_label(&task),
                    ticket: self.ticket_key(&task),
                    title: title.into(),
                    detail: String::new(),
                    action: Action::Open {
                        label,
                        target: Target::Ticket(task_id.clone(), "Brief"),
                    },
                });
            }
        }
        if self.workspace["github"]["state"] == "rejected" {
            items.push(NeedsItem {
                id: "credentials-github".into(),
                impact: impact("credentials"),
                project: "All projects".into(),
                ticket: "GitHub".into(),
                title: "GitHub rejected the saved token".into(),
                detail: "GitHub Issues, PR publication and issue updates wait until a working token is saved.".into(),
                action: Action::Open {
                    label: "Reconnect GitHub",
                    target: Target::Integrations,
                },
            });
        }
        for project in self.ordered_projects() {
            if project["projectSource"]["state"] == "mismatch" {
                let id = string(&project, "id");
                items.push(NeedsItem {
                    id: format!("conflict-{id}"),
                    impact: impact("conflict"),
                    project: string(&project, "name"),
                    ticket: "Ticket source".into(),
                    title: "Ticket source no longer matches the project's remote".into(),
                    detail: "Confirm or change the ticket source before importing.".into(),
                    action: Action::Open {
                        label: "Open project settings",
                        target: Target::Project(id),
                    },
                });
            }
        }
        // A stable sort keeps the service's order within one impact.
        items.sort_by_key(|i| i.impact);
        items
    }

    fn open_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        match target {
            Target::Ticket(id, tab) => {
                self.selected = Some(id.clone());
                self.panel = Panel::Ticket;
                self.tab = tab;
            }
            Target::NeedsYou => self.panel = Panel::NeedsYou,
            Target::Integrations => self.panel = Panel::Import,
            Target::Project(id) => {
                self.settings.repository = id.clone();
                self.panel = Panel::Repositories;
            }
        }
        self.navigation_open = false;
        cx.notify();
    }

    /// The sidebar entry for the Overview.
    pub(super) fn overview_entry(&self, cx: &mut Context<Self>) -> Div {
        div().child(
            Button::new("overview")
                .ghost()
                .w_full()
                .selected(self.sidebar_current().as_deref() == Some("overview"))
                .label("Overview")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.panel = Panel::Overview;
                    this.navigation_open = false;
                    cx.notify();
                })),
        )
    }

    /// Re-renders every 30 seconds so elapsed times and the no-progress flag stay current
    /// between service updates. Only with a live service connection.
    pub(super) fn overview_ticker(cx: &mut Context<Self>) -> gpui_kit::Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
    }

    fn overview_section(slug: &str, title: &str, count: Option<usize>) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("overview-{slug}")))
            .flex()
            .flex_col()
            .gap_2()
            .p_5()
            .rounded_lg()
            .bg(rgb(0x17212a))
            .border_1()
            .border_color(rgb(0x293642))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                crate::accessibility::named(
                    format!("overview-{slug}-heading"),
                    match count {
                        Some(n) => format!("{title} · {n}"),
                        None => title.to_owned(),
                    },
                ),
            ))
    }

    fn overview_line(id: String) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgb(0x0e171e))
    }

    /// "Project · KEY", the label every Overview line starts with.
    fn place(id: String, project: &str, ticket: &str) -> Div {
        div()
            .text_xs()
            .text_color(rgb(0x9eacb9))
            .child(crate::accessibility::named(
                id,
                format!("{project} · {ticket}"),
            ))
    }

    fn needs_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let items = self.overview_needs();
        let available = self.needs_you().is_some();
        let mut block = Self::overview_section("needs", "Needs you", Some(items.len()));
        if !available {
            block = block.child(named_hint(
                "overview-needs-unavailable",
                "Workflow decisions cannot be listed: the service is running without its execution layer. Items below come from tickets, projects and connections only.",
            ));
        }
        if items.is_empty() {
            return block.child(named_hint(
                "overview-needs-empty",
                "Nothing needs you right now. Plan decisions, questions, re-acceptance, PR feedback, credential problems and conflicts appear here.",
            ));
        }
        let blocked = self.blocked();
        for item in items {
            let id = item.id.clone();
            let mut line =
                Self::overview_line(format!("overview-needs-{id}"))
                    .border_l_4()
                    .border_color(rgb(if item.impact <= 1 { 0xf1bf73 } else { 0x84d7bc }))
                    .child(Self::place(
                        format!("overview-needs-place-{id}"),
                        &item.project,
                        &item.ticket,
                    ))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(
                        crate::accessibility::named(
                            format!("overview-needs-title-{id}"),
                            item.title.clone(),
                        ),
                    ));
            if !item.detail.is_empty() {
                line = line.child(hint(item.detail.clone()));
            }
            let button = Button::new(SharedString::from(format!("overview-needs-action-{id}")));
            let button = match item.action {
                Action::Post {
                    label,
                    path,
                    unavailable,
                } => {
                    if let Some(reason) = unavailable {
                        line = line.child(hint(reason));
                    }
                    button
                        .primary()
                        .label(label)
                        .disabled(blocked || unavailable.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({}), false, cx)
                        }))
                }
                Action::Open { label, target } => button
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_target(&target, cx))),
            };
            line = line.child(div().flex().child(button));
            block = block.child(line);
        }
        block
    }

    /// The coordinator and specialists of a run, from its saved launch, else from its processes.
    fn team_text(&self, run: &Value) -> String {
        let team = records(&run["launch"], "team");
        let (coordinator, specialists): (Vec<_>, Vec<_>) = if team.is_empty() {
            let names: Vec<_> = records(&self.workspace, "activity")
                .into_iter()
                .filter(|a| a["runId"] == run["id"])
                .map(|a| string(&a, "assignmentName"))
                .collect();
            (vec![], names)
        } else {
            let (c, s): (Vec<_>, Vec<_>) = team.iter().partition(|m| m["coordinator"] == true);
            (
                c.iter().map(|m| string(m, "name")).collect(),
                s.iter().map(|m| string(m, "name")).collect(),
            )
        };
        let mut parts = vec![];
        if let Some(c) = coordinator.first() {
            parts.push(format!("Coordinator {c}"));
        }
        parts.push(if specialists.is_empty() {
            "no specialists".into()
        } else {
            format!("specialists {}", specialists.join(", "))
        });
        parts.join(" · ")
    }

    fn running_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let prefs = self.overview_prefs();
        let runs: Vec<_> = records(&self.workspace, "runs")
            .into_iter()
            .filter(|r| matches!(r["status"].as_str(), Some("running" | "pausing")))
            .collect();
        let mut block = Self::overview_section("running", "Running", Some(runs.len())).child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(
                    named_hint(
                        "overview-thresholds",
                        format!(
                            "Flag no progress after {} min · list under Needs you after {} min",
                            prefs.stale_minutes, prefs.promote_minutes
                        ),
                    )
                    .text_xs(),
                )
                .children(
                    [
                        ("overview-stale-less", "Flag sooner", -5i64, 0i64),
                        ("overview-stale-more", "Flag later", 5, 0),
                        ("overview-promote-less", "List sooner", 0, -15),
                        ("overview-promote-more", "List later", 0, 15),
                    ]
                    .into_iter()
                    .map(|(id, label, stale, promote)| {
                        Button::new(id).ghost().label(label).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.update_overview(|o| {
                                    o.stale_minutes =
                                        (i64::from(o.stale_minutes) + stale).max(1) as u32;
                                    o.promote_minutes =
                                        (i64::from(o.promote_minutes) + promote).max(1) as u32;
                                });
                                cx.notify();
                            },
                        ))
                    }),
                ),
        );
        if runs.is_empty() {
            return block.child(named_hint(
                "overview-running-empty",
                "No run is executing. Queued plans are not executing agents; they are listed under Next.",
            ));
        }
        for run in runs {
            let id = string(&run, "id");
            let task = records(&self.workspace, "tasks")
                .into_iter()
                .find(|t| t["id"] == run["taskId"])
                .unwrap_or(Value::Null);
            let (activity, _, activity_tone) = self.activity_summary(Some(&run));
            let last = match self.run_progress(&run) {
                Some((Some(minutes), source)) => {
                    format!("Last progress {minutes} min ago · {source}")
                }
                Some((None, _)) => "No progress observed yet".into(),
                None => "Progress unavailable from the service".into(),
            };
            let mut facts = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .text_xs()
                .child(crate::accessibility::named(
                    format!("overview-running-stage-{id}"),
                    Self::workflow_label(&task, Some(&run)),
                ))
                .child(
                    div()
                        .text_color(rgb(activity_tone))
                        .child(crate::accessibility::named(
                            format!("overview-running-activity-{id}"),
                            activity,
                        )),
                )
                .child(crate::accessibility::named(
                    format!("overview-running-last-{id}"),
                    last,
                ));
            // Separate from workflow state and agent activity; it never claims failure.
            if let Some(minutes) = self.quiet_for(&run, prefs.stale_minutes) {
                facts = facts.child(
                    div()
                        .px_1p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(0xf1bf73))
                        .text_color(rgb(0xf1bf73))
                        .child(crate::accessibility::named(
                            format!("overview-running-stale-{id}"),
                            no_progress(minutes),
                        )),
                );
            }
            let open = string(&run, "taskId");
            block = block.child(
                Self::overview_line(format!("overview-running-{id}"))
                    .child(Self::place(
                        format!("overview-running-place-{id}"),
                        &self.project_label(&run["taskId"]),
                        &self.ticket_key(&run["taskId"]),
                    ))
                    .child(crate::accessibility::named(
                        format!("overview-running-team-{id}"),
                        self.team_text(&run),
                    ))
                    .child(facts)
                    .child(
                        div().flex().child(
                            Button::new(SharedString::from(format!("overview-running-open-{id}")))
                                .ghost()
                                .label("Open session")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_target(
                                        &Target::Ticket(open.clone(), "Conversation"),
                                        cx,
                                    )
                                })),
                        ),
                    ),
            );
        }
        block
    }

    fn landed_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(entries) = self.workspace["landed"].as_array() else {
            return Self::overview_section("landed", "Landed", None).child(named_hint(
                "overview-landed-unavailable",
                "Landed work cannot be listed: the service is running without its execution layer.",
            ));
        };
        let entries: Vec<_> = entries.iter().take(8).cloned().collect();
        let mut block = Self::overview_section("landed", "Landed", Some(entries.len()));
        if entries.is_empty() {
            return block.child(named_hint(
                "overview-landed-empty",
                "Nothing has landed yet. Published PRs and completed research appear here with the revision their evidence covers.",
            ));
        }
        for entry in entries {
            let id = string(&entry, "runId");
            let (what, covers) = if entry["kind"] == "pr" {
                (
                    format!(
                        "PR #{} · {}",
                        entry["pr"],
                        if entry["merged"] == true {
                            "merged".to_owned()
                        } else {
                            entry["providerState"]
                                .as_str()
                                .map(|s| format!("published · {}", s.to_lowercase()))
                                .unwrap_or_else(|| "published".into())
                        }
                    ),
                    match entry["revision"].as_str() {
                        Some(head) => format!(
                            "Evidence covers revision {}",
                            head.chars().take(12).collect::<String>()
                        ),
                        None => "Evidence revision not recorded".into(),
                    },
                )
            } else {
                (
                    "Research completed".into(),
                    match entry["researchRevision"].as_u64() {
                        Some(n) => format!("Evidence covers research revision {n}"),
                        None => "Evidence revision not recorded".into(),
                    },
                )
            };
            let open = string(&entry, "taskId");
            block = block.child(
                Self::overview_line(format!("overview-landed-{id}"))
                    .child(Self::place(
                        format!("overview-landed-place-{id}"),
                        &self.project_label(&entry["taskId"]),
                        &self.ticket_key(&entry["taskId"]),
                    ))
                    .child(crate::accessibility::named(
                        format!("overview-landed-what-{id}"),
                        what,
                    ))
                    .child(named_hint(format!("overview-landed-revision-{id}"), covers).text_xs())
                    .child(
                        div().flex().child(
                            Button::new(SharedString::from(format!("overview-landed-open-{id}")))
                                .ghost()
                                .label("Open results")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_target(&Target::Ticket(open.clone(), "Results"), cx)
                                })),
                        ),
                    ),
            );
        }
        block
    }

    fn next_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let plans = records(&self.workspace["queue"], "plans");
        let mut runs: Vec<_> = records(&self.workspace, "runs")
            .into_iter()
            .filter(|r| r["status"] == "awaiting_runner")
            .collect();
        runs.sort_by_key(|r| r["position"].as_u64().unwrap_or(u64::MAX));
        let mut block = Self::overview_section("next", "Next", Some(runs.len()));
        if runs.is_empty() {
            return block.child(named_hint(
                "overview-next-empty",
                "Nothing is queued. Start a ticket from its project to queue it.",
            ));
        }
        for run in runs {
            let id = string(&run, "id");
            let reason = match plans.iter().find(|p| p["id"] == run["id"]) {
                Some(plan) => super::planning::hold_reason(plan, &self.workspace),
                None if self.workspace["capabilities"]["execution"] == true => {
                    "Waiting for the runner".into()
                }
                None => "Runner not configured".into(),
            };
            let position = run["position"]
                .as_u64()
                .map(|p| format!("#{} in queue · ", p + 1))
                .unwrap_or_default();
            let open = string(&run, "taskId");
            block = block.child(
                Self::overview_line(format!("overview-next-{id}"))
                    .child(Self::place(
                        format!("overview-next-place-{id}"),
                        &self.project_label(&run["taskId"]),
                        &self.ticket_key(&run["taskId"]),
                    ))
                    .child(crate::accessibility::named(
                        format!("overview-next-reason-{id}"),
                        format!("{position}{reason}"),
                    ))
                    .child(
                        div().flex().child(
                            Button::new(SharedString::from(format!("overview-next-open-{id}")))
                                .ghost()
                                .label("Open ticket")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_target(&Target::Ticket(open.clone(), "Brief"), cx)
                                })),
                        ),
                    ),
            );
        }
        block
    }

    pub(super) fn overview_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let page = div().flex().flex_col().gap_4().child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(crate::accessibility::named("overview-title", "Overview")),
                )
                .child(hint(
                    "Every project at once, from recorded state. Source status, workflow state and agent activity stay separate.",
                )),
        );
        if !self.workspace_loaded() {
            return page.child(named_hint("overview-waiting", WORKSPACE_WAITING));
        }
        page.child(self.needs_section(cx))
            .child(self.running_section(cx))
            .child(self.landed_section(cx))
            .child(self.next_section(cx))
    }
}
