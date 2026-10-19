//! Jira refresh per project (FOR-53). A project whose confirmed ticket source
//! (FOR-47) is a Jira site sets its own filters (project key, saved filter,
//! assignee, JQL), refreshes, and shows its last refresh result and whether
//! Jira is connected for its site, in its row under Settings → Projects.
//! Settings → Integrations summarises every project. Jira status appears on
//! each ticket as its source status; it never changes workflow state or agent
//! activity.
use super::*;

/// One project's Jira filter inputs, prefilled from its saved source. The
/// site is the confirmed one, so it is not typed here.
pub(super) struct JiraFilterInputs {
    pub(super) project_key: Entity<InputState>,
    pub(super) filter_id: Entity<InputState>,
    pub(super) assignee: Entity<InputState>,
    pub(super) jql: Entity<InputState>,
}
impl JiraFilterInputs {
    pub(super) fn new(view: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let source = &view["source"];
        let mut input = |placeholder: &'static str, key: &str| {
            let value = source[key].as_str().unwrap_or("").to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        Self {
            project_key: input("Jira project key, for example CR", "projectKey"),
            filter_id: input("Saved filter ID (digits)", "filterId"),
            assignee: input("Assignee account ID (optional)", "assignee"),
            jql: input("Extra JQL, without ORDER BY (optional)", "jql"),
        }
    }
    /// The request body for `…/jira/filters`; empty fields are omitted.
    pub(super) fn body(&self, cx: &App) -> Value {
        let mut body = json!({});
        for (key, input) in [
            ("projectKey", &self.project_key),
            ("filterId", &self.filter_id),
            ("assignee", &self.assignee),
            ("jql", &self.jql),
        ] {
            let value = input.read(cx).value().trim().to_owned();
            if !value.is_empty() {
                body[key] = json!(value);
            }
        }
        body
    }
}

/// The project a `/repositories/{id}/jira/{action}` path names.
fn route(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/repositories/")?;
    let (id, action) = rest.split_once("/jira/")?;
    (!id.is_empty() && !id.contains('/')).then_some((id, action))
}

/// One project's status in a response or workspace.
pub(super) fn project_view<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
    value["jiraRefresh"]["projects"]
        .as_array()?
        .iter()
        .find(|p| p["repositoryId"] == id)
}

/// Completion text for a Jira refresh command; the status line carries detail.
pub(super) fn notice(path: &str, value: &Value) -> Option<String> {
    if path == "/jira/sync" {
        let count = value["refreshed"].as_array().map_or(0, Vec::len);
        return Some(match count {
            0 => "No project uses the connected Jira site, so nothing was refreshed.".into(),
            n => format!(
                "Jira refresh finished for {n} project{}. Check each project's status for completeness.",
                if n == 1 { "" } else { "s" }
            ),
        });
    }
    let (id, action) = route(path)?;
    let project = project_view(value, id).cloned().unwrap_or_default();
    Some(match action {
        "filters" => "Jira filters saved for this project. Choose Refresh Jira to import.".into(),
        "sync" if project["state"] == "connected" => {
            "Jira refresh finished. Imported fields stay separate from your refinements.".into()
        }
        "sync" => {
            "Jira refresh did not complete; nothing partial was imported. See the reason below."
                .into()
        }
        _ => return None,
    })
}

fn line(id: String, value: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .text_color(rgb(0x9eacb9))
        .child(crate::accessibility::named(id, value))
}

