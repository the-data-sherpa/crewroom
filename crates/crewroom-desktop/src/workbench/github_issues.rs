//! GitHub Issues per project (FOR-45). A project whose confirmed ticket source
//! (FOR-47) is a GitHub repository sets its own filters, syncs, and shows its
//! last sync result as the local service reports it, in its row under
//! Settings → Projects. Settings → Integrations summarises every project.
//! GitHub's open/closed state appears on each ticket as its source status; it
//! never changes workflow state or agent activity.
use super::*;

/// One project's filter and workflow-label inputs, prefilled from its saved
/// source. The repository is the confirmed one, so it is not typed here.
pub(super) struct IssueSourceInputs {
    pub(super) labels: Entity<InputState>,
    pub(super) assignee: Entity<InputState>,
    pub(super) milestone: Entity<InputState>,
    pub(super) state: Entity<InputState>,
    /// Optional workflow labels for issue delivery (FOR-48); empty is off.
    pub(super) in_progress: Entity<InputState>,
    pub(super) in_review: Entity<InputState>,
}
impl IssueSourceInputs {
    pub(super) fn new(view: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let source = &view["source"];
        let labels = source["labels"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let mut input = |placeholder: &'static str, value: String| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let text = |value: &Value| value.as_str().unwrap_or("").to_owned();
        Self {
            labels: input("Labels, comma-separated (all must match)", labels),
            assignee: input(
                "Assignee login, * for anyone, none for unassigned",
                text(&source["assignee"]),
            ),
            milestone: input(
                "Milestone title or number, *, or none",
                text(&source["milestone"]),
            ),
            state: input("open, closed or all (default open)", text(&source["state"])),
            in_progress: input(
                "In-progress label (empty: off)",
                text(&view["statusLabels"]["inProgress"]),
            ),
            in_review: input(
                "In-review label (empty: off)",
                text(&view["statusLabels"]["inReview"]),
            ),
        }
    }
    /// The request body for `…/github-issues/labels`; an empty field turns
    /// that label off.
    pub(super) fn labels_body(&self, cx: &App) -> Value {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        json!({"inProgress": text(&self.in_progress), "inReview": text(&self.in_review)})
    }
    /// The request body for `…/github-issues/filters`; empty fields are omitted.
    pub(super) fn body(&self, cx: &App) -> Value {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let mut body = json!({});
        for (key, input) in [
            ("labels", &self.labels),
            ("assignee", &self.assignee),
            ("milestone", &self.milestone),
            ("state", &self.state),
        ] {
            let value = text(input);
            if !value.is_empty() {
                body[key] = json!(value);
            }
        }
        body
    }
}

/// The project a `/repositories/{id}/github-issues/{action}` path names.
fn route(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/repositories/")?;
    let (id, action) = rest.split_once("/github-issues/")?;
    (!id.is_empty() && !id.contains('/')).then_some((id, action))
}

/// One project's status in a response or workspace.
pub(super) fn project_view<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
    value["githubIssues"]["projects"]
        .as_array()?
        .iter()
        .find(|p| p["repositoryId"] == id)
}

