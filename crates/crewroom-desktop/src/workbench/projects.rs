//! The project sidebar (FOR-75). A project is one configured checkout with its ticket source,
//! checks, instructions and branch prefix; the service and the data keep calling it a
//! repository. Tickets are grouped by the project they target, in four sections, with tickets
//! that have none under "Unassigned". The current project, the order and the collapsed groups
//! are this machine's navigation only and never change workflow data.
//!
//! Each row keeps source status, workflow state and agent activity as separate facts; the
//! section a ticket sits in is derived from workflow state and pending decisions only.
use super::navigation::{ProjectPrefs, UNASSIGNED};
use super::shelves::Shelf;
use super::*;
use gpui_kit::Stateful;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Section {
    NeedsYou,
    Running,
    Queued,
    Other,
}
const SECTIONS: [(Section, &str, &str); 4] = [
    (Section::NeedsYou, "needs-you", "Needs you"),
    (Section::Running, "running", "Running"),
    (Section::Queued, "queued", "Queued"),
    (Section::Other, "other", "Other"),
];

/// Up to two initials of a project name, for the header monogram.
pub(super) fn monogram(name: &str) -> String {
    let words: Vec<_> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    letters.to_uppercase()
}

/// The project's ticket-source state in a few words, from the service's projection. The
/// service's own sentence stays in Settings → Projects.
pub(super) fn source_status(repository: &Value) -> (String, u32) {
    let view = &repository["projectSource"];
    if view.is_null() {
        return ("Ticket source unavailable".into(), 0x9eacb9);
    }
    match view["state"].as_str() {
        Some("confirmed") => (
            match view["saved"]["tickets"].as_str() {
                Some("github") => "GitHub Issues".into(),
                Some("jira") => "Jira".into(),
                _ => "Ticket source confirmed".into(),
            },
            0x9eacb9,
        ),
        Some("mismatch") => ("Source no longer matches remote".into(), 0xf1bf73),
        _ => ("No ticket source · local tickets".into(), 0x9eacb9),
    }
}

/// A group of tickets in the sidebar: one project, or the tickets without one.
struct Group {
    key: String,
    project: Option<Value>,
    rows: Vec<(Value, Option<Value>, Section)>,
    /// Snoozed and settled tickets (FOR-78), kept out of the four sections.
    shelved: Vec<(Value, Option<Value>, Shelf)>,
}

impl Workbench {
    pub(super) fn project_prefs(&self) -> &ProjectPrefs {
        &self.navigation.saved().projects
    }

    /// Active projects in the operator's order; projects added since come last, in service order.
    pub(super) fn ordered_projects(&self) -> Vec<Value> {
        let mut projects: Vec<_> = records(&self.workspace, "repositories")
            .into_iter()
            .filter(|r| r["retiredAt"].is_null())
            .collect();
        let order = &self.project_prefs().order;
        projects.sort_by_key(|p| {
            order
                .iter()
                .position(|id| p["id"] == *id)
                .unwrap_or(usize::MAX)
        });
        projects
    }

    /// The remembered project while it is still active.
    pub(super) fn current_project(&self) -> Option<String> {
        let id = self.project_prefs().current.as_ref()?;
        records(&self.workspace, "repositories")
            .iter()
            .any(|r| r["id"] == *id && r["retiredAt"].is_null())
            .then(|| id.clone())
    }

    pub(super) fn save_projects(&mut self, change: impl FnOnce(&mut ProjectPrefs)) {
        if let Err(error) = self.navigation.update_projects(change) {
            self.notice = format!(
                "Project layout could not be saved for reopening: {error}. Workflow data is unaffected."
            );
            self.feedback_origin = Some(self.origin_at(feedback::Place::Top));
        }
    }

    pub(super) fn set_current_project(&mut self, id: Option<String>) {
        self.save_projects(|p| p.current = id);
    }

    /// The active project a ticket targets, if any.
    pub(super) fn ticket_project(&self, task: &Value) -> Option<Value> {
        records(&self.workspace, "repositories")
            .into_iter()
            .find(|r| r["id"] == task["repositoryId"] && r["retiredAt"].is_null())
    }

    /// Pending decisions come first, then running work, then the local queue.
    pub(super) fn section(&self, task: &Value, run: Option<&Value>) -> Section {
        let pending = self
            .needs_you()
            .is_some_and(|items| items.iter().any(|i| i["taskId"] == task["id"]));
        if pending || Self::workflow_label(task, run).starts_with("Needs attention") {
            return Section::NeedsYou;
        }
        match Self::group(run) {
            shell::Group::Active => Section::Running,
            shell::Group::Queued => Section::Queued,
            shell::Group::Other => Section::Other,
        }
    }

