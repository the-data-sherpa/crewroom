#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod agent_state_tests;
mod agents;
mod allowance;
#[cfg(test)]
mod blocker_tests;
mod blockers;
#[cfg(test)]
mod change_tests;
mod collaboration;
mod comparisons;
#[cfg(test)]
mod composition_tests;
mod credentials;
#[cfg(test)]
mod credentials_tests;
#[cfg(test)]
mod delivery_wording_tests;
#[cfg(test)]
mod demo_seed;
mod feedback;
#[cfg(test)]
mod feedback_tests;
#[cfg(test)]
mod focus_tests;
mod forms;
mod github_issues;
#[cfg(test)]
mod github_issues_tests;
mod history;
mod intake;
mod jira_refresh;
#[cfg(test)]
mod jira_refresh_tests;
#[cfg(test)]
mod journey_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod loading_tests;
mod model_details;
mod navigation;
#[cfg(test)]
mod navigation_cue_tests;
mod needs;
mod observation;
mod overview;
#[cfg(test)]
mod overview_tests;
#[cfg(test)]
mod plan_authoring_tests;
#[cfg(test)]
mod plan_disclosure_tests;
mod planning;
mod playbooks;
mod project_source;
#[cfg(test)]
mod project_source_tests;
mod projects;
#[cfg(test)]
mod projects_tests;
mod results;
mod run_actions;
mod settings;
mod shell;
mod shelves;
#[cfg(test)]
mod shelves_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod wall_entry_tests;
gpui_kit::actions!(
    crewroom_forms,
    [
        NextField,
        PreviousField,
        FocusTickets,
        FocusSession,
        ShowNeedsYou
    ]
);
use crate::client::{Client, Event};
use crewroom_core::ticket_source;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::WindowExt;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputState, Textarea, TextareaState},
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Div, Entity, FontWeight, IntoElement, Render, SharedString, Window,
    div, px, rgb,
};
use run_actions::Confirmation;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Ticket,
    New,
    Import,
    Repositories,
    Roles,
    Models,
    Principles,
    Playbooks,
    Queue,
    Learning,
    NeedsYou,
    Runner,
    /// The home screen across projects (FOR-77).
    Overview,
}
struct Draft {
    brief: Entity<TextareaState>,
    criteria: Entity<TextareaState>,
    revision: u64,
    source_revision: u64,
    original_brief: String,
    original_criteria: String,
}
impl Draft {
    fn edited(&self, cx: &App) -> bool {
        self.brief.read(cx).value().as_ref() != self.original_brief
            || self.criteria.read(cx).value().as_ref() != self.original_criteria
    }
}
pub struct Workbench {
    focus: gpui_kit::FocusHandle,
    /// Keyboard destinations: the ticket list and the session beside it.
    tickets_focus: gpui_kit::FocusHandle,
    session_focus: gpui_kit::FocusHandle,
    /// Scrolls whichever container shows the details, so saved evidence can be revealed.
    details_scroll: gpui_kit::ScrollHandle,
    client: Option<Client>,
    workspace: Value,
    connected: bool,
    busy: bool,
    pending_commands: std::collections::HashSet<String>,
    notice: String,
    error: String,
    /// Where `notice` and `error` belong; `None` shows them on every panel.
    feedback_origin: Option<feedback::Origin>,
    command_origins: HashMap<String, feedback::Origin>,
    feedback_placed: std::cell::Cell<bool>,
    connection_error: String,
    selected: Option<String>,
    selected_runs: HashMap<String, String>,
    navigation: navigation::Navigation,
    observation: observation::Observation,
    panel: Panel,
    navigation_open: bool,
    tab: &'static str,
    drafts: HashMap<String, Draft>,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    import: Entity<TextareaState>,
    credential_inputs: credentials::CredentialInputs,
    issue_inputs: HashMap<String, github_issues::IssueSourceInputs>,
    /// Per-project Jira filter inputs (FOR-53).
    jira_inputs: HashMap<String, jira_refresh::JiraFilterInputs>,
    /// Ticket-source inputs per repository, and repositories whose confirmed
    /// source the operator opened to change (FOR-47).
    source_inputs: HashMap<String, project_source::SourceInputs>,
    source_open: std::collections::HashSet<String>,
    steering: HashMap<String, Entity<TextareaState>>,
    settings: settings::Settings,
    comparisons: comparisons::Comparisons,
    planning: planning::Planning,
    collaboration: collaboration::Collaboration,
    agents: agents::Agents,
    pending_keys: HashMap<String, (String, Value)>,
    reset_new: bool,
    reset_import: bool,
    /// The project a ticket created from New ticket targets; `None` leaves it unassigned.
    new_project: Option<String>,
    /// Snooze choices, Undo and wake timers for the sidebar shelves (FOR-78).
    shelf: shelves::ShelfState,
    _events: Option<gpui_kit::Task<()>>,
    /// Keeps the Overview's elapsed times current between service updates (FOR-77).
    _ticker: Option<gpui_kit::Task<()>>,
    _focus_lost: gpui_kit::Subscription,
}
/// Shown until the first workspace snapshot, in place of empty or unconfigured facts.
const WORKSPACE_WAITING: &str =
    "Waiting for the workflow service. Tickets, queue and decisions appear once it connects.";
fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").into()
}
fn records(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
#[track_caller]
fn section(title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_5()
        .rounded_lg()
        .bg(rgb(0x17212a))
        .border_1()
        .border_color(rgb(0x293642))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(crate::accessibility::text(title)),
        )
}
#[track_caller]
/// A hint tests can locate by a stable identity.
fn named_hint(id: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .text_color(rgb(0x9eacb9))
        .child(crate::accessibility::named(id, value))
}
/// Each call site has its own identity, so sibling hints do not share a node.
#[track_caller]
fn hint(value: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .text_color(rgb(0x9eacb9))
        .child(crate::accessibility::text(value))
}