impl Workbench {
    /// Settings → Integrations: the Jira connection, every project's refresh
    /// status, and a workspace-level setting no project uses. Filters and
    /// refresh live with each project under Settings → Projects.
    pub(super) fn jira_refresh_section(&self) -> Div {
        let refresh = &self.workspace["jiraRefresh"];
        let mut panel = div().flex().flex_col().gap_1().child(line(
            "jira-refresh-state".into(),
            refresh["label"]
                .as_str()
                .unwrap_or("Jira refresh status unavailable until the service connects.")
                .to_owned(),
        ));
        if let Some(connection) = refresh["connectionLabel"].as_str() {
            panel = panel.child(line(
                "jira-refresh-connection".into(),
                connection.to_owned(),
            ));
        }
        for project in refresh["projects"].as_array().into_iter().flatten() {
            let id = project["repositoryId"].as_str().unwrap_or("");
            let mut text = format!(
                "{}: {}",
                project["project"].as_str().unwrap_or("Project"),
                project["label"].as_str().unwrap_or("")
            );
            if let Some(note) = project["connectionNote"].as_str() {
                text = format!("{text} {note}");
            }
            panel = panel.child(line(format!("jira-refresh-{id}-summary"), text));
        }
        if let Some(legacy) = refresh["legacy"]["label"].as_str() {
            panel = panel.child(line("jira-refresh-legacy".into(), legacy.to_owned()));
        }
        panel
    }

    /// A project's Jira filters and refresh, shown in its Settings →
    /// Repositories row once a Jira site is its confirmed ticket source.
    pub(super) fn jira_refresh_block(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(view) = project_view(&self.workspace, id).cloned() else {
            return div().child(line(
                format!("jira-refresh-{id}-state"),
                "Jira refresh status unavailable until the service reports this project.",
            ));
        };
        let mut block = div().flex().flex_col().gap_1().child(hint(
            "Jira refresh for this project: its own filters on its confirmed site. Jira status is each ticket's source status only.",
        ));
        block = block.child(line(
            format!("jira-refresh-{id}-state"),
            view["label"].as_str().unwrap_or("").to_owned(),
        ));
        block = block.child(line(
            format!("jira-refresh-{id}-filters"),
            format!(
                "Filters: {}",
                view["filters"].as_str().unwrap_or("none saved")
            ),
        ));
        if let Some(note) = view["connectionNote"].as_str() {
            block = block.child(line(
                format!("jira-refresh-{id}-connection"),
                note.to_owned(),
            ));
        }
        if view["adoptedFrom"] == "workspace" {
            block = block.child(line(
                format!("jira-refresh-{id}-adopted"),
                "Filters carried over from the former workspace-level Jira refresh setting.",
            ));
        }
        block = block.child(line(
            format!("jira-refresh-{id}-last-sync"),
            format!(
                "Last complete refresh: {}",
                view["lastSyncedAt"].as_str().unwrap_or("not yet recorded")
            ),
        ));
        if !self.jira_inputs.contains_key(id) {
            let inputs = JiraFilterInputs::new(&view, window, cx);
            self.jira_inputs.insert(id.to_owned(), inputs);
        }
        let inputs = &self.jira_inputs[id];
        for (label, input, field) in [
            ("Project key", &inputs.project_key, "project-key"),
            ("Saved filter ID", &inputs.filter_id, "filter-id"),
            ("Assignee account ID", &inputs.assignee, "assignee"),
            ("JQL", &inputs.jql, "jql"),
        ] {
            block = block.child(crate::accessibility::text(label)).child(
                Input::new(input)
                    .id(SharedString::from(format!("jira-refresh-{id}-{field}")))
                    .disabled(self.blocked()),
            );
        }
        let (filters_target, sync_target) = (id.to_owned(), id.to_owned());
        // Refresh stays available without a connection so the reason is
        // recorded; it is disabled only until filters are saved.
        let can_refresh = view["hasFilters"] == true;
        block.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("save-jira-filters-{id}")))
                        .label("Save Jira filters")
                        .disabled(self.blocked())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let body = this.jira_inputs[&filters_target].body(cx);
                            this.send(
                                format!("/repositories/{filters_target}/jira/filters"),
                                body,
                                false,
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("refresh-jira-{id}")))
                        .primary()
                        .label("Refresh Jira")
                        .disabled(self.blocked() || !can_refresh)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                format!("/repositories/{sync_target}/jira/sync"),
                                json!({}),
                                false,
                                cx,
                            )
                        })),
                ),
        )
    }
}