    fn groups(&self) -> Vec<Group> {
        let repositories = records(&self.workspace, "repositories");
        let mut groups: Vec<Group> = self
            .ordered_projects()
            .into_iter()
            .map(|p| Group {
                key: string(&p, "id"),
                project: Some(p),
                rows: vec![],
                shelved: vec![],
            })
            .collect();
        let mut unassigned = Group {
            key: UNASSIGNED.into(),
            project: None,
            rows: vec![],
            shelved: vec![],
        };
        for task in records(&self.workspace, "tasks") {
            let id = string(&task, "id");
            let run = self.current_run(&id);
            let section = self.section(&task, run.as_ref());
            let shelf = self.shelf_of(&id, section);
            let target = task["repositoryId"].as_str().filter(|id| !id.is_empty());
            let group = if let Some(i) = groups.iter().position(|g| Some(g.key.as_str()) == target)
            {
                &mut groups[i]
            } else if let Some(retired) = target
                .and_then(|id| repositories.iter().find(|r| r["id"] == id))
                .cloned()
            {
                // A retired project keeps showing its saved tickets.
                groups.push(Group {
                    key: string(&retired, "id"),
                    project: Some(retired),
                    rows: vec![],
                    shelved: vec![],
                });
                groups.last_mut().unwrap()
            } else {
                &mut unassigned
            };
            match shelf {
                Some(shelf) => group.shelved.push((task, run, shelf)),
                None => group.rows.push((task, run, section)),
            }
        }
        if !unassigned.rows.is_empty() || !unassigned.shelved.is_empty() {
            groups.push(unassigned);
        }
        for group in &mut groups {
            group.rows.sort_by_key(|(_, run, section)| {
                (
                    SECTIONS.iter().position(|(s, ..)| s == section),
                    (*section == Section::Queued)
                        .then(|| run.as_ref().and_then(|r| r["position"].as_u64()))
                        .flatten()
                        .unwrap_or(u64::MAX),
                )
            });
        }
        groups
    }

    /// Every project group, then "Unassigned", for the ticket list.
    pub(super) fn project_groups(&self, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let current = self.current_project();
        let active: Vec<String> = self
            .ordered_projects()
            .iter()
            .map(|p| string(p, "id"))
            .collect();
        self.groups()
            .into_iter()
            .map(|group| {
                let collapsed = self.project_prefs().collapsed.contains(&group.key);
                let key = group.key.clone();
                let mut column = div()
                    .id(SharedString::from(format!("project-{key}")))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pt_2()
                    .pl_2()
                    .border_l_2()
                    .border_color(rgb(if current.as_deref() == Some(key.as_str()) {
                        0x5fa8c4
                    } else {
                        0x0e171e
                    }))
                    .child(self.project_header(&group, collapsed, &active, cx));
                if collapsed {
                    return column;
                }
                if group.rows.is_empty() && group.shelved.is_empty() {
                    column = column.child(
                        named_hint(
                            format!("project-{key}-empty"),
                            "No tickets in this project yet.",
                        )
                        .text_xs(),
                    );
                }
                for (section, slug, title) in SECTIONS {
                    let rows: Vec<_> = group.rows.iter().filter(|r| r.2 == section).collect();
                    if rows.is_empty() {
                        continue;
                    }
                    column = column.child(div().pt_1().text_xs().text_color(rgb(0x9eacb9)).child(
                        crate::accessibility::named(
                            format!("project-{key}-{slug}"),
                            format!("{title} · {}", rows.len()),
                        ),
                    ));
                    for (task, run, _) in rows {
                        column = column.child(self.ticket_row(
                            task,
                            run.as_ref(),
                            group.project.as_ref(),
                            cx,
                        ));
                    }
                }
                // Shelves start collapsed; each project remembers which are open.
                for shelf in [Shelf::Snoozed, Shelf::Settled] {
                    let rows: Vec<_> = group.shelved.iter().filter(|r| r.2 == shelf).collect();
                    if rows.is_empty() {
                        continue;
                    }
                    let (open, header) = self.shelf_header(&key, shelf, rows.len(), cx);
                    column = column.child(div().pt_1().flex().child(header));
                    if open {
                        for (task, run, _) in rows {
                            column = column.child(self.ticket_row(
                                task,
                                run.as_ref(),
                                group.project.as_ref(),
                                cx,
                            ));
                        }
                    }
                }
                column
            })
            .collect()
    }

