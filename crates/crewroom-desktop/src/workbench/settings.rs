//! Operator-owned configuration drafts. Push updates never replace an edited form.
use super::forms::Fields;
use super::*;
use gpui_kit::component::checkbox::Checkbox;
mod learning;
mod model_picker;
pub(super) use model_picker::billing_label;
mod models;
mod playbooks;
mod principles;
pub(super) mod repository_folder;
mod roles;

const NEW: &str = "__new";
#[derive(Default)]
pub(super) struct Settings {
    role: String,
    learning: Option<(String, String)>,
    history_open: bool,
    draft_open: bool,
    draft_description: Option<Fields>,
    models: Option<models::ModelDraft>,
    pickers: HashMap<String, model_picker::Picker>,
    playbook: String,
    playbooks: HashMap<String, playbooks::PlaybookDraft>,
    principle: String,
    principles: HashMap<String, principles::PrincipleDraft>,
    role_open: bool,
    preview_task: String,
    preview: Option<Value>,
    preview_source: Option<(Value, String)>,
    repository: String,
    roles: HashMap<String, RoleDraft>,
    repositories: HashMap<String, RepositoryDraft>,
    submitted_role: String,
    saved: Option<(String, Value)>,
}
struct RoleDraft {
    fields: Fields,
    draft_proposal: Option<String>,
    effort: String,
    job: Value,
    principles: Vec<String>,
    enabled: bool,
    revision: Option<u64>,
    original: Value,
}
impl RoleDraft {
    fn new(value: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mut draft = Self {
            fields: Fields::new(
                value,
                &[
                    ("id", false),
                    ("name", false),
                    ("category", false),
                    ("purpose", true),
                    ("instructions", true),
                    ("model", false),
                ],
                window,
                cx,
            ),
            draft_proposal: value["draftProposalId"].as_str().map(str::to_owned),
            job: value["job"].clone(),
            principles: value["principles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            effort: value["effort"].as_str().unwrap_or("default").into(),
            enabled: value["enabled"].as_bool().unwrap_or(true),
            revision: value["revision"].as_u64(),
            original: Value::Null,
        };
        draft.original = draft.value(cx);
        draft
    }
    fn value(&self, cx: &App) -> Value {
        let mut v = self.fields.values(cx);
        if let Some(id) = &self.draft_proposal {
            v["draftProposalId"] = json!(id);
        }
        v["effort"] = json!(self.effort);
        v["job"] = self.job.clone();
        v["principles"] = json!(self.principles);
        v["enabled"] = json!(self.enabled);
        if let Some(revision) = self.revision {
            v["revision"] = json!(revision);
        }
        v
    }
    fn dirty(&self, cx: &App) -> bool {
        self.value(cx) != self.original
    }
}
struct CheckDraft {
    key: String,
    fields: Fields,
    required: bool,
    tmp_executable: bool,
}
impl CheckDraft {
    fn new(value: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mut value = value.clone();
        if value["timeoutMs"].is_null() {
            value["timeoutMs"] = json!(60000);
        }
        Self {
            key: uuid::Uuid::new_v4().to_string(),
            fields: Fields::new(
                &value,
                &[
                    ("id", false),
                    ("image", false),
                    ("command", false),
                    ("args", true),
                    ("timeoutMs", false),
                    ("memoryMiB", false),
                ],
                window,
                cx,
            ),
            required: value["required"].as_bool().unwrap_or(true),
            tmp_executable: value["tmpExecutable"].as_bool().unwrap_or(false),
        }
    }
    fn value(&self, cx: &App) -> Result<Value, String> {
        let mut v = self.fields.values(cx);
        v["args"] = json!(
            self.fields
                .text("args", cx)
                .lines()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
        for (key, label) in [("timeoutMs", "Timeout"), ("memoryMiB", "Memory")] {
            let text = self.fields.text(key, cx);
            if key == "memoryMiB" && text.trim().is_empty() {
                v.as_object_mut().unwrap().remove(key);
            } else {
                v[key] = json!(
                    text.trim()
                        .parse::<u64>()
                        .map_err(|_| format!("{label} must be a whole number."))?
                );
            }
        }
        v["required"] = json!(self.required);
        v["tmpExecutable"] = json!(self.tmp_executable);
        Ok(v)
    }
}
struct RepositoryDraft {
    fields: Fields,
    checks: Vec<CheckDraft>,
    revision: Option<u64>,
    original: Value,
    folder: repository_folder::Folder,
}
impl RepositoryDraft {
    fn new(value: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mut draft = Self {
            fields: Fields::new(
                value,
                &[
                    ("name", false),
                    ("path", false),
                    ("targetBranch", false),
                    ("branchPrefix", false),
                ],
                window,
                cx,
            ),
            checks: records(value, "checks")
                .iter()
                .map(|v| CheckDraft::new(v, window, cx))
                .collect(),
            revision: value["revision"].as_u64(),
            original: Value::Null,
            folder: Default::default(),
        };
        draft.original = draft.value(cx).unwrap();
        draft
    }
    fn value(&self, cx: &App) -> Result<Value, String> {
        let mut v = self.fields.values(cx);
        if self.fields.text("branchPrefix", cx).trim().is_empty() {
            v.as_object_mut().unwrap().remove("branchPrefix");
        }
        v["checks"] = Value::Array(
            self.checks
                .iter()
                .enumerate()
                .map(|(index, c)| {
                    // Match the service's prefix: position, then the ID when one is entered.
                    c.value(cx)
                        .map_err(|error| match c.fields.text("id", cx).trim() {
                            "" => format!("Check {}: {error}", index + 1),
                            id => format!("Check {} ({id}): {error}", index + 1),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        if let Some(revision) = self.revision {
            v["revision"] = json!(revision);
        }
        Ok(v)
    }
    fn dirty(&self, cx: &App) -> bool {
        !self.value(cx).is_ok_and(|v| v == self.original)
    }
}
impl Settings {
    pub(super) fn comparison_draft(&self, kind: &str, id: &str, cx: &App) -> Option<Value> {
        match kind {
            "role" => self.roles.get(id).map(|draft| draft.value(cx)),
            "principle" => self.principles.get(id).map(|draft| draft.value(cx)),
            "playbook" => self.playbooks.get(id).map(|draft| draft.value(cx)),
            _ => None,
        }
    }
    pub(super) fn unsaved(&self, cx: &App) -> bool {
        self.playbooks.values().any(|d| d.dirty(cx))
            || self
                .draft_description
                .as_ref()
                .is_some_and(|d| !d.text("description", cx).trim().is_empty())
            || self.principles.values().any(|d| d.dirty(cx))
            || self.roles.values().any(|d| d.dirty(cx))
            || self.repositories.values().any(|d| d.dirty(cx))
            || self.models.as_ref().is_some_and(|d| d.dirty(cx))
    }
    pub(super) fn saved(&mut self, path: &str, value: &Value) {
        if path == "/learning" || path == "/role-drafts" {
            self.saved = Some((path.into(), value.clone()));
            return;
        }
        if path == "/playbooks" {
            self.saved = Some(("playbook".into(), value.clone()));
            return;
        }
        if path == "/principles" {
            self.saved = Some(("principle".into(), value.clone()));
            return;
        }
        // A deleted principle has nothing left to save: drop its draft and close the editor.
        if let Some(id) = path
            .strip_prefix("/principles/")
            .and_then(|rest| rest.strip_suffix("/delete"))
        {
            self.principles.remove(id);
            if self.principle == id {
                self.principle.clear();
            }
            return;
        }
        if path == repository_folder::INITIALIZE {
            self.initialized(value);
            return;
        }
        if path == "/model-jobs" {
            self.saved = Some(("models".into(), value.clone()));
            return;
        }
        if path == "/role-preview" {
            self.preview = Some(value.clone());
            return;
        }
        if path == "/roles" || path.starts_with("/roles/") {
            if let Some(role) = value
                .as_array()
                .and_then(|roles| roles.iter().find(|r| r["id"] == self.submitted_role))
            {
                self.saved = Some(("role".into(), role.clone()));
            }
        } else if path == "/repositories"
            || (path.starts_with("/repositories/") && !path.ends_with("/retire"))
        {
            self.saved = Some(("repository".into(), value.clone()));
        }
    }
}
impl Workbench {
    pub(super) fn connection_summary(&self) -> String {
        if !self.workspace_loaded() {
            return "Waiting for the workflow service".into();
        }
        let execution = self.workspace["capabilities"]["execution"] == true;
        let jira = if self.workspace["capabilities"]["liveJira"] != true {
            "Jira not configured"
        } else if self.workspace["connections"]["jira"] == true {
            "Jira connected"
        } else {
            "Jira sync incomplete"
        };
        format!(
            "{jira}  ·  {}  ·  {}",
            if execution {
                "Runner configured"
            } else {
                "Runner not configured"
            },
            if self.connected {
                "Service connected"
            } else {
                "Retained state · reconnecting"
            }
        )
    }
    pub(super) fn apply_settings_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, value)) = self.settings.saved.take() else {
            return;
        };
        if kind == "/learning" {
            if let Some((role, mode)) = self.settings.learning.take() {
                self.collaboration
                    .forms
                    .remove(&format!("role-learning-{role}-{mode}"));
            }
            self.notice =
                "Proposal recorded with your evidence. Approve or reject it in Learning.".into();
            self.panel = Panel::Learning;
            self.feedback_origin = Some(self.origin_at(crate::workbench::feedback::Place::Top));
            return;
        }
        if kind == "/role-drafts" {
            self.notice="Drafting session recorded. Follow its status and review its proposal in Agent roles.".into();
            self.settings.draft_description = None;
            return;
        }
        if kind == "playbook" {
            self.saved_playbook(&value, window, cx);
            return;
        }
        if kind == "principle" {
            self.saved_principle(&value, window, cx);
            return;
        }
        if kind == "models" {
            self.settings.models = Some(models::ModelDraft::new(&value, window, cx));
            self.workspace["modelJobs"] = value;
            self.notice = "Models and budget saved. Admitted assignments keep their captured model and effort.".into();
            return;
        }
        let id = string(&value, "id");
        if kind == "role" {
            self.settings.roles.remove(&self.settings.role);
            self.settings.role = id.clone();
            self.settings
                .roles
                .insert(id, RoleDraft::new(&value, window, cx));
            self.notice =
                "Role saved. Existing assignments retain their original definition.".into();
        } else {
            self.settings.repositories.remove(&self.settings.repository);
            self.settings.repository = id.clone();
            self.settings
                .repositories
                .insert(id, RepositoryDraft::new(&value, window, cx));
            self.notice =
                "Repository saved. Existing runs retain their captured configuration.".into();
        }
        let key = if kind == "role" {
            "roles"
        } else {
            "repositories"
        };
        if let Some(rows) = self.workspace[key].as_array_mut() {
            if let Some(existing) = rows.iter_mut().find(|r| r["id"] == value["id"]) {
                if existing["revision"].as_u64().unwrap_or(1)
                    <= value["revision"].as_u64().unwrap_or(1)
                {
                    *existing = value;
                }
            } else {
                rows.push(value);
            }
        }
    }
    fn reload_setting(
        &self,
        kind: &'static str,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        // A never-saved role has no saved revision; removing its draft lets the editor
        // recreate it from the creation defaults.
        let (title, description, ok) = if kind == "role" && key.starts_with(NEW) {
            (
                "Discard this new role?",
                "Reset the form to the new-role defaults. Your unsaved edits will be discarded.",
                "Discard new role",
            )
        } else {
            (
                "Discard this draft?",
                "Reload the latest saved revision. Your unsaved edits to this form will be discarded.",
                "Discard and reload",
            )
        };
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let key = key.clone();
            dialog
                .title(crate::accessibility::text(title))
                .description(crate::accessibility::text(description))
                .show_cancel(true)
                .cancel_text("Keep editing")
                .ok_text(ok)
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if kind == "playbook" {
                            this.settings.playbooks.remove(&key);
                        } else if kind == "principle" {
                            this.settings.principles.remove(&key);
                        } else if kind == "models" {
                            this.settings.models = None;
                        } else if kind == "role" {
                            this.settings.roles.remove(&key);
                        } else {
                            this.settings.repositories.remove(&key);
                        }
                        cx.notify();
                    });
                    true
                })
        });
    }
    /// Never-saved drafts live under keys no catalog card opens, so each catalog lists them.
    fn unsaved_drafts(
        &self,
        kind: &'static str,
        mut drafts: Vec<(String, String)>,
        open: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        drafts.sort();
        let mut list = div().flex().flex_col().gap_2();
        for (name, key) in drafts {
            let name = if name.trim().is_empty() {
                "Untitled".to_owned()
            } else {
                name
            };
            let (target, discard) = (key.clone(), key.clone());
            list = list.child(
                div()
                    .id(SharedString::from(format!("{kind}-draft-{key}")))
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x293642))
                    .rounded_md()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(crate::accessibility::text(format!(
                        "Unsaved draft · {name}"
                    )))
                    .child(
                        Button::new(SharedString::from(format!("open-{kind}-draft-{key}")))
                            .label("Open")
                            .accessibility_label(format!("Open unsaved draft {name}"))
                            .selected(key == open)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match kind {
                                    "role" => {
                                        this.settings.role = target.clone();
                                        this.settings.role_open = true;
                                    }
                                    "principle" => this.settings.principle = target.clone(),
                                    _ => this.settings.playbook = target.clone(),
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("discard-{kind}-draft-{key}")))
                            .label("Discard")
                            .accessibility_label(format!("Discard unsaved draft {name}"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.discard_new_draft(kind, discard.clone(), window, cx)
                            })),
                    ),
            );
        }
        list
    }
    fn discard_new_draft(
        &self,
        kind: &'static str,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx,move|dialog,_,_|{let view=view.clone();let key=key.clone();dialog.title(crate::accessibility::text("Discard this unsaved draft?")).description(crate::accessibility::text(format!("This {kind} has never been saved. Discarding removes the draft and its contents."))).show_cancel(true).cancel_text("Keep draft").ok_text("Discard draft").on_ok(move|_,_,cx|{let _=view.update(cx,|this,cx|{let s=&mut this.settings;match kind{"role"=>{s.roles.remove(&key);if s.role==key{s.role.clear();s.role_open=false;}}"principle"=>{s.principles.remove(&key);if s.principle==key{s.principle.clear();}}_=>{s.playbooks.remove(&key);if s.playbook==key{s.playbook.clear();}}}cx.notify();});true})});
    }
    pub(super) fn repositories(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.settings.repository.is_empty() {
            self.settings.repository = NEW.into();
        }
        let id = self.settings.repository.clone();
        let current = records(&self.workspace, "repositories")
            .into_iter()
            .find(|r| r["id"] == id);
        if !self.settings.repositories.contains_key(&id) {
            let value = current.clone().unwrap_or(json!({"targetBranch":"main"}));
            self.settings
                .repositories
                .insert(id.clone(), RepositoryDraft::new(&value, window, cx));
        }
        self.apply_folder(window, cx);
        let mut panel=section("Repositories").child(hint("Choose the checkout folder; the local service inspects it and verifies it again on save. Existing runs keep their captured configuration."));
        for repo in records(&self.workspace, "repositories") {
            let target = string(&repo, "id");
            let edit = target.clone();
            let retired = !repo["retiredAt"].is_null();
            let name = string(&repo, "name");
            let revision = repo["revision"].as_u64().unwrap_or(1);
            let mut row = div()
                .id(SharedString::from(format!("repository-row-{target}")))
                .p_3()
                .border_1()
                .border_color(rgb(0x293642))
                .rounded_md()
                .flex()
                .flex_col()
                .gap_2()
                .child(crate::accessibility::text(format!(
                    "{name}{}",
                    if retired { " · Retired" } else { "" }
                )))
                .child(hint(string(&repo, "path")))
                .child(hint(format!(
                    "Target: {} · Prefix: {} · {} checks · PR publication {}",
                    string(&repo, "targetBranch"),
                    repo["branchPrefix"].as_str().unwrap_or("codex/crewroom/"),
                    records(&repo, "checks").len(),
                    publication_state(&repo)
                )));
            if let Some(reason) = repo["publicationProvider"]["reason"].as_str() {
                row = row.child(hint(reason));
            }
            if !retired {
                row = row.child(self.ticket_source_block(&repo, window, cx));
            }
            if !retired {
                row=row.child(div().flex().flex_wrap().gap_2().child(Button::new(SharedString::from(format!("edit-repository-{target}"))).label(format!("Edit {name}")).selected(target==id).disabled(self.busy).on_click(cx.listener(move|this,_,_,cx|{this.settings.repository=edit.clone();cx.notify();}))).child(Button::new(SharedString::from(format!("retire-repository-{target}"))).label("Retire repository").disabled(self.blocked()).on_click(cx.listener(move|_,_,window,cx|{let view=cx.entity().downgrade();let target=target.clone();window.open_alert_dialog(cx,move|dialog,_,_|{let view=view.clone();let target=target.clone();dialog.title(crate::accessibility::text("Retire this repository?")).description(crate::accessibility::text("New work will require another target. Saved tickets, history and worktrees are retained. Active or queued work can prevent retirement.")).show_cancel(true).cancel_text("Keep repository").ok_text("Retire repository").on_ok(move|_,_,cx|{let _=view.update(cx,|this,cx|this.send(format!("/repositories/{target}/retire"),json!({"revision":revision}),true,cx));true})});}))));
            }
            panel = panel.child(row);
        }
        panel = panel.child(
            Button::new("new-repository")
                .label("Add a repository")
                .disabled(self.busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.repository = NEW.into();
                    cx.notify();
                })),
        );
        let draft = &self.settings.repositories[&id];
        let retired = current.as_ref().is_some_and(|r| !r["retiredAt"].is_null());
        panel = panel.child(hint(match draft.revision {
            Some(r) => format!(
                "Edit repository · revision {r}{}",
                if draft.dirty(cx) {
                    " · Unsaved changes"
                } else {
                    ""
                }
            ),
            None => "New repository".into(),
        }));
        if let Some(current) = current
            && current["revision"].as_u64() != draft.revision
        {
            panel = panel.child(hint(format!(
                "The saved configuration is now revision {}. Your draft is retained.",
                current["revision"]
            )));
        }
        let problems = draft.problems(cx);
        for (key, label) in [
            ("path", "Checkout path"),
            ("name", "Repository name"),
            ("targetBranch", "Target branch"),
            ("branchPrefix", "Branch prefix"),
        ] {
            panel = panel.child(draft.fields.render(
                "repository",
                key,
                label,
                self.busy || retired,
            ));
            if let Some(problem) = problems.fields.get(key) {
                panel = panel.child(named_hint(
                    format!("repository-{key}-problem"),
                    problem.clone(),
                ));
            }
            if key == "path" {
                panel = panel.child(self.folder_block(self.busy || retired, cx));
            }
        }
        let draft = &self.settings.repositories[&id];
        panel=panel.child(hint("Leave the prefix blank to use codex/crewroom/.")).child(crate::accessibility::text("Repository checks")).child(hint("Each check uses a pinned container image. Leave the list empty if no checks are required."));
        for check in &draft.checks {
            let key = check.key.clone();
            let prefix = format!("check-{key}");
            let mut row = section(format!("Check {}", check.fields.text("id", cx)))
                .id(SharedString::from(prefix.clone()));
            for (field, label) in [
                ("id", "Check ID"),
                ("image", "Image digest"),
                ("command", "Command"),
                ("args", "Arguments (one per line)"),
                ("timeoutMs", "Timeout (ms)"),
                ("memoryMiB", "Memory (MiB, optional)"),
            ] {
                row = row.child(
                    check
                        .fields
                        .render(&prefix, field, label, self.busy || retired),
                );
                if field == "image" {
                    row = row.child(hint(
                        "Image digest · sha256: followed by 64 lowercase hex characters",
                    ));
                }
            }
            for (property, label, checked) in [
                ("required", "Required", check.required),
                ("tmp", "Executable temporary files", check.tmp_executable),
            ] {
                let key = key.clone();
                row = row.child(
                    Checkbox::new(SharedString::from(format!("{prefix}-{property}")))
                        .label(label)
                        .checked(checked)
                        .disabled(self.busy || retired)
                        .on_click(cx.listener(move |this, value, _, cx| {
                            if let Some(check) = this
                                .settings
                                .repositories
                                .get_mut(&this.settings.repository)
                                .unwrap()
                                .checks
                                .iter_mut()
                                .find(|c| c.key == key)
                            {
                                if property == "required" {
                                    check.required = *value;
                                } else {
                                    check.tmp_executable = *value;
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            row = row.child(
                Button::new(SharedString::from(format!("{prefix}-remove")))
                    .label("Remove check")
                    .disabled(self.busy || retired)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings
                            .repositories
                            .get_mut(&this.settings.repository)
                            .unwrap()
                            .checks
                            .retain(|c| c.key != key);
                        cx.notify();
                    })),
            );
            panel = panel.child(row);
        }
        let target = id.clone();
        panel
            .child(
                Button::new("add-check")
                    .label("Add check")
                    .disabled(self.busy || retired)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let check = CheckDraft::new(&json!({}), window, cx);
                        this.settings
                            .repositories
                            .get_mut(&this.settings.repository)
                            .unwrap()
                            .checks
                            .push(check);
                        cx.notify();
                    })),
            )
            .children(self.feedback(crate::workbench::feedback::Place::SaveRow))
            .children(if retired {
                None
            } else {
                self.save_blockers(&problems)
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("save-repository")
                            .primary()
                            .label("Save repository")
                            .disabled(self.blocked() || retired || !problems.blockers.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let id = this.settings.repository.clone();
                                match this.settings.repositories[&id].value(cx) {
                                    Ok(body) => this.send(
                                        if id == NEW {
                                            "/repositories".into()
                                        } else {
                                            format!("/repositories/{id}")
                                        },
                                        body,
                                        true,
                                        cx,
                                    ),
                                    Err(error) => {
                                        this.fail_at(error, feedback::Place::SaveRow);
                                        cx.notify();
                                    }
                                }
                            })),
                    )
                    .child(
                        Button::new("reload-repository")
                            .label("Reload saved repository")
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.reload_setting("repository", target.clone(), window, cx)
                            })),
                    ),
            )
    }
}

/// Publication configuration for a repository row. An unsupported publish
/// remote is reported as unavailable, distinct from no configuration.
fn publication_state(repo: &Value) -> &'static str {
    if repo["publicationAvailable"] == true {
        "configured"
    } else if repo["publicationProvider"]["state"] == "unavailable" {
        "unavailable"
    } else {
        "not configured"
    }
}
#[cfg(test)]
mod tests;