/// Completion text for a GitHub Issues command; the status line carries detail.
pub(super) fn notice(path: &str, value: &Value) -> Option<String> {
    if let Some((id, action)) = route(path) {
        let project = project_view(value, id).cloned().unwrap_or_default();
        return Some(match action {
            "filters" => {
                "GitHub filters saved for this project. Choose Sync issues to import.".into()
            }
            "labels" => project["labelsNote"]
                .as_str()
                .unwrap_or("GitHub workflow labels saved.")
                .to_owned(),
            "sync" if project["state"] == "connected" => {
                "GitHub sync finished. Imported fields stay separate from your refinements.".into()
            }
            "sync" => {
                "GitHub sync did not complete; nothing partial was imported. See the reason below."
                    .into()
            }
            _ => return None,
        });
    }
    Some(match path {
        "/github/delivery/reconcile" => {
            "GitHub issue updates reconciled. Only confirmed updates reached the issue.".into()
        }
        p if p.starts_with("/github/delivery/") && p.ends_with("/retry") => {
            "Retry requested. Crewroom re-read the issue first, so an update GitHub already has is not posted twice.".into()
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
    /// Settings → Integrations: every project's GitHub Issues status, and a
    /// former workspace-level source no project has adopted. Filters and sync
    /// live with each project under Settings → Projects.
    pub(super) fn github_issues_section(&self, _cx: &mut Context<Self>) -> Div {
        let issues = &self.workspace["githubIssues"];
        let mut panel = section("GitHub Issues").child(hint(
            "Each project whose ticket source is a GitHub repository imports its issues as tickets, with its own filters, under Settings → Projects. Pull requests are not imported. GitHub open/closed is shown as each ticket's source status, separate from its workflow state and agent activity.",
        ));
        panel = panel.child(line(
            "github-issues-state".into(),
            issues["label"]
                .as_str()
                .unwrap_or("GitHub Issues status unavailable until the service connects.")
                .to_owned(),
        ));
        for project in issues["projects"].as_array().into_iter().flatten() {
            let id = project["repositoryId"].as_str().unwrap_or("");
            panel = panel.child(line(
                format!("github-issues-{id}-summary"),
                format!(
                    "{}: {}",
                    project["project"].as_str().unwrap_or("Project"),
                    project["label"].as_str().unwrap_or("")
                ),
            ));
        }
        if let Some(legacy) = issues["legacy"]["label"].as_str() {
            panel = panel.child(line("github-issues-legacy".into(), legacy.to_owned()));
        }
        if let Some(note) = issues["connectionNote"].as_str() {
            panel = panel.child(line("github-issues-connection".into(), note.to_owned()));
        }
        panel
    }

    /// A project's GitHub Issues filters, sync and workflow labels, shown in
    /// its Settings → Projects row once GitHub Issues is its confirmed
    /// ticket source.
    pub(super) fn github_issues_block(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(view) = project_view(&self.workspace, id).cloned() else {
            return div().child(line(
                format!("github-issues-{id}-state"),
                "GitHub Issues status unavailable until the service reports this project.",
            ));
        };
        let state = view["state"].as_str().unwrap_or("").to_owned();
        let mut block = div().flex().flex_col().gap_1().child(hint(
            "GitHub Issues for this project: pull requests are not imported, and GitHub open/closed is each ticket's source status only.",
        ));
        block = block.child(line(
            format!("github-issues-{id}-state"),
            view["label"].as_str().unwrap_or("").to_owned(),
        ));
        if let Some(filters) = view["filters"].as_str() {
            block = block.child(line(
                format!("github-issues-{id}-filters"),
                format!("Filters: {filters}"),
            ));
        }
        if let Some(note) = view["connectionNote"].as_str() {
            block = block.child(line(
                format!("github-issues-{id}-connection"),
                note.to_owned(),
            ));
        }
        if view["adoptedFrom"] == "workspace" {
            block = block.child(line(
                format!("github-issues-{id}-adopted"),
                "Filters and workflow labels carried over from the former workspace-level GitHub Issues setting.",
            ));
        }
        block = block.child(line(
            format!("github-issues-{id}-last-sync"),
            format!(
                "Last complete sync: {}",
                view["lastSyncedAt"].as_str().unwrap_or("not yet recorded")
            ),
        ));
        if !self.issue_inputs.contains_key(id) {
            let inputs = IssueSourceInputs::new(&view, window, cx);
            self.issue_inputs.insert(id.to_owned(), inputs);
        }
        let inputs = &self.issue_inputs[id];
        for (label, input, field) in [
            ("Labels", &inputs.labels, "labels"),
            ("Assignee", &inputs.assignee, "assignee"),
            ("Milestone", &inputs.milestone, "milestone"),
            ("State", &inputs.state, "filter-state"),
        ] {
            block = block.child(crate::accessibility::text(label)).child(
                Input::new(input)
                    .id(SharedString::from(format!("github-issues-{id}-{field}")))
                    .disabled(self.blocked()),
            );
        }
        let (filters_target, sync_target) = (id.to_owned(), id.to_owned());
        block = block.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("save-github-filters-{id}")))
                        .label("Save filters")
                        .disabled(self.blocked())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let body = this.issue_inputs[&filters_target].body(cx);
                            this.send(
                                format!("/repositories/{filters_target}/github-issues/filters"),
                                body,
                                false,
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("sync-github-issues-{id}")))
                        .primary()
                        .label("Sync issues")
                        .disabled(self.blocked() || state.is_empty())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                format!("/repositories/{sync_target}/github-issues/sync"),
                                json!({}),
                                false,
                                cx,
                            )
                        })),
                ),
        );
        block.child(self.github_labels(id, &view, cx))
    }

    /// Issue delivery (FOR-48): progress reaches the issue as comments; the
    /// optional workflow labels apply only when named here.
    fn github_labels(&self, id: &str, view: &Value, cx: &mut Context<Self>) -> Div {
        let mut labels = div().flex().flex_col().gap_1().child(hint(
            "Issue delivery posts milestone, blocker, evidence, PR-link and research-outcome comments on each GitHub-sourced ticket's issue. GitHub closes the issue when the PR that says Closes #N merges; Crewroom records that state without closing it itself.",
        ));
        if let Some(note) = view["labelsNote"].as_str() {
            labels = labels.child(line(
                format!("github-issues-{id}-labels-note"),
                note.to_owned(),
            ));
        }
        let inputs = &self.issue_inputs[id];
        for (label, input, field) in [
            (
                "In-progress label",
                &inputs.in_progress,
                "in-progress-label",
            ),
            ("In-review label", &inputs.in_review, "in-review-label"),
        ] {
            labels = labels.child(crate::accessibility::text(label)).child(
                Input::new(input)
                    .id(SharedString::from(format!("github-issues-{id}-{field}")))
                    .disabled(self.blocked()),
            );
        }
        let target = id.to_owned();
        labels.child(
            Button::new(SharedString::from(format!("save-github-labels-{id}")))
                .label("Save workflow labels")
                .disabled(self.blocked())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let body = this.issue_inputs[&target].labels_body(cx);
                    this.send(
                        format!("/repositories/{target}/github-issues/labels"),
                        body,
                        false,
                        cx,
                    );
                })),
        )
    }
}