    fn project_header(
        &self,
        group: &Group,
        collapsed: bool,
        active: &[String],
        cx: &mut Context<Self>,
    ) -> Div {
        let key = group.key.clone();
        let toggle = key.clone();
        let (name, monogram_text) = match &group.project {
            Some(p) => (string(p, "name"), monogram(&string(p, "name"))),
            None => ("Unassigned".to_owned(), "–".to_owned()),
        };
        let retired = group
            .project
            .as_ref()
            .is_some_and(|p| !p["retiredAt"].is_null());
        // Snoozed and settled tickets still run; the indicator counts them too.
        let running = group
            .rows
            .iter()
            .map(|(_, run, _)| run)
            .chain(group.shelved.iter().map(|(_, run, _)| run))
            .filter(|run| Self::group(run.as_ref()) == shell::Group::Active)
            .count();
        let title = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new(SharedString::from(format!("project-collapse-{key}")))
                    .ghost()
                    .label(if collapsed { "▸" } else { "▾" })
                    .accessibility_label(format!(
                        "{} {name}",
                        if collapsed { "Expand" } else { "Collapse" }
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = toggle.clone();
                        this.save_projects(|p| {
                            if let Some(i) = p.collapsed.iter().position(|c| *c == id) {
                                p.collapsed.remove(i);
                            } else {
                                p.collapsed.push(id);
                            }
                        });
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(0x223b46))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(crate::accessibility::named(
                        format!("project-monogram-{key}"),
                        monogram_text,
                    )),
            )
            .child({
                let label = format!("{name}{}", if retired { " · Retired" } else { "" });
                match &group.project {
                    Some(_) if !retired => {
                        let id = key.clone();
                        Button::new(SharedString::from(format!("project-name-{key}")))
                            .ghost()
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.save_projects(|p| {
                                    p.current = Some(id.clone());
                                    p.collapsed.retain(|c| *c != id);
                                });
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                    _ => div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(crate::accessibility::named(
                            format!("project-name-{key}"),
                            label,
                        ))
                        .into_any_element(),
                }
            });
        let mut facts = div().flex().flex_wrap().gap_2().text_xs();
        if let Some(project) = group.project.as_ref().filter(|_| !retired) {
            let (source, tone) = source_status(project);
            facts = facts.child(
                div()
                    .text_color(rgb(tone))
                    .child(crate::accessibility::named(
                        format!("project-source-{key}"),
                        source,
                    )),
            );
        }
        facts = facts.child(
            div()
                .text_color(rgb(if running > 0 { 0x84d7bc } else { 0x9eacb9 }))
                .child(crate::accessibility::named(
                    format!("project-running-{key}"),
                    if running > 0 {
                        format!("● {running} running")
                    } else {
                        "Nothing running".into()
                    },
                )),
        );
        let mut header = div().flex().flex_col().gap_1().child(title).child(facts);
        let Some(project) = group.project.as_ref().filter(|_| !retired) else {
            return header;
        };
        let mut actions = div().flex().flex_wrap().gap_1();
        let id = key.clone();
        actions = actions.child(
            Button::new(SharedString::from(format!("project-new-{key}")))
                .ghost()
                .label("New ticket")
                .accessibility_label(format!("New ticket in {name}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_current_project(Some(id.clone()));
                    this.new_project = Some(id.clone());
                    this.panel = Panel::New;
                    this.navigation_open = false;
                    cx.notify();
                })),
        );
        let source = &project["projectSource"];
        if source["state"] == "confirmed"
            && matches!(source["saved"]["tickets"].as_str(), Some("github" | "jira"))
        {
            let id = key.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("project-import-{key}")))
                    .ghost()
                    .label("Import")
                    .accessibility_label(format!("Import tickets into {name}"))
                    .disabled(self.blocked())
                    .on_click(cx.listener(move |this, _, _, cx| this.import_into(&id, cx))),
            );
        }
        let position = active.iter().position(|a| *a == key);
        for (slug, glyph, word, step) in [("up", "↑", "up", -1isize), ("down", "↓", "down", 1)]
        {
            let target = position.and_then(|i| i.checked_add_signed(step));
            let order = active.to_vec();
            let from = position;
            actions = actions.child(
                Button::new(SharedString::from(format!("project-{slug}-{key}")))
                    .ghost()
                    .label(glyph)
                    .accessibility_label(format!("Move {name} {word}"))
                    .disabled(target.is_none_or(|t| t >= active.len()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let (Some(from), Some(to)) = (from, target) {
                            let mut order = order.clone();
                            order.swap(from, to);
                            this.save_projects(|p| p.order = order);
                            cx.notify();
                        }
                    })),
            );
        }
        header = header.child(actions);
        header
    }

    /// Runs the project's own ticket-source import. A Jira project without saved filters opens
    /// its settings instead, since a refresh needs them.
    pub(super) fn import_into(&mut self, id: &str, cx: &mut Context<Self>) {
        self.set_current_project(Some(id.to_owned()));
        let project = records(&self.workspace, "repositories")
            .into_iter()
            .find(|r| r["id"] == id);
        let tickets = project.as_ref().and_then(|p| {
            p["projectSource"]["saved"]["tickets"]
                .as_str()
                .map(str::to_owned)
        });
        match tickets.as_deref() {
            Some("github")
                if github_issues::project_view(&self.workspace, id)
                    .is_some_and(|v| v["state"].as_str().is_some_and(|s| !s.is_empty())) =>
            {
                self.send(
                    format!("/repositories/{id}/github-issues/sync"),
                    json!({}),
                    false,
                    cx,
                )
            }
            Some("jira")
                if jira_refresh::project_view(&self.workspace, id)
                    .is_some_and(|v| v["hasFilters"] == true) =>
            {
                self.send(
                    format!("/repositories/{id}/jira/sync"),
                    json!({}),
                    false,
                    cx,
                )
            }
            _ => {
                self.settings.repository = id.to_owned();
                self.panel = Panel::Repositories;
                self.navigation_open = false;
                self.fail_at(
                    "Save this project's ticket-source filters below, then import.",
                    feedback::Place::Top,
                );
                cx.notify();
            }
        }
    }

    /// Whether Start can queue this ticket's coordinator: it targets an active project and
    /// has no unfinished run. The service still checks the brief revision and the project.
    pub(super) fn startable(&self, task: &Value, run: Option<&Value>) -> bool {
        self.ticket_project(task).is_some()
            && task["internal"] != true
            && task["needsReacceptance"] != true
            // No run, or only finished ones.
            && Self::group(run) == shell::Group::Other
    }

    /// Start: queues the ticket with its coordinator in its project, with the same launch the
    /// ticket's Queue run builds by default. Without an accepted plan the coordinator scopes it.
    pub(super) fn start_ticket(&mut self, task_id: &str, cx: &mut Context<Self>) {
        let Some(task) = records(&self.workspace, "tasks")
            .into_iter()
            .find(|t| t["id"] == task_id)
        else {
            return;
        };
        self.selected = Some(task_id.to_owned());
        self.panel = Panel::Ticket;
        self.navigation_open = false;
        if let Some(project) = self.ticket_project(&task) {
            self.set_current_project(Some(string(&project, "id")));
        }
        let role = self.coordinator_role(task_id, None);
        if role.is_empty() {
            self.tab = "Brief";
            self.fail_at(
                "Enable an agent role to coordinate this ticket, then start it.",
                feedback::Place::Top,
            );
            cx.notify();
            return;
        }
        let launch = self.coordinator_launch(&task, &role);
        let mut body = json!({"revision":task["revision"],"launch":launch});
        if let Some(playbook) = self
            .planning
            .playbooks
            .get(task_id)
            .filter(|p| !p.is_empty())
        {
            body["playbookId"] = json!(playbook);
        }
        self.send(format!("/tasks/{task_id}/runs"), body, true, cx);
    }

    /// The empty workspace: no project yet. Add a project opens the choose-folder flow.
    pub(super) fn empty_workspace(&self, cx: &mut Context<Self>) -> Div {
        section("What should we work on?")
            .child(named_hint(
                "empty-workspace",
                "Add a project to begin: choose its checkout folder, and the local service inspects it before you save. Tickets created without a project wait under Unassigned.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("empty-add-project")
                            .primary()
                            .label("Add a project")
                            .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
                    )
                    .child(
                        Button::new("empty-new")
                            .label("Create a ticket")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_project = None;
                                this.panel = Panel::New;
                                this.navigation_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("empty-import")
                            .label("Import from Jira")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Import;
                                this.navigation_open = false;
                                cx.notify();
                            })),
                    ),
            )
    }
}
