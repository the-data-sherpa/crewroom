//! Settings → Integrations → GitHub Issues (FOR-45): the repository whose
//! issues become tickets, its filters, and the last sync result as the local
//! service reports it. GitHub's open/closed state appears on each ticket as its
//! source status; it never changes workflow state or agent activity.
use super::*;

pub(super) struct IssueSourceInputs {
    pub(super) repository: Entity<InputState>,
    pub(super) labels: Entity<InputState>,
    pub(super) assignee: Entity<InputState>,
    pub(super) milestone: Entity<InputState>,
    pub(super) state: Entity<InputState>,
    /// Optional workflow labels for issue delivery (FOR-48); empty is off.
    pub(super) in_progress: Entity<InputState>,
    pub(super) in_review: Entity<InputState>,
}
impl IssueSourceInputs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let input =
            |placeholder: &'static str, window: &mut Window, cx: &mut Context<Workbench>| {
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
            };
        Self {
            repository: input("owner/repository", window, cx),
            labels: input("Labels, comma-separated (all must match)", window, cx),
            assignee: input(
                "Assignee login, * for anyone, none for unassigned",
                window,
                cx,
            ),
            milestone: input("Milestone title or number, *, or none", window, cx),
            state: input("open, closed or all (default open)", window, cx),
            in_progress: input("In-progress label (empty: off)", window, cx),
            in_review: input("In-review label (empty: off)", window, cx),
        }
    }
    /// The request body for `/github/issues/labels`; an empty field turns that
    /// label off.
    pub(super) fn labels_body(&self, cx: &App) -> Value {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        json!({"inProgress": text(&self.in_progress), "inReview": text(&self.in_review)})
    }
    /// The request body for `/github/issues/source`; empty fields are omitted.
    pub(super) fn body(&self, cx: &App) -> Value {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let mut body = json!({"repository": text(&self.repository)});
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

/// Completion text for a GitHub Issues command; the status line carries detail.
pub(super) fn notice(path: &str, value: &Value) -> Option<String> {
    let issues = &value["githubIssues"];
    Some(match path {
        "/github/issues/source" => {
            "GitHub ticket source saved. Choose Sync issues to import.".into()
        }
        "/github/issues/labels" => issues["labelsNote"]
            .as_str()
            .unwrap_or("GitHub workflow labels saved.")
            .to_owned(),
        "/github/delivery/reconcile" => {
            "GitHub issue updates reconciled. Only confirmed updates reached the issue.".into()
        }
        p if p.starts_with("/github/delivery/") && p.ends_with("/retry") => {
            "Retry requested. Crewroom re-read the issue first, so an update GitHub already has is not posted twice.".into()
        }
        "/github/issues/sync" => match issues["state"].as_str().unwrap_or("") {
            "connected" => {
                "GitHub sync finished. Imported fields stay separate from your refinements.".into()
            }
            _ => {
                "GitHub sync did not complete; nothing partial was imported. See the reason below."
                    .into()
            }
        },
        _ => return None,
    })
}

fn line(id: &str, value: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .text_color(rgb(0x9eacb9))
        .child(crate::accessibility::named(id.to_owned(), value))
}

impl Workbench {
    pub(super) fn github_issues_section(&self, cx: &mut Context<Self>) -> Div {
        let issues = &self.workspace["githubIssues"];
        let state = issues["state"].as_str().unwrap_or("");
        let mut panel = section("GitHub Issues").child(hint(
            "Import a repository's issues as tickets. Pull requests are not imported. GitHub open/closed is shown as each ticket's source status, separate from its workflow state and agent activity.",
        ));
        panel = panel.child(line(
            "github-issues-state",
            issues["label"]
                .as_str()
                .unwrap_or("GitHub Issues status unavailable until the service connects.")
                .to_owned(),
        ));
        if let Some(filters) = issues["filters"].as_str() {
            panel = panel.child(line("github-issues-filters", format!("Filters: {filters}")));
        }
        if let Some(note) = issues["connectionNote"].as_str() {
            panel = panel.child(line("github-issues-connection", note.to_owned()));
        }
        if state != "not_configured" && !state.is_empty() {
            panel = panel.child(line(
                "github-issues-last-sync",
                format!(
                    "Last complete sync: {}",
                    issues["lastSyncedAt"]
                        .as_str()
                        .unwrap_or("not yet recorded")
                ),
            ));
        }
        let inputs = &self.issue_inputs;
        for (label, input, id) in [
            ("Repository", &inputs.repository, "github-issues-repository"),
            ("Labels", &inputs.labels, "github-issues-labels"),
            ("Assignee", &inputs.assignee, "github-issues-assignee"),
            ("Milestone", &inputs.milestone, "github-issues-milestone"),
            ("State", &inputs.state, "github-issues-filter-state"),
        ] {
            panel = panel
                .child(crate::accessibility::text(label))
                .child(Input::new(input).id(id).disabled(self.blocked()));
        }
        panel
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("save-github-source")
                            .primary()
                            .label("Save source")
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let body = this.issue_inputs.body(cx);
                                if body["repository"] == "" {
                                    this.fail_at(
                                        "Enter the GitHub repository as owner/name.",
                                        feedback::Place::Top,
                                    );
                                    cx.notify();
                                    return;
                                }
                                this.send("/github/issues/source".into(), body, false, cx);
                            })),
                    )
                    .child(
                        Button::new("sync-github-issues")
                            .label("Sync issues")
                            .disabled(self.blocked() || matches!(state, "" | "not_configured"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send("/github/issues/sync".into(), json!({}), false, cx)
                            })),
                    ),
            )
            .child(self.github_labels(state, cx))
    }

    /// Issue delivery (FOR-48): progress reaches the issue as comments; the
    /// optional workflow labels apply only when named here.
    fn github_labels(&self, state: &str, cx: &mut Context<Self>) -> Div {
        let issues = &self.workspace["githubIssues"];
        let mut labels = div().flex().flex_col().gap_1().child(hint(
            "Issue delivery posts milestone, blocker, evidence, PR-link and research-outcome comments on each GitHub-sourced ticket's issue. GitHub closes the issue when the PR that says Closes #N merges; Crewroom records that state without closing it itself.",
        ));
        if let Some(note) = issues["labelsNote"].as_str() {
            labels = labels.child(line("github-issues-labels-note", note.to_owned()));
        }
        let inputs = &self.issue_inputs;
        for (label, input, id) in [
            (
                "In-progress label",
                &inputs.in_progress,
                "github-issues-in-progress-label",
            ),
            (
                "In-review label",
                &inputs.in_review,
                "github-issues-in-review-label",
            ),
        ] {
            labels = labels
                .child(crate::accessibility::text(label))
                .child(Input::new(input).id(id).disabled(self.blocked()));
        }
        labels.child(
            Button::new("save-github-labels")
                .label("Save workflow labels")
                .disabled(self.blocked() || matches!(state, "" | "not_configured"))
                .on_click(cx.listener(|this, _, _, cx| {
                    let body = this.issue_inputs.labels_body(cx);
                    this.send("/github/issues/labels".into(), body, false, cx);
                })),
        )
    }
}
