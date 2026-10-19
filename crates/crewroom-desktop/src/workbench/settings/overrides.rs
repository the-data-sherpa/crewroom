//! FOR-79: Settings scope. Settings default to **All projects**; choosing a
//! project at the top of Settings shows that project's overrides for job
//! models and budget, available roles, instructions and principles, and run
//! capacity. Each setting names where its value comes from (built-in
//! default, workspace or project override), shows the full chain on demand,
//! and offers Reset to inherit again. Workspace-only settings are read-only
//! while a project is selected. The service resolves the same chain at
//! admission; assignments keep the values they captured.
use super::*;
use crewroom_core::project_settings::{
    self as ps, DEFAULT, PROJECT, Resolved, WORKSPACE, WORKSPACE_KEYS,
};

pub(in crate::workbench) const SCOPE_PATH: &str = "/project-settings";

pub(in crate::workbench) fn source_label(source: &str) -> &'static str {
    match source {
        PROJECT => "Project override",
        WORKSPACE => "Workspace",
        _ => "Built-in default",
    }
}

/// Text field drafts, one per scope and key, so saving one setting never
/// discards another's draft.
#[derive(Default)]
pub(in crate::workbench) struct ScopeDrafts {
    pub(in crate::workbench) fields: HashMap<String, Fields>,
    pub(in crate::workbench) chains: std::collections::HashSet<String>,
}
fn draft_key(scope: Option<&str>, key: &str) -> String {
    format!("{}:{key}", scope.unwrap_or(""))
}