impl Workbench {
    pub fn new(
        client: Option<Client>,
        events: Option<async_channel::Receiver<Event>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Ordinary workbench forms use Tab for navigation, including multiline fields.
        cx.bind_keys([
            gpui_kit::KeyBinding::new(
                "cmd-enter",
                collaboration::Send,
                Some("CrewroomSteering > Input"),
            ),
            gpui_kit::KeyBinding::new(
                "ctrl-enter",
                collaboration::Send,
                Some("CrewroomSteering > Input"),
            ),
            gpui_kit::KeyBinding::new("tab", NextField, Some("CrewroomSteering > Input")),
            gpui_kit::KeyBinding::new("shift-tab", PreviousField, Some("CrewroomSteering > Input")),
            gpui_kit::KeyBinding::new("tab", NextField, Some("CrewroomForm")),
            gpui_kit::KeyBinding::new("shift-tab", PreviousField, Some("CrewroomForm")),
            gpui_kit::KeyBinding::new("tab", NextField, Some("CrewroomForm > Input")),
            gpui_kit::KeyBinding::new("shift-tab", PreviousField, Some("CrewroomForm > Input")),
            // Routes past the ticket list; "secondary" is Cmd on macOS and Ctrl elsewhere.
            gpui_kit::KeyBinding::new("secondary-1", FocusTickets, Some("CrewroomForm")),
            gpui_kit::KeyBinding::new("secondary-2", FocusSession, Some("CrewroomForm")),
            gpui_kit::KeyBinding::new("secondary-3", ShowNeedsYou, Some("CrewroomForm")),
        ]);
        let task = events.map(|events| {
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this
                        .update(cx, |this, cx| {
                            this.accept(event, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let descriptor = client.as_ref().map(Client::descriptor_path);
        let ticker = client.is_some().then(|| Self::overview_ticker(cx));
        let navigation = navigation::Navigation::new(descriptor.as_deref());
        let saved = navigation.saved().clone();
        let tab = match saved.tab.as_str() {
            "History" => "History",
            "Results" => "Results",
            "Conversation" => "Conversation",
            _ => "Brief",
        };
        let mut selected_runs = HashMap::new();
        if let (Some(task), Some(run)) = (&saved.task, &saved.run) {
            selected_runs.insert(task.clone(), run.clone());
        }
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| {
            if !window.has_active_dialog(cx) && !window.has_active_sheet(cx) {
                let target = window
                    .focus_lost_restore_target(cx)
                    .unwrap_or_else(|| this.focus.clone());
                window.focus(&target, cx);
            }
        });
        Self {
            _focus_lost: focus_lost,
            focus,
            tickets_focus: cx.focus_handle(),
            session_focus: cx.focus_handle(),
            details_scroll: Default::default(),
            client,
            workspace: json!({}),
            connected: false,
            busy: false,
            pending_commands: Default::default(),
            notice: String::new(),
            error: String::new(),
            feedback_origin: None,
            command_origins: HashMap::new(),
            feedback_placed: Default::default(),
            connection_error: String::new(),
            selected: saved.task,
            selected_runs,
            navigation,
            observation: observation::Observation::default(),
            panel: Panel::Ticket,
            navigation_open: false,
            tab,
            drafts: HashMap::new(),
            steering: HashMap::new(),
            title: cx.new(|cx| InputState::new(window, cx).placeholder("What needs to change?")),
            description: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Describe the request and useful context.")
            }),
            import: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("{\"siteUrl\":\"https://example.atlassian.net\",\"issues\":[…]}")
            }),
            credential_inputs: credentials::CredentialInputs::new(window, cx),
            issue_inputs: HashMap::new(),
            jira_inputs: HashMap::new(),
            source_inputs: HashMap::new(),
            source_open: Default::default(),
            settings: settings::Settings::default(),
            comparisons: comparisons::Comparisons::default(),
            planning: planning::Planning::default(),
            collaboration: collaboration::Collaboration::default(),
            agents: agents::Agents::default(),
            pending_keys: HashMap::new(),
            reset_new: false,
            reset_import: false,
            new_project: None,
            shelf: Default::default(),
            _events: task,
            _ticker: ticker,
        }
    }
    /// The workspace starts as an empty object; every service snapshot has fields.
    fn workspace_loaded(&self) -> bool {
        self.workspace.as_object().is_none_or(|w| !w.is_empty())
    }
    fn accept(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Superseded { path } => {
                self.pending_commands.remove(&path);
                self.pending_keys.remove(&path);
                self.command_origins.remove(&path);
                self.busy = !self.pending_commands.is_empty();
            }
            Event::Workspace(value) => {
                let tasks = records(&value, "tasks");
                self.drafts.retain(|id, draft| {
                    draft.edited(cx)
                        || tasks.iter().any(|t| {
                            t["id"] == *id
                                && t["revision"].as_u64() == Some(draft.revision)
                                && t["sourceRevision"].as_u64() == Some(draft.source_revision)
                        })
                });
                if self.workspace["runner"] != value["runner"] {
                    self.settings.runner.stale = true;
                }
                self.workspace = value;
                self.refresh_comparison();
                self.wake_snoozed(cx);
            }
            Event::Connected(value) => {
                self.connected = value;
                if !value {
                    self.observation.disconnected();
                }
                if value {
                    self.connection_error.clear();
                    // A runner reload restarts the service; re-read its view.
                    self.settings.runner.stale = true;
                }
            }
            Event::Live {
                generation,
                message,
            } => self.observation.accept(generation, message),
            Event::Read {
                generation,
                path,
                result,
            } => {
                if path.starts_with(settings::repository_folder::INSPECT) {
                    self.settings.inspected(&path, result);
                } else if path == settings::runner::READ {
                    self.settings.runner.read(result);
                } else if path == "/comparisons" || path.starts_with("/comparisons/") {
                    self.comparison_read(&path, result);
                } else if generation == self.observation.generation && path.ends_with("/feedback") {
                    self.observation.feedback_pending = false;
                    self.observation.feedback_error = result.err().unwrap_or_default();
                } else if generation == self.observation.generation
                    && self
                        .observation
                        .detail
                        .as_ref()
                        .is_some_and(|d| d.path == path)
                {
                    let detail = self.observation.detail.as_mut().unwrap();
                    detail.result = Some(result);
                    detail.content = None;
                } else if generation == self.observation.generation
                    && self.observation.archive_path.as_deref() == Some(&path)
                {
                    self.observation.archive_path = None;
                    let after = self.observation.archive_pending_start.take();
                    match result {
                        Ok(value) => {
                            if let Some(after) = after {
                                let starts = &mut self.observation.archive_starts;
                                if let Some(index) = starts.iter().position(|v| *v == after) {
                                    starts.truncate(index + 1);
                                } else {
                                    starts.push(after);
                                }
                            }
                            self.observation.archive = Some(value);
                            self.observation.archive_error.clear();
                        }
                        Err(error) => self.observation.archive_error = error,
                    }
                }
            }
            Event::Error(message) => self.connection_error = message,
            Event::Result { path, result } => {
                self.pending_commands.remove(&path);
                if let Some(origin) = self.command_origins.remove(&path) {
                    self.feedback_origin = Some(origin);
                }
                self.busy = !self.pending_commands.is_empty();
                match result {
                    Ok(value) => {
                        self.settings.saved(&path, &value);
                        if path.ends_with("/check-environment")
                            && let Some((_, payload)) = self.pending_keys.get(&path)
                            && let Some(id) = payload["checkId"].as_str()
                        {
                            self.collaboration.forms.remove(&format!("{path}/{id}"));
                        }
                        if path.ends_with("/target-refresh") {
                            self.collaboration
                                .forms
                                .retain(|key, _| !key.starts_with(&format!("{path}/")));
                        }
                        self.collaboration.saved(&path);
                        self.pending_keys.remove(&path);
                        self.notice = if path == "/role-preview" {
                            "Preview ready. No assignment was created."
                        } else if path == "/model-catalog/refresh" {
                            "Model refresh requested. Check each harness status below."
                        } else if path.ends_with("/publication/reconcile") {
                            "Publication reconciliation finished. Check the push and PR receipts."
                        } else if path == "/jira/reconcile" {
                            "Jira reconciliation finished. Only confirmed updates have receipts."
                        } else if path.starts_with("/principles/") && path.ends_with("/delete") {
                            "Principle deleted. Existing assignments keep their copied text."
                        } else {
                            "Saved on this machine."
                        }
                        .into();
                        self.error.clear();
                        if path == settings::overrides::SCOPE_PATH {
                            self.workspace["projectSettings"] = value.clone();
                            if let Some(body) = self.settings.scope_pending.take() {
                                self.scope_saved(&body);
                            }
                            self.notice = "Settings saved. New assignments use them; existing runs keep the values they captured.".into();
                        }
                        if path.starts_with("/runner/") {
                            self.settings.runner.saved();
                            if let Some(text) = value["notice"].as_str() {
                                self.notice = text.into();
                            }
                        }
                        if let Some(text) = credentials::saved_notice(&path) {
                            self.notice = text.into();
                            if let Some(facts) = value.get("credentials") {
                                self.workspace["credentials"] = facts.clone();
                            }
                        }
                        if let Some(text) = github_issues::notice(&path, &value) {
                            self.notice = text;
                            if let Some(facts) = value.get("githubIssues") {
                                self.workspace["githubIssues"] = facts.clone();
                            }
                        }
                        if let Some(text) = jira_refresh::notice(&path, &value) {
                            self.notice = text;
                            if let Some(facts) = value.get("jiraRefresh") {
                                self.workspace["jiraRefresh"] = facts.clone();
                            }
                        }
                        if let Some(text) = project_source::notice(&path) {
                            self.notice = text;
                            // Collapse the editor and re-prefill from the saved source.
                            if let Some(id) = value["repository"]["id"].as_str() {
                                self.source_open.remove(id);
                                self.source_inputs.remove(id);
                                // A new source brings its own filters.
                                self.issue_inputs.remove(id);
                                self.jira_inputs.remove(id);
                            }
                        }
                        if let Some(text) = credentials::github_notice(&path, &value) {
                            self.notice = text;
                            for key in ["credentials", "github"] {
                                if let Some(facts) = value.get(key) {
                                    self.workspace[key] = facts.clone();
                                }
                            }
                        }
                        if path.starts_with("/implementation-plans/") && path.ends_with("/decision")
                        {
                            self.notice = self.plan_decision_notice(&value);
                        }
                        if path == "/tasks" {
                            self.selected = value["id"].as_str().map(str::to_owned);
                            self.panel = Panel::Ticket;
                            self.reset_new = true;
                            self.feedback_origin = Some(self.origin_at(feedback::Place::Top));
                        }
                        if path.ends_with("/refinement") {
                            if let Some(tasks) = self.workspace["tasks"].as_array_mut()
                                && let Some(task) =
                                    tasks.iter_mut().find(|t| t["id"] == value["id"])
                            {
                                *task = value.clone();
                            }
                            if let Some(id) = path.split('/').nth(2) {
                                self.drafts.remove(id);
                            }
                        }
                        if path.ends_with("/steering") {
                            if let Some(id) = path.split('/').nth(2) {
                                self.steering.remove(id);
                            }
                            self.notice =
                                "Instruction stored. Delivery state is shown on the message."
                                    .into();
                        }
                        if path == "/queue/order" {
                            self.notice = "Queue order saved. Jira order is unchanged.".into();
                        }
                        if path.starts_with("/queue/") && path.ends_with("/remove") {
                            self.notice = "Queued plan cancelled. History is retained.".into();
                        }
                        if path.ends_with("/launch") {
                            self.notice = "Launch details saved. The plan waits for an available coding slot.".into();
                        }
                        if path.ends_with("/dependencies") {
                            self.notice =
                                "Dependencies saved. Running work keeps its captured requirements."
                                    .into();
                        }
                        if path.ends_with("/runs") {
                            // Without an accepted plan, Queue starts a coordinator scoping run.
                            self.notice = if value["purpose"] == "grill" {
                                "Scoping requested. A coordinator will draft a plan for you to review; coding waits for your acceptance."
                            } else {
                                "Plan queued. Execution waits for a saved launch team and an available coding slot."
                            }
                            .into();
                        }
                        if ["/runs", "/research-runs", "/refinement-runs"]
                            .iter()
                            .any(|suffix| path.ends_with(suffix))
                            && value["taskId"].is_string()
                            && value["id"].is_string()
                        {
                            self.selected_runs
                                .insert(string(&value, "taskId"), string(&value, "id"));
                            if path.ends_with("/research-runs") {
                                self.notice =
                                    "Research queued. Start its coordinator when ready.".into();
                            }
                            if path.ends_with("/refinement-runs") {
                                self.notice =
                                    "Refinement run saved. Check current agent activity.".into();
                            }
                        }
                        if path.ends_with("/adopt-requirements") {
                            self.notice =
                                "Requirements adopted. Check the run state before resuming.".into();
                        }
                        if path.ends_with("/continue-later")
                            && value["id"].is_string()
                            && value["taskId"].is_string()
                        {
                            self.selected_runs
                                .insert(string(&value, "taskId"), string(&value, "id"));
                            self.notice =
                                "Linked successor saved. Check its queue and execution state."
                                    .into();
                        }
                        for (suffix, notice) in [
                            (
                                "/pause",
                                "Pause request finished. Check confirmed process state.",
                            ),
                            (
                                "/resume",
                                "Resume submitted. Check whether it is queued or executing.",
                            ),
                            (
                                "/cancel",
                                "Cancellation requested. Changes and history are retained.",
                            ),
                            (
                                "/budget",
                                "Total time limit saved. Resume remains an explicit action.",
                            ),
                            (
                                "/repair-allowance",
                                "One additional automatic repair cycle allowed.",
                            ),
                            (
                                "/target-refresh",
                                "Target request finished. Inspect its recorded outcome.",
                            ),
                            (
                                "/check-environment",
                                "Check environment corrected. Earlier evidence may be stale.",
                            ),
                        ] {
                            if path.ends_with(suffix) {
                                self.notice = notice.into();
                            }
                        }
                        if path.ends_with("/specialists") {
                            self.notice =
                                "Specialist proposal saved. Admission is shown separately.".into();
                        }
                        if path == "/import/jira" {
                            self.selected = value[0]["id"].as_str().map(str::to_owned);
                            self.panel = Panel::Ticket;
                            self.reset_import = true;
                            self.feedback_origin = Some(self.origin_at(feedback::Place::Top));
                        }
                        if path.starts_with("/repositories/") && path.ends_with("/retire") {
                            self.notice = "Project retired. Saved tickets, history and worktrees are retained.".into();
                        }
                        if path == "/repositories" {
                            self.notice =
                                "Project saved. Existing runs keep their captured configuration."
                                    .into();
                        }
                        if path == settings::repository_folder::INITIALIZE {
                            self.notice = "Repository initialized with one empty commit. Existing files were not staged or committed. Review the details, then save.".into();
                        }
                        self.comparison_result(&path, &value, cx);
                    }
                    Err(error) => {
                        self.error = if path.starts_with("/comparisons/")
                            && path.ends_with("/adoption")
                        {
                            format!(
                                "Instruction saved; comparison association pending: {error}. Reopen the comparison to reconcile the saved revision."
                            )
                        } else {
                            error
                        };
                        self.notice.clear();
                    }
                }
            }
        }
        cx.notify();
    }
    fn send(&mut self, path: String, mut body: Value, keyed: bool, cx: &mut Context<Self>) {
        if (self.busy && !crate::client::stop_command(&path))
            || !self.connected
            || self.pending_commands.contains(&path)
        {
            return;
        }
        if let Some(reason) = self.scope_refusal(&path, &body) {
            self.error = reason;
            self.notice.clear();
            self.feedback_origin = Some(self.origin(&path));
            cx.notify();
            return;
        }
        if keyed {
            let key = self
                .pending_keys
                .get(&path)
                .filter(|(_, payload)| *payload == body)
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            self.pending_keys
                .insert(path.clone(), (key.clone(), body.clone()));
            body["requestId"] = json!(key);
        }
        match self
            .client
            .as_ref()
            .ok_or_else(|| "No service connection.".to_owned())
            .and_then(|c| c.send(path.clone(), body.clone()))
        {
            Ok(()) => self.started(path, &body),
            Err(error) => {
                self.error = error;
                self.notice.clear();
                self.feedback_origin = Some(self.origin(&path));
            }
        }
        cx.notify();
    }
    /// Records a command the service accepted for delivery.
    fn started(&mut self, path: String, body: &Value) {
        let origin = self.origin(&path);
        self.notice = feedback::pending(&path, body);
        self.error.clear();
        self.feedback_origin = Some(origin.clone());
        self.command_origins.insert(path.clone(), origin);
        self.pending_commands.insert(path);
        self.busy = true;
    }
    /// FOR-79: Settings navigation, the scope's overrides for this page, and
    /// the page's own content, read-only when it is workspace-only and a
    /// project is selected. The Projects page edits the project itself, so
    /// only its workspace ceiling is read-only there.
    fn settings_page(&mut self, content: Div, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let nav = self.settings_nav(cx);
        let scoped = self.scope_section(window, cx);
        let project = self.settings_scope().is_some();
        let content = if self.panel == Panel::Repositories {
            content
        } else {
            self.read_only_workspace(content)
        };
        // A project's overrides lead its pages; with All projects the
        // workspace editors come first and the override summary follows.
        let page = div().flex().flex_col().gap_4().child(nav);
        if project {
            page.children(scoped).child(content)
        } else {
            page.child(content).children(scoped)
        }
    }
    fn blocked(&self) -> bool {
        self.busy || !self.connected
    }
    pub fn unsaved(&self, cx: &App) -> bool {
        self.busy
            || (!self.reset_import && !self.import.read(cx).value().is_empty())
            || self.settings.unsaved(cx)
            || self.collaboration.unsaved(cx)
            || (!self.reset_new
                && (!self.title.read(cx).value().is_empty()
                    || !self.description.read(cx).value().is_empty()))
            || self
                .steering
                .values()
                .any(|s| !s.read(cx).value().trim().is_empty())
            || self.drafts.values().any(|draft| draft.edited(cx))
    }
    pub fn request_close(&self, window: &mut Window, cx: &mut App) -> bool {
        if !self.unsaved(cx) {
            cx.quit();
            return true;
        }
        window.open_alert_dialog(cx,|dialog,_,_|dialog.title(crate::accessibility::text("Leave with unsaved work?"))
            .description(crate::accessibility::text("Unsaved drafts will be discarded. A save already sent to the service may still complete. Queued and running work stays with the service."))
            .show_cancel(true).cancel_text("Keep editing").ok_text("Discard drafts and close")
            .on_ok(|_,_,cx|{cx.quit();true}));
        false
    }
    fn draft(&mut self, task: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let id = string(task, "id");
        self.drafts.entry(id).or_insert_with(|| Draft {
            brief: cx.new(|cx| TextareaState::new(window, cx).default_value(string(task, "brief"))),
            criteria: cx.new(|cx| {
                TextareaState::new(window, cx).default_value(
                    records(task, "criteria")
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }),
            revision: task["revision"].as_u64().unwrap_or(1),
            source_revision: task["sourceRevision"].as_u64().unwrap_or(1),
            original_brief: string(task, "brief"),
            original_criteria: records(task, "criteria")
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    /// Opens Settings → Projects on a new project, which starts from Choose folder (FOR-70).
    pub(super) fn add_project(&mut self, cx: &mut Context<Self>) {
        self.settings.repository = settings::NEW.into();
        self.panel = Panel::Repositories;
        self.navigation_open = false;
        cx.notify();
    }
    fn new_ticket(&self, cx: &mut Context<Self>) -> Div {
        let projects = self.ordered_projects();
        let target = self
            .new_project
            .as_ref()
            .and_then(|id| projects.iter().find(|p| p["id"] == *id));
        let mut choices = div().flex().flex_wrap().gap_2();
        for project in &projects {
            let id = string(project, "id");
            let selected = target.is_some_and(|t| t["id"] == id);
            choices = choices.child(
                Button::new(SharedString::from(format!("new-ticket-project-{id}")))
                    .label(string(project, "name"))
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_project = Some(id.clone());
                        cx.notify();
                    })),
            );
        }
        choices = choices.child(
            Button::new("new-ticket-project-none")
                .label("No project")
                .selected(target.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.new_project = None;
                    cx.notify();
                })),
        );
        section("New local ticket").child(hint("Capture the request, then refine its brief and acceptance criteria."))
            .child(crate::accessibility::text("Project"))
            .child(choices)
            .child(named_hint("new-ticket-project", match target {
                Some(p) => format!("Creates the ticket in {}.", string(p, "name")),
                None => "Creates the ticket under Unassigned. Choose a project before starting it.".into(),
            }))
            .child(crate::accessibility::text("Title")).child(Input::new(&self.title).id("ticket-title").disabled(self.blocked()))
            .child(crate::accessibility::text("Description")).child(Textarea::new(&self.description).accessibility_id("ticket-description").aria_label("Description").h(px(180.)).disabled(self.blocked()))
            .child(Button::new("create-ticket").primary().label("Create ticket").disabled(self.blocked()).on_click(cx.listener(|this,_,_,cx| {
                let mut body = json!({"title":this.title.read(cx).value().to_string(),"description":this.description.read(cx).value().to_string()});
                if let Some(id) = this.new_project.clone().filter(|id| this.ordered_projects().iter().any(|p| p["id"] == *id)) {
                    body["repositoryId"] = json!(id);
                }
                this.send("/tasks".into(),body,false,cx);
            })))
    }
    fn import_panel(&self, cx: &mut Context<Self>) -> Div {
        let unreadable = self.workspace["credentials"]["jira"]["source"] == "unreadable";
        let jira = section("Jira")
            .child(hint(if unreadable {
                "Jira: Credentials unreadable — reconnect under Credentials below."
            } else {
                ""
            })).child(hint(if self.workspace["capabilities"]["liveJira"]==true {"Each project whose ticket source is a Jira site refreshes with its own filters under Settings → Projects. Imported fields remain separate from your accepted brief."}else{"Live Jira is not configured. You can import a snapshot and retain source fields separately."}))
            .child(self.jira_refresh_section())
            .child(Button::new("refresh-jira").label("Refresh all Jira projects").disabled(self.blocked()||self.workspace["capabilities"]["liveJira"]!=true).on_click(cx.listener(|this,_,_,cx|this.send("/jira/sync".into(),json!({}),false,cx))))
            .child(crate::accessibility::text("Import a Jira snapshot"))
            .child(Textarea::new(&self.import).accessibility_id("jira-json").aria_label("Jira JSON").h(px(320.)).disabled(self.blocked()))
            .child(Button::new("save-import").primary().label("Import snapshot").disabled(self.blocked()).on_click(cx.listener(|this,_,_,cx| {
                match serde_json::from_str(&this.import.read(cx).value()) {Ok(body)=>this.send("/import/jira".into(),body,false,cx),Err(_)=>{this.fail_at("Enter valid JSON with siteUrl and issues.", feedback::Place::Top);cx.notify();}}
            })));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(jira)
            .child(self.github_section(cx))
            .child(self.github_issues_section(cx))
            .child(self.credentials_section(cx))
    }
    /// The session column and, when a details tab is open, the drawer beside it.
    fn ticket(
        &mut self,
        task: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Div, Option<Div>) {
        self.draft(&task, window, cx);
        let task_id = string(&task, "id");
        let runs: Vec<_> = records(&self.workspace, "runs")
            .into_iter()
            .filter(|r| r["taskId"] == task_id)
            .collect();
        let run = self.current_run(&task_id);
        self.observe_run(run.as_ref().map(|r| string(r, "id")));
        let details_open = self.tab != "Conversation";
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(220.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(hint(format!(
                                "{}  /  {}",
                                string(&task, "key"),
                                ticket_source::ticket_label(&task)
                            )))
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(crate::accessibility::text(string(&task, "title"))),
                            ),
                    )
                    .child(
                        Button::new("toggle-details")
                            .label(if details_open {
                                "Hide details"
                            } else {
                                "Brief · Results · History"
                            })
                            .selected(details_open)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tab = if details_open {
                                    "Conversation"
                                } else {
                                    "Brief"
                                };
                                cx.notify();
                            })),
                    ),
            )
            .child(self.facts_bar(&task, run.as_ref()));
        if let Some(run) = &run {
            panel = panel.child(self.run_playbook(run, window, cx));
        }
        if runs.len() > 1 {
            let mut choices = div().flex().flex_wrap().gap_2();
            for item in runs.iter().rev() {
                let id = string(item, "id");
                let task = task_id.clone();
                choices = choices.child(
                    Button::new(SharedString::from(format!("select-run-{id}")))
                        .label(format!(
                            "{} · {} · {}",
                            item["purpose"].as_str().unwrap_or("coding"),
                            crewroom_core::lifecycle::run_label(item),
                            &id[..8.min(id.len())]
                        ))
                        .selected(run.as_ref().is_some_and(|r| r["id"] == id))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_runs.insert(task.clone(), id.clone());
                            cx.notify();
                        })),
                );
            }
            panel = panel.child(section("Select a run").child(choices));
        }
        if run.is_some()
            && (!self.observation.runtime_fresh || self.observation.errors.contains_key("run"))
        {
            panel = panel.child(hint(
                self.observation
                    .errors
                    .get("run")
                    .map(|error| format!("Observation unavailable: {error}. This does not establish an agent failure."))
                    .unwrap_or_else(|| {
                        "Loading run observation · retained activity may be stale".into()
                    }),
            ));
        }
        if let Some(run) = &run {
            panel = panel.child(self.run_controls(&task, run, window, cx));
        }
        if let (Some(run), Some(client)) = (&run, &self.client) {
            let run_id = string(run, "id");
            let descriptor = client.descriptor_path();
            let subject = crate::wall::Subject {
                key: string(&task, "key"),
                title: string(&task, "title"),
            };
            panel = panel.child(
                div().flex().child(
                    Button::new("open-agent-wall")
                        .ghost()
                        .label(crate::wall::OPEN_LABEL)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(error) = crate::wall::open(
                                descriptor.clone(),
                                run_id.clone(),
                                subject.clone(),
                                cx,
                            ) {
                                this.fail_at(error.to_string(), feedback::Place::Top);
                            }
                        })),
                ),
            );
        }
        // The session column shows the coordinator, its steering and the specialist slots.
        if let Some(run) = &run {
            panel = panel.child(self.agent_session(run, window, cx));
        } else {
            panel = panel.child(section("Coordinator conversation").child(hint(
                "Queue an accepted brief to save steering for its coordinator.",
            )));
        }
        let drawer = details_open.then(|| {
            let mut tabs = div()
                .flex()
                .flex_wrap()
                .gap_1()
                .items_center()
                .pb_2()
                .border_b_1()
                .border_color(rgb(0x293642));
            for tab in ["Brief", "Results", "History"] {
                tabs = tabs.child(
                    Button::new(tab)
                        .ghost()
                        .selected(self.tab == tab)
                        .label(tab)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tab = tab;
                            cx.notify();
                        })),
                );
            }
            tabs.child(div().flex_1()).child(
                Button::new("hide-details")
                    .ghost()
                    .label("Back to session")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tab = "Conversation";
                        cx.notify();
                    })),
            )
        });
        let Some(tabs) = drawer else {
            self.save_navigation();
            return (panel, None);
        };
        let mut drawer = div().flex().flex_col().gap_4().child(tabs);
        if self.tab == "Brief" {
            let draft = self.drafts.get(&task_id).unwrap();
            let mut brief = section("Accepted brief").child(hint(format!(
                "Revision {} · source revision {}",
                draft.revision, draft.source_revision
            )));
            if task["needsReacceptance"] == true {
                let mut lines = source_changes(&task).into_iter();
                brief = brief
                    .child(
                        div()
                            .text_color(rgb(0xf1bf73))
                            .children(lines.next().map(crate::accessibility::text)),
                    )
                    .children(
                        lines
                            .enumerate()
                            .map(|(i, line)| div().id(("source-change", i)).child(hint(line))),
                    );
            }
            if draft.edited(cx)
                || draft.revision != task["revision"].as_u64().unwrap_or(1)
                || draft.source_revision != task["sourceRevision"].as_u64().unwrap_or(1)
            {
                let reload = task_id.clone();
                brief = brief.child(hint("Your draft is retained. Reload the saved brief to discard edits or resolve a changed revision."))
                    .child(Button::new("reload-brief").label("Load current brief").on_click(cx.listener(move |this,_,window,cx|{
                        if !this.drafts.get(&reload).is_some_and(|d|d.edited(cx)) {this.drafts.remove(&reload);cx.notify();return;}
                        let view=cx.entity().downgrade();let id=reload.clone();
                        window.open_alert_dialog(cx,move|dialog,_,_|{let view=view.clone();let id=id.clone();dialog.title("Discard this brief draft?").description("Reload the latest accepted brief and criteria. Unsaved edits to this brief will be discarded.").show_cancel(true).cancel_text("Keep editing").ok_text("Discard and reload").on_ok(move|_,_,cx|{let _=view.update(cx,|this,cx|{this.drafts.remove(&id);cx.notify();});true})});
                    })));
            }
            brief = brief
                .child(hint(format!(
                    "Source description: {}",
                    string(&task, "description")
                )))
                .child(crate::accessibility::text("Brief"))
                .child(
                    Textarea::new(&draft.brief)
                        .accessibility_id("brief-input")
                        .aria_label("Brief")
                        .h(px(140.))
                        .disabled(self.busy),
                )
                .child(crate::accessibility::text(
                    "Acceptance criteria · one per line",
                ))
                .child(
                    Textarea::new(&draft.criteria)
                        .accessibility_id("criteria-input")
                        .aria_label("Acceptance criteria")
                        .h(px(140.))
                        .disabled(self.busy),
                );
            if let Some(version) = self.accepted_plan_version(&task_id)
                && self.brief_dirty(&task, cx)
            {
                brief = brief.child(hint(format!("Saving changed requirements sets accepted plan {version} aside. Queue will start scoping again unless you accept a new plan version.")));
            }
            let save = task_id.clone();
            brief = brief.child(Button::new("save-brief").primary().label("Save brief").disabled(self.blocked()).on_click(cx.listener(move |this,_,_,cx| {
                let draft = this.drafts.get(&save).unwrap();
                let body = json!({"revision":draft.revision,"sourceRevision":draft.source_revision,"brief":draft.brief.read(cx).value().to_string(),"criteria":draft.criteria.read(cx).value().lines().map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>()});
                this.send(format!("/tasks/{save}/refinement"),body,false,cx);
            })));
            drawer = drawer.child(brief);
            drawer = drawer
                .child(self.intake_controls(&task, window, cx))
                .child(self.plan_controls(&task, &runs, cx))
                .child(self.dependencies(&task, cx))
                .child(self.refinement_controls(&task, run.as_ref(), window, cx))
                .child(self.refinement_proposals(&task, window, cx));
            if let Some(run) = &run {
                drawer = drawer.child(self.adoption(&task, run, window, cx));
            }
        } else if self.tab == "Results" {
            if let Some(run) = &run {
                drawer = drawer.child(self.results_panel(run, window, cx));
            } else {
                drawer = drawer.child(hint("Queue a run to inspect its saved results."));
            }
        } else {
            drawer = drawer.child(self.history_panel(&task, &runs, run.as_ref(), window, cx));
        }
        self.save_navigation();
        (panel, Some(drawer))
    }
}
impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        if self.reset_new {
            self.title
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.description
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.reset_new = false;
        }
        if self.reset_import {
            self.import
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.reset_import = false;
        }
        self.apply_settings_saved(window, cx);
        if self
            .comparisons
            .return_panel
            .is_some_and(|panel| panel != self.panel)
        {
            self.comparisons.open = false;
        }
        let narrow = window.viewport_size().width < px(900.);
        if narrow && self.navigation_open {
            // The overlay keeps the root's focus tracking and Tab bindings so it stays keyboard reachable.
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(0x111a22))
                .text_color(rgb(0xe2e8ed))
                .track_focus(&self.focus)
                .key_context("CrewroomForm")
                .on_action(|_: &NextField, window, cx| window.focus_next(cx))
                .on_action(|_: &PreviousField, window, cx| window.focus_prev(cx))
                .on_action(
                    cx.listener(|this, _: &FocusTickets, window, cx| {
                        this.focus_tickets(window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &FocusSession, window, cx| {
                        this.focus_session(window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &ShowNeedsYou, window, cx| {
                        this.show_needs_you(window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &crate::Quit, window, cx| {
                    this.request_close(window, cx);
                }))
                .child(
                    Button::new("close-navigation")
                        .label("Back to work")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.navigation_open = false;
                            cx.notify();
                        })),
                )
                .child(self.sidebar(cx));
        }
        let mut main = div()
            .id("workspace")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_6();
        self.feedback_placed.set(false);
        let (content, drawer) = if self.comparisons.open {
            (self.comparison_panel(cx), None)
        } else {
            match self.panel {
                Panel::New => (self.new_ticket(cx), None),
                Panel::Import => {
                    let content = self.import_panel(cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Repositories => {
                    let content = self.repositories(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Playbooks => {
                    let content = self.playbooks(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Principles => {
                    let content = self.principles(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Models => {
                    let content = self.models(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Runner => {
                    let content = self.runner_panel(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Roles => {
                    let content = self.roles(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Queue => (self.queue_panel(cx), None),
                Panel::NeedsYou => (self.needs_panel(window, cx), None),
                Panel::Overview => (self.overview_panel(cx), None),
                Panel::Learning => {
                    let content = self.learning_panel(window, cx);
                    (self.settings_page(content, window, cx), None)
                }
                Panel::Ticket => {
                    let task = records(&self.workspace, "tasks")
                        .into_iter()
                        .find(|t| t["id"].as_str() == self.selected.as_deref());
                    if let Some(task) = task {
                        self.ticket(task, window, cx)
                    } else if self.workspace_loaded() && self.ordered_projects().is_empty() {
                        (self.empty_workspace(cx), None)
                    } else {
                        // With no ticket chosen, the home screen is the Overview (FOR-77).
                        (self.overview_panel(cx), None)
                    }
                }
            }
        };
        // Wide windows keep the session visible beside its details; narrow windows show one at a time.
        let wide = window.viewport_size().width >= px(1100.);
        let drawer = match drawer {
            Some(drawer) if wide => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .children(self.feedback(feedback::Place::Drawer))
                    .child(drawer),
            ),
            drawer => drawer,
        };
        if !self.connection_error.is_empty() {
            main = main.child(crate::accessibility::observable(
                div()
                    .id("error-connection")
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0x462c2e))
                    .text_color(rgb(0xffc4b8))
                    .child(crate::accessibility::text(self.connection_error.clone())),
            ));
        }
        main = main.children(self.top_feedback());
        let session = &self.session_focus;
        let body = match drawer {
            Some(drawer) if wide => div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(crate::accessibility::focus_region(
                    main.child(content),
                    session,
                ))
                .child(
                    div()
                        .id("details-drawer")
                        .w(px(440.))
                        .flex_shrink_0()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.details_scroll)
                        .p_5()
                        .bg(rgb(0x0e171e))
                        .border_l_1()
                        .border_color(rgb(0x293642))
                        .child(drawer),
                ),
            Some(drawer) => {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(crate::accessibility::focus_region(
                        main.track_scroll(&self.details_scroll).child(drawer),
                        session,
                    ))
            }
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(crate::accessibility::focus_region(
                    main.child(content),
                    session,
                )),
        };
        div()
            .size_full()
            .flex()
            .bg(rgb(0x111a22))
            .text_color(rgb(0xe2e8ed))
            .font_family(".SystemUIFont")
            .text_sm()
            .track_focus(&self.focus)
            .key_context("CrewroomForm")
            .on_action(|_: &NextField, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousField, window, cx| window.focus_prev(cx))
            .on_action(
                cx.listener(|this, _: &FocusTickets, window, cx| this.focus_tickets(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FocusSession, window, cx| this.focus_session(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowNeedsYou, window, cx| this.show_needs_you(window, cx)),
            )
            .on_action(cx.listener(|this, _: &crate::Quit, window, cx| {
                this.request_close(window, cx);
            }))
            .when(!narrow, |root| root.child(self.sidebar(cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when(narrow, |column| {
                        column.child(
                            div()
                                .min_h(px(52.))
                                .flex_shrink_0()
                                .px_6()
                                .flex()
                                .items_center()
                                .justify_between()
                                .flex_wrap()
                                .gap_2()
                                .border_b_1()
                                .border_color(rgb(0x293642))
                                .when(narrow, |header| {
                                    header
                                        .child(
                                            Button::new("open-navigation")
                                                .label("Tickets & settings")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.navigation_open = true;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(self.needs_entry(cx))
                                })
                                .child(
                                    div()
                                        .text_color(if self.connected {
                                            rgb(0x84d7bc)
                                        } else {
                                            rgb(0xf1bf73)
                                        })
                                        .child(crate::accessibility::readable(
                                            "service-connection-status",
                                            if self.connected {
                                                "Connected"
                                            } else {
                                                "Reconnecting · retained data may be stale"
                                            },
                                        )),
                                ),
                        )
                    })
                    .child(body)
                    .child(
                        div()
                            .min_h(px(32.))
                            .flex_shrink_0()
                            .px_4()
                            .py_2()
                            .flex()
                            .items_center()
                            .border_t_1()
                            .border_color(rgb(0x293642))
                            .child(
                                named_hint("connection-summary", self.connection_summary())
                                    .flex_1()
                                    .min_w_0(),
                            ),
                    ),
            )
    }
}

/// Re-acceptance copy naming which Jira fields changed since the brief was last
/// accepted, with previous and current text. The first line is the warning.
/// What changed in the imported source since the brief was accepted, named for its provider.
fn source_changes(task: &Value) -> Vec<String> {
    let name = ticket_source::source_name(task);
    let accepted = &task["acceptedSource"];
    if !accepted.is_object() {
        return vec![format!(
            "{name} requirements changed. The previously accepted {name} text was not retained; review the source description below and save the brief to accept it."
        )];
    }
    let fields: Vec<_> = [("title", "title"), ("description", "description")]
        .into_iter()
        .filter(|(key, _)| accepted[*key] != task[*key])
        .collect();
    let mut lines = vec![format!(
        "{name} requirements changed. Review what changed and save the brief to accept them."
    )];
    if fields.is_empty() {
        lines.push(format!(
            "{name} title and description now match the accepted text."
        ));
        return lines;
    }
    let names: Vec<_> = fields.iter().map(|(_, name)| *name).collect();
    lines.push(format!("Changed in {name}: {}.", names.join(" and ")));
    for (key, field) in fields {
        lines.push(format!("Previous {field}: {}", string(accepted, key)));
        lines.push(format!("Current {field}: {}", string(task, key)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::test::TestWindowExt;
    use gpui::{Bounds, WindowBounds, WindowOptions, point, size};
    use gpui_kit as gpui;

    #[gpui::test]
    fn native_navigation_preserves_draft_and_stale_push_does_not_replace_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (handle,view) = cx.update(|cx| {
            gpui::init(cx);
            cx.bind_keys([gpui::KeyBinding::new(crate::QUIT_SHORTCUT, crate::Quit, None)]);
            gpui::open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds{origin:point(px(0.),px(0.)),size:size(px(1280.),px(1000.))})),..Default::default()},cx,|window,cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None,None,window,cx);
                    view.connected = true;view.selected = Some("fixture".into());
                    view.workspace = json!({"tasks":[{"id":"fixture","key":"SIM-1","source":"local","title":"Native fixture","brief":"","criteria":[],"revision":1,"sourceRevision":1}],"runs":[],"roles":[],"repositories":[]});
                    view
                })
            }).unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let input = view.read(cx).drafts["fixture"].brief.clone();
            input.update(cx, |input, cx| input.focus(window, cx));
            window.input("Retained 日本語 🦀", cx);
            assert_eq!(input.read(cx).value(), "Retained 日本語 🦀");
            assert!(view.read(cx).unsaved(cx));
            window.click("settings", cx);
            window.render_frame(cx);
            window.click("roles", cx);
            window.render_frame(cx);
            assert!(
                view.read(cx).focus.contains_focused(window, cx),
                "Hidden retained input must restore focus to a rendered ancestor."
            );
            window.press("tab", cx);
            window.press(crate::QUIT_SHORTCUT, cx);
            window.render_frame(cx);
            assert!(
                window.try_find("ok").is_some(),
                "Quit must reach the draft dialog after the input disappears."
            );
            // Dialog entry uses wall time, not GPUI's deterministic test clock.
            // Resolve the final button bounds before dispatching the pointer.
            std::thread::sleep(std::time::Duration::from_millis(300));
            window.render_frame(cx);
            window.click("cancel", cx);
            assert!(!window.has_active_dialog(cx));
            // The alert overlay disappears on the next frame, before navigation.
            window.render_frame(cx);
            window.click(SharedString::from("ticket-fixture"), cx);
            window.render_frame(cx);
            assert!(matches!(view.read(cx).panel, Panel::Ticket));
            assert_eq!(
                view.read(cx).drafts["fixture"].brief.read(cx).value(),
                "Retained 日本語 🦀"
            );
            view.update(cx, |view, cx| {
                let mut refreshed = view.workspace.clone();
                refreshed["tasks"][0]["revision"] = json!(2);
                refreshed["tasks"][0]["brief"] = json!("A different accepted revision");
                view.accept(Event::Workspace(refreshed), cx);
            });
            window.render_frame(cx);
            assert_eq!(view.read(cx).drafts["fixture"].revision, 1);
            assert_eq!(
                view.read(cx).drafts["fixture"].brief.read(cx).value(),
                "Retained 日本語 🦀"
            );
            assert!(window.try_find("reload-brief").is_some());
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press(crate::QUIT_SHORTCUT, cx);
            window.render_frame(cx);
            assert!(window.try_find("ok").is_some());
            assert!(window.try_find("cancel").is_some());
            std::thread::sleep(std::time::Duration::from_millis(300));
            window.render_frame(cx);
            window.click("cancel", cx);
            assert!(!window.has_active_dialog(cx));
            assert_eq!(input.read(cx).value(), "Retained 日本語 🦀");
        })
        .unwrap();
    }
}
