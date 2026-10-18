//! Notices and errors belong to the panel, ticket and control whose action produced them.
//! Connection errors describe the service and stay global.
use super::*;

/// The control area whose action produced the feedback.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Place {
    /// The top of the panel that sent the command.
    Top,
    /// Beside the coordinator steering composer.
    Composer,
    /// Above the Save row of a long settings form.
    SaveRow,
    /// The top of the wide details drawer.
    Drawer,
}
impl Place {
    fn name(self) -> &'static str {
        match self {
            Place::Top => "top",
            Place::Composer => "composer",
            Place::SaveRow => "save-row",
            Place::Drawer => "drawer",
        }
    }
}
#[derive(Clone, PartialEq)]
pub(super) struct Origin {
    panel: Panel,
    task: Option<String>,
    comparison: bool,
    place: Place,
}
/// Session-column commands; other ticket commands come from the details drawer while it is open.
const SESSION: [&str; 9] = [
    "/pause",
    "/resume",
    "/cancel",
    "/budget",
    "/continue-later",
    "/repair-allowance",
    "/playbook",
    "/specialists",
    "/steering",
];
/// Settings saves and deletes whose controls sit at the bottom of a long form.
fn save_row(panel: Panel, path: &str) -> bool {
    let parts: Vec<_> = path.split('/').skip(1).collect();
    matches!(
        (panel, parts.as_slice()),
        (Panel::Repositories, ["repositories"] | ["repositories", _])
            | (Panel::Roles, ["roles"] | ["roles", _])
            | (
                Panel::Principles,
                ["principles"] | ["principles", _, "delete"]
            )
    )
}
/// Pending text names the command instead of a generic save.
pub(super) fn pending(path: &str, body: &Value) -> String {
    if path.starts_with("/comparisons/") && path.ends_with("/control") {
        return match body["action"].as_str().unwrap_or("") {
            "start" => "Starting comparison…",
            "pause" => "Pausing comparison…",
            "resume" => "Resuming comparison…",
            "cancel" => "Cancelling remaining cells…",
            _ => "Recording comparison control…",
        }
        .into();
    }
    let exact = [
        ("/tasks", "Creating ticket…"),
        ("/import/jira", "Importing snapshot…"),
        ("/jira/sync", "Refreshing Jira…"),
        ("/jira/reconcile", "Reconciling Jira…"),
        ("/queue/order", "Saving queue order…"),
        ("/role-preview", "Preparing preview…"),
        ("/role-drafts", "Starting drafting session…"),
        ("/learning", "Recording proposal…"),
        ("/model-catalog/refresh", "Requesting model refresh…"),
        ("/comparisons/preflight", "Preparing preflight…"),
        ("/comparisons", "Saving comparison…"),
        ("/github/connect", "Checking the token with GitHub…"),
        ("/github/check", "Checking with GitHub…"),
        ("/github/disconnect", "Disconnecting GitHub…"),
        (
            "/github/delivery/reconcile",
            "Reconciling GitHub issue updates…",
        ),
    ];
    if let Some((_, text)) = exact.iter().find(|(p, _)| *p == path) {
        return (*text).into();
    }
    let suffixes = [
        ("/github-issues/sync", "Syncing GitHub issues…"),
        ("/github-issues/filters", "Saving GitHub issue filters…"),
        ("/github-issues/labels", "Saving GitHub workflow labels…"),
        ("/retire", "Retiring…"),
        ("/delete", "Deleting…"),
        ("/decision", "Recording decision…"),
        ("/publication/reconcile", "Reconciling publication…"),
        ("/retry", "Retrying GitHub update…"),
        ("/steering", "Storing instruction…"),
        ("/pause", "Requesting pause…"),
        ("/resume", "Requesting resume…"),
        ("/cancel", "Requesting cancellation…"),
        ("/remove", "Cancelling queued plan…"),
        ("/research-runs", "Queueing research…"),
        ("/refinement-runs", "Queueing refinement…"),
        ("/runs", "Queueing plan…"),
        ("/start", "Starting…"),
        ("/close", "Closing run…"),
        ("/target-refresh", "Requesting target refresh…"),
        ("/check-environment", "Correcting check environment…"),
        ("/repair-allowance", "Allowing one more cycle…"),
        ("/specialists", "Saving specialist proposal…"),
        ("/adopt-requirements", "Adopting requirements…"),
        ("/continue-later", "Saving linked successor…"),
        ("/adoption", "Associating saved revision…"),
        ("/revalidate", "Revalidating…"),
    ];
    suffixes
        .iter()
        .find(|(suffix, _)| path.ends_with(suffix))
        .map_or("Saving…", |(_, text)| text)
        .into()
}
impl Workbench {
    /// The current view, with the control area that sends `path`.
    pub(super) fn origin(&self, path: &str) -> Origin {
        let place = if path.ends_with("/steering") {
            Place::Composer
        } else if save_row(self.panel, path) {
            Place::SaveRow
        } else if self.panel == Panel::Ticket
            && self.tab != "Conversation"
            && !(path.starts_with("/runs/") && SESSION.iter().any(|s| path.ends_with(s)))
        {
            Place::Drawer
        } else {
            Place::Top
        };
        self.origin_at(place)
    }
    pub(super) fn origin_at(&self, place: Place) -> Origin {
        Origin {
            panel: self.panel,
            task: (self.panel == Panel::Ticket)
                .then(|| self.selected.clone())
                .flatten(),
            comparison: self.comparisons.open,
            place,
        }
    }
    /// Shows a locally detected error at the control that produced it.
    pub(super) fn fail_at(&mut self, error: impl Into<String>, place: Place) {
        self.error = error.into();
        self.notice.clear();
        self.feedback_origin = Some(self.origin_at(place));
    }
    fn shown_here(&self) -> bool {
        self.feedback_origin.as_ref().is_none_or(|origin| {
            origin.panel == self.panel
                && origin.comparison == self.comparisons.open
                && (self.panel != Panel::Ticket || origin.task == self.selected)
        })
    }
    fn feedback_element(&self, place: Place) -> Option<Div> {
        if self.error.is_empty() && self.notice.is_empty() {
            return None;
        }
        let mut column = div().flex().flex_col().gap_2();
        if !self.error.is_empty() {
            column = column.child(crate::accessibility::observable(
                div()
                    .id(SharedString::from(format!(
                        "feedback-error-{}",
                        place.name()
                    )))
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0x462c2e))
                    .text_color(rgb(0xffc4b8))
                    .child(crate::accessibility::text(self.error.clone())),
            ));
        }
        if !self.notice.is_empty() {
            column = column.child(crate::accessibility::observable(
                div()
                    .id(SharedString::from(format!(
                        "feedback-notice-{}",
                        place.name()
                    )))
                    .child(hint(self.notice.clone())),
            ));
        }
        Some(column)
    }
    /// Feedback for the control area at `place`, when the current view owns it.
    pub(super) fn feedback(&self, place: Place) -> Option<Div> {
        let origin = self.feedback_origin.as_ref()?;
        if origin.place != place || place == Place::Top || !self.shown_here() {
            return None;
        }
        let element = self.feedback_element(place)?;
        self.feedback_placed.set(true);
        Some(element)
    }
    /// Panel-top feedback, including anything whose own control is not rendered.
    pub(super) fn top_feedback(&self) -> Option<Div> {
        if self.feedback_placed.get() || !self.shown_here() {
            return None;
        }
        self.feedback_element(Place::Top)
    }
    pub(super) fn plan_decision_notice(&self, value: &Value) -> String {
        let view = self.workspace["intake"]
            .as_object()
            .into_iter()
            .flat_map(|views| views.values())
            .find(|view| {
                records(view, "plans")
                    .iter()
                    .any(|p| p["id"] == value["planId"])
            });
        let version = view
            .and_then(|view| {
                records(view, "plans")
                    .into_iter()
                    .find(|p| p["id"] == value["planId"])
            })
            .and_then(|plan| plan["version"].as_u64());
        let plan = version.map_or("Plan".into(), |v| format!("Plan {v}"));
        if value["decision"] == "accept" {
            format!("{plan} accepted.")
        } else if view.is_some_and(|view| view["intake"].is_object()) {
            format!("{plan} rejected. A revision was requested.")
        } else {
            format!("{plan} rejected. No agent will revise it.")
        }
    }
}
