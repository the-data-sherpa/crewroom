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
        }
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
        panel.child(
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
    }
}