impl Workbench {
    /// The selected project while it is still active; `None` is All projects.
    pub(in crate::workbench) fn settings_scope(&self) -> Option<(String, String)> {
        let id = self.settings.scope.as_ref()?;
        records(&self.workspace, "repositories")
            .into_iter()
            .find(|r| r["id"] == *id && r["retiredAt"].is_null())
            .map(|r| (id.clone(), string(&r, "name")))
    }
    fn project_name(&self, id: &str) -> String {
        records(&self.workspace, "repositories")
            .into_iter()
            .find(|r| r["id"] == id)
            .map(|r| string(&r, "name"))
            .unwrap_or_else(|| "Unknown project".into())
    }
    pub(in crate::workbench) fn resolved_setting(
        &self,
        key: &str,
        project: Option<&str>,
    ) -> Resolved {
        ps::resolve(
            key,
            &self.workspace["projectSettings"],
            project,
            &self.workspace["modelJobs"],
            &records(&self.workspace, "roles"),
        )
    }
    fn describe(&self, key: &str, value: &Value) -> String {
        if key.starts_with("roles.") {
            return if value == true {
                "Allowed"
            } else {
                "Not allowed"
            }
            .into();
        }
        match key {
            "capacity" => match value.as_u64() {
                Some(1) => "1 run at a time".into(),
                Some(n) => format!("{n} parallel runs"),
                None => "Not set".into(),
            },
            "principles" => {
                let catalog = records(&self.workspace, "principles");
                let names: Vec<String> = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|id| {
                        catalog
                            .iter()
                            .find(|p| p["id"] == *id)
                            .map(|p| string(p, "name"))
                            .unwrap_or_else(|| {
                                format!("{} (unavailable)", id.as_str().unwrap_or(""))
                            })
                    })
                    .collect();
                if names.is_empty() {
                    "None".into()
                } else {
                    names.join(", ")
                }
            }
            _ => match value.as_str() {
                Some("") | None => {
                    if key == "instructions" {
                        "None".into()
                    } else {
                        "Not configured".into()
                    }
                }
                Some(text) => {
                    let short: String = text.chars().take(120).collect();
                    if short.len() < text.len() {
                        format!("{short}…")
                    } else {
                        short
                    }
                }
            },
        }
    }

    /// "Settings for" selector shown at the top of every Settings page.
    pub(in crate::workbench) fn scope_bar(&self, cx: &mut Context<Self>) -> Div {
        let current = self.settings_scope().map(|(id, _)| id);
        let mut bar = div()
            .id("settings-scope")
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(crate::accessibility::text("Settings for"))
            .child(
                Button::new("settings-scope-all")
                    .label("All projects")
                    .selected(current.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.scope = None;
                        cx.notify();
                    })),
            );
        for project in self.ordered_projects() {
            let id = string(&project, "id");
            let target = id.clone();
            bar = bar.child(
                Button::new(SharedString::from(format!("settings-scope-{id}")))
                    .label(string(&project, "name"))
                    .selected(current.as_deref() == Some(id.as_str()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_settings_scope(Some(target.clone()));
                        cx.notify();
                    })),
            );
        }
        let note = match self.settings_scope() {
            Some((_, name)) => format!(
                "Showing {name}. Overridable settings below apply only to {name}; workspace-only settings are read-only. Choose All projects to change them."
            ),
            None => {
                "Showing All projects. Choose a project to override settings for it alone.".into()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(bar)
            .child(named_hint("settings-scope-note", note))
    }
    pub(in crate::workbench) fn select_settings_scope(&mut self, scope: Option<String>) {
        if let Some(id) = &scope {
            // The project form follows the selected project.
            self.settings.repository = id.clone();
        }
        self.settings.scope = scope;
    }

    fn save_scope(
        &mut self,
        scope: Option<String>,
        set: Value,
        reset: &[&str],
        cx: &mut Context<Self>,
    ) {
        let mut body = json!({"repositoryId": scope});
        if set.as_object().is_some_and(|m| !m.is_empty()) {
            body["set"] = set;
        }
        if !reset.is_empty() {
            body["reset"] = json!(reset);
        }
        self.settings.scope_pending = Some(body.clone());
        self.send(SCOPE_PATH.into(), body, false, cx);
    }
    /// Drops drafts for the keys a save changed.
    pub(in crate::workbench) fn scope_saved(&mut self, body: &Value) {
        let scope = body["repositoryId"].as_str();
        let keys = body["set"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.keys().cloned())
            .chain(
                body["reset"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k.as_str().map(str::to_owned)),
            );
        for key in keys.collect::<Vec<_>>() {
            self.settings
                .scope_drafts
                .fields
                .remove(&draft_key(scope, &key));
        }
    }

    /// One setting: label, value, source, the chain on demand, an editor and
    /// Reset; in All projects, how many projects override it, with links.
    fn setting_row(
        &self,
        key: &str,
        label: &str,
        editor: Option<Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let scope = self.settings_scope();
        let project = scope.as_ref().map(|(id, _)| id.as_str());
        let resolved = self.resolved_setting(key, project);
        let id = key.replace('.', "-");
        let mut row = div()
            .id(SharedString::from(format!("setting-{id}")))
            .p_3()
            .border_1()
            .border_color(rgb(0x293642))
            .rounded_md()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(crate::accessibility::text(format!(
                        "{label}: {}",
                        self.describe(key, &resolved.value)
                    )))
                    .child(named_hint(
                        format!("source-{id}"),
                        format!("Source: {}", source_label(resolved.source)),
                    )),
            );
        let open = self.settings.scope_drafts.chains.contains(key);
        let toggle = key.to_owned();
        let mut actions = div().flex().flex_wrap().gap_2().child(
            Button::new(SharedString::from(format!("chain-{id}")))
                .ghost()
                .label(if open {
                    "Hide where this comes from"
                } else {
                    "Where this comes from"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let chains = &mut this.settings.scope_drafts.chains;
                    if !chains.remove(&toggle) {
                        chains.insert(toggle.clone());
                    }
                    cx.notify();
                })),
        );
        let resettable = match project {
            Some(_) => resolved.source == PROJECT,
            None => WORKSPACE_KEYS.contains(&key) && resolved.source == WORKSPACE,
        };
        if resettable {
            let key_owned = key.to_owned();
            let target = project.map(str::to_owned);
            actions = actions.child(
                Button::new(SharedString::from(format!("reset-{id}")))
                    .label(if project.is_some() {
                        "Reset to inherit"
                    } else {
                        "Reset to default"
                    })
                    .disabled(self.blocked())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_scope(target.clone(), json!({}), &[key_owned.as_str()], cx)
                    })),
            );
        }
        row = row.child(actions);
        if open {
            let mut chain = div().flex().flex_col().gap_1();
            for (source, value) in &resolved.chain {
                let layer = match *source {
                    DEFAULT => "Built-in default".to_owned(),
                    WORKSPACE => "Workspace".to_owned(),
                    _ => format!(
                        "Project ({})",
                        scope.as_ref().map(|s| s.1.as_str()).unwrap_or("")
                    ),
                };
                let text = match value {
                    Some(v) => self.describe(key, v),
                    None => "not set · inherits".into(),
                };
                let effect = if *source == resolved.source {
                    " · in effect"
                } else {
                    ""
                };
                chain = chain.child(named_hint(
                    format!("chain-{id}-{source}"),
                    format!("{layer}: {text}{effect}"),
                ));
            }
            row = row.child(chain);
        }
        if project.is_none() {
            let overriding = ps::overriding_projects(key, &self.workspace["projectSettings"]);
            let summary = match overriding.len() {
                0 => "No project overrides this".to_owned(),
                1 => "Overridden in 1 project".to_owned(),
                n => format!("Overridden in {n} projects"),
            };
            let mut links = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(named_hint(format!("overrides-{id}"), summary));
            for repo in overriding {
                let target = repo.clone();
                links = links.child(
                    Button::new(SharedString::from(format!("override-{id}-{repo}")))
                        .ghost()
                        .label(self.project_name(&repo))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_settings_scope(Some(target.clone()));
                            cx.notify();
                        })),
                );
            }
            row = row.child(links);
        }
        if let Some(editor) = editor {
            row = row.child(editor);
        }
        row
    }

    fn scope_field(
        &mut self,
        key: &str,
        label: &'static str,
        multiline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let scope = self.settings_scope().map(|(id, _)| id);
        let draft = draft_key(scope.as_deref(), key);
        if !self.settings.scope_drafts.fields.contains_key(&draft) {
            let value = self.resolved_setting(key, scope.as_deref()).value;
            let fields = Fields::new(
                &json!({"value": value}),
                &[("value", multiline)],
                window,
                cx,
            );
            self.settings
                .scope_drafts
                .fields
                .insert(draft.clone(), fields);
        }
        let id = key.replace('.', "-");
        let fields = &self.settings.scope_drafts.fields[&draft];
        let target = scope.clone();
        let key_owned = key.to_owned();
        let button = if scope.is_some() {
            "Use for this project"
        } else {
            "Save for all projects"
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(fields.render(&format!("scope-{id}"), "value", label, self.busy))
            .child(
                Button::new(SharedString::from(format!("set-{id}")))
                    .label(button)
                    .disabled(self.blocked())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let draft = draft_key(target.as_deref(), &key_owned);
                        let text = this.settings.scope_drafts.fields[&draft].text("value", cx);
                        this.save_scope(target.clone(), json!({key_owned.clone(): text}), &[], cx)
                    })),
            )
    }

    /// The overridable settings that belong on the current Settings page.
    pub(in crate::workbench) fn scope_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let scope = self.settings_scope();
        let target = scope.as_ref().map(|(id, _)| id.clone());
        let title = match &scope {
            Some((_, name)) => format!("Overrides for {name}"),
            None => "Project overrides".into(),
        };
        let mut page = section(title).id("scope-section");
        match self.panel {
            Panel::Models => {
                page = page.child(hint(match &scope {
                    Some(_) => "Budget and job models for new assignments in this project only. Role overrides still take precedence; existing runs keep their captured model and effort.",
                    None => "Projects that override models or budget. Edit the workspace values below.",
                }));
                let editor = target.is_some().then(|| {
                    let current = self
                        .resolved_setting("models.budget", target.as_deref())
                        .value;
                    let mut row = div().flex().flex_wrap().gap_2();
                    for budget in crewroom_core::model_jobs::BUDGETS {
                        let t = target.clone();
                        row = row.child(
                            Button::new(SharedString::from(format!("scope-budget-{budget}")))
                                .label(budget)
                                .selected(current == budget)
                                .disabled(self.blocked())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_scope(
                                        t.clone(),
                                        json!({"models.budget": budget}),
                                        &[],
                                        cx,
                                    )
                                })),
                        );
                    }
                    row
                });
                page = page.child(self.setting_row("models.budget", "Budget", editor, cx));
                for (job, label) in [
                    ("code", "Code model"),
                    ("judgment", "Judgment model"),
                    ("review", "Review model"),
                    ("fast", "Fast model"),
                ] {
                    let key = format!("models.jobs.{job}");
                    let editor = target
                        .is_some()
                        .then(|| self.scope_field(&key, label, false, window, cx));
                    page = page.child(self.setting_row(&key, label, editor, cx));
                }
            }
            Panel::Roles => {
                let roles = records(&self.workspace, "roles");
                match &scope {
                    Some((_, name)) => {
                        page = page.child(hint(format!(
                            "Which roles may be assigned in {name}. Role definitions stay workspace settings and are read-only below."
                        )));
                        for role in &roles {
                            let id = string(role, "id");
                            let key = format!("roles.{id}");
                            let allowed =
                                self.resolved_setting(&key, target.as_deref()).value == true;
                            let t = target.clone();
                            let set_key = key.clone();
                            let editor = div().child(
                                Button::new(SharedString::from(format!("scope-role-{id}")))
                                    .label(if allowed {
                                        format!("Don't allow in {name}")
                                    } else {
                                        format!("Allow in {name}")
                                    })
                                    .disabled(self.blocked())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.save_scope(
                                            t.clone(),
                                            json!({set_key.clone(): !allowed}),
                                            &[],
                                            cx,
                                        )
                                    })),
                            );
                            page = page.child(self.setting_row(
                                &key,
                                &string(role, "name"),
                                Some(editor),
                                cx,
                            ));
                        }
                    }
                    None => {
                        let overridden: Vec<_> = roles
                            .iter()
                            .filter(|r| {
                                !ps::overriding_projects(
                                    &format!("roles.{}", string(r, "id")),
                                    &self.workspace["projectSettings"],
                                )
                                .is_empty()
                            })
                            .collect();
                        // The role catalog fills the page; the summary
                        // appears only when a project changes a role.
                        if overridden.is_empty() {
                            return None;
                        }
                        for role in overridden {
                            let key = format!("roles.{}", string(role, "id"));
                            page =
                                page.child(self.setting_row(&key, &string(role, "name"), None, cx));
                        }
                    }
                }
            }
            Panel::Principles => {
                page = page.child(hint(match &scope {
                    Some(_) => "Instructions and principles added to every new assignment in this project, after each role's own. Existing runs keep what they captured.",
                    None => "Instructions and principles added to every new assignment in every project unless a project overrides them.",
                }));
                let editor = self.scope_field("instructions", "Instructions", true, window, cx);
                page =
                    page.child(self.setting_row("instructions", "Instructions", Some(editor), cx));
                let current = self.resolved_setting("principles", target.as_deref()).value;
                let selected: Vec<Value> = current.as_array().cloned().unwrap_or_default();
                let mut boxes = div().flex().flex_col().gap_1();
                let catalog = records(&self.workspace, "principles");
                if catalog.is_empty() {
                    boxes = boxes.child(hint(
                        "No principles yet. Create one below with All projects selected.",
                    ));
                }
                for principle in catalog {
                    let id = string(&principle, "id");
                    let on = selected.iter().any(|v| v == id.as_str());
                    let t = target.clone();
                    let list = selected.clone();
                    boxes = boxes.child(
                        Checkbox::new(SharedString::from(format!("scope-principle-{id}")))
                            .label(string(&principle, "name"))
                            .checked(on)
                            .disabled(self.blocked())
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let mut next: Vec<Value> = list
                                    .iter()
                                    .filter(|v| **v != id.as_str())
                                    .cloned()
                                    .collect();
                                if *checked {
                                    next.push(json!(id));
                                }
                                this.save_scope(t.clone(), json!({"principles": next}), &[], cx)
                            })),
                    );
                }
                page = page.child(self.setting_row("principles", "Principles", Some(boxes), cx));
            }
            Panel::Repositories => {
                page = page.child(hint(match &scope {
                    Some(_) => "Run capacity for this project. Checks are always per project; edit them in the project form below.",
                    None => "Default run capacity for projects without their own. Lowering it never stops running work.",
                }));
                let current = self
                    .resolved_setting("capacity", target.as_deref())
                    .value
                    .as_u64()
                    .unwrap_or(1);
                let max = crewroom_core::capacity::MAX_CAPACITY;
                let step = |label: &'static str,
                            id: &'static str,
                            next: u64,
                            enabled: bool,
                            this: &Self,
                            cx: &mut Context<Self>| {
                    let t = target.clone();
                    Button::new(id)
                        .label(label)
                        .disabled(this.blocked() || !enabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.save_scope(t.clone(), json!({"capacity": next}), &[], cx)
                        }))
                };
                let editor = div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(step(
                        "Fewer parallel runs",
                        "scope-capacity-down",
                        current.saturating_sub(1).max(1),
                        current > 1,
                        self,
                        cx,
                    ))
                    .child(step(
                        "More parallel runs",
                        "scope-capacity-up",
                        (current + 1).min(max),
                        current < max,
                        self,
                        cx,
                    ));
                page = page.child(self.setting_row("capacity", "Run capacity", Some(editor), cx));
            }
            _ => return None,
        }
        Some(div().child(page))
    }

    /// Workspace-only content while a project is selected: visible, dimmed
    /// and covered so it cannot be changed. `send` refuses its mutations too.
    pub(in crate::workbench) fn read_only_workspace(&self, content: Div) -> Div {
        let Some((_, name)) = self.settings_scope() else {
            return content;
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(named_hint(
                "workspace-read-only",
                format!("Workspace settings · read-only while {name} is selected. Choose All projects to change them."),
            ))
            .child(
                div()
                    .relative()
                    .child(div().opacity(0.55).child(content))
                    .child(
                        div()
                            .id("workspace-read-only-cover")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            // Scrolling still reaches the page.
                            .block_mouse_except_scroll(),
                    ),
            )
    }

    /// While a project is selected in Settings, only that project's settings
    /// may change.
    pub(in crate::workbench) fn scope_refusal(&self, path: &str, body: &Value) -> Option<String> {
        let (id, name) = self.settings_scope()?;
        if !matches!(
            self.panel,
            Panel::Roles
                | Panel::Models
                | Panel::Runner
                | Panel::Principles
                | Panel::Playbooks
                | Panel::Repositories
                | Panel::Learning
                | Panel::Import
        ) {
            return None;
        }
        let allowed = match path {
            SCOPE_PATH => body["repositoryId"] == id.as_str(),
            "/execution-capacity" => body.get("workspaceCeiling").is_none(),
            "/role-preview" | "/model-catalog/refresh" => true,
            p => p.starts_with("/repositories") || p.starts_with("/repository-folders"),
        };
        (!allowed).then(|| {
            format!("Workspace settings are read-only while {name} is selected. Choose All projects to change them.")
        })
    }
}

#[cfg(test)]
#[path = "overrides_tests.rs"]
mod tests;
