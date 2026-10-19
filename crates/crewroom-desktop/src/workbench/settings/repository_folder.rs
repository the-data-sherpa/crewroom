//! Choose, inspect and optionally initialize a repository's checkout (FOR-70).
//!
//! Choose folder opens the system folder picker (the xdg desktop portal on
//! Linux); typing a path stays available. The local service inspects the
//! folder read-only and, only after the operator confirms, initializes a
//! non-Git folder. The desktop never runs Git itself.
use super::*;
use crate::workbench::blockers;
use std::path::PathBuf;

pub(in crate::workbench) const INSPECT: &str = "/repository-folders/inspect";
pub(in crate::workbench) const INITIALIZE: &str = "/repository-folders/initialize";

#[derive(Default, Debug, PartialEq)]
pub(in crate::workbench) enum Picker {
    #[default]
    Idle,
    Open,
    Cancelled,
    /// The picker could not open; the reason is shown beside the typed path.
    Unavailable(String),
}

#[derive(Default)]
pub(in crate::workbench) struct Folder {
    /// The inspection request awaiting its answer.
    pending: Option<String>,
    /// The latest inspection answer.
    pub(in crate::workbench) view: Option<Value>,
    /// The latest inspection request failure.
    error: Option<String>,
    pub(in crate::workbench) picker: Picker,
    /// A picked path waiting to be written to the path field.
    picked: Option<String>,
    /// An inspection waiting to prefill the form.
    prefill: Option<Value>,
    /// Values the inspection filled in, replaced by the next inspection
    /// unless the operator has edited them.
    auto_name: Option<String>,
    auto_target: Option<String>,
}

impl Folder {
    pub(in crate::workbench) fn inspecting(&self) -> bool {
        self.pending.is_some()
    }
}

/// The checkout path an inspection answered for: the resolved root, the
/// folder offered for initialization, or the refused path as requested.
fn answered(view: &Value) -> &str {
    match view["state"].as_str() {
        Some("repository") => view["root"].as_str(),
        Some("notRepository") => view["folder"].as_str(),
        _ => view["path"].as_str(),
    }
    .unwrap_or("")
}

/// Inline problems for each field, and the blockers Save names (FOR-28).
#[derive(Default)]
pub(super) struct Problems {
    pub(super) fields: HashMap<&'static str, String>,
    pub(super) blockers: Vec<String>,
}
impl Problems {
    fn add(&mut self, field: &'static str, problem: impl Into<String>, blocker: &str) {
        self.fields.entry(field).or_insert_with(|| problem.into());
        if !self.blockers.iter().any(|b| b == blocker) {
            self.blockers.push(blocker.into());
        }
    }
}

impl RepositoryDraft {
    /// Field problems known before saving. The service re-verifies on save.
    pub(super) fn problems(&self, cx: &App) -> Problems {
        let mut out = Problems::default();
        let name = self.fields.text("name", cx);
        if name.trim().is_empty() {
            out.add(
                "name",
                "Enter a repository name.",
                "enter a repository name",
            );
        } else if name.trim().chars().count() > 120 {
            out.add(
                "name",
                "Use at most 120 characters.",
                "shorten the repository name",
            );
        }
        let path = self.fields.text("path", cx);
        let path = path.trim();
        let saved_path = self.original["path"].as_str().unwrap_or("");
        let fresh = self
            .folder
            .view
            .as_ref()
            .filter(|view| !path.is_empty() && answered(view) == path);
        if path.is_empty() {
            out.add(
                "path",
                "Choose the checkout folder.",
                "choose the checkout folder",
            );
        } else if self.revision.is_none() || path != saved_path {
            match fresh {
                _ if self.folder.inspecting() => out.add(
                    "path",
                    "Inspecting this folder…",
                    "wait for the folder inspection",
                ),
                None => out.add(
                    "path",
                    "Inspect this path before saving.",
                    "inspect the checkout path",
                ),
                Some(view) if view["state"] == "notRepository" => out.add(
                    "path",
                    "This folder is not a Git repository.",
                    "initialize the folder or choose a Git checkout",
                ),
                Some(view) if view["state"] != "repository" => out.add(
                    "path",
                    view["reason"]
                        .as_str()
                        .unwrap_or("This folder cannot be used."),
                    "choose another folder",
                ),
                Some(_) => {}
            }
        }
        let target = self.fields.text("targetBranch", cx);
        let target = target.trim();
        if target.is_empty() {
            out.add(
                "targetBranch",
                "Choose a target branch.",
                "choose a target branch",
            );
        } else if let Some(view) = fresh.filter(|v| v["state"] == "repository")
            && !view["branches"]
                .as_array()
                .is_some_and(|b| b.iter().any(|b| b == target))
        {
            out.add(
                "targetBranch",
                format!("'{target}' is not a branch in this checkout."),
                "choose one of the checkout's branches",
            );
        }
        let prefix = self.fields.text("branchPrefix", cx);
        if !prefix.trim().is_empty() && !valid_prefix(&prefix) {
            out.add(
                "branchPrefix",
                "Use a branch prefix ending in /, with letters, numbers, hyphens or underscores.",
                "fix the branch prefix",
            );
        }
        out
    }
}

/// Mirrors the service's prefix rule so the problem shows before saving.
fn valid_prefix(prefix: &str) -> bool {
    prefix.chars().count() <= 100
        && prefix.ends_with('/')
        && prefix[..prefix.len() - 1].split('/').all(|part| {
            part.as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        })
}

fn set(input: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut App) {
    input.update(cx, |i, cx| i.set_value(text.to_owned(), window, cx));
}

impl Settings {
    /// Routes an inspection answer to the draft that asked for it.
    pub(in crate::workbench) fn inspected(&mut self, path: &str, result: Result<Value, String>) {
        for draft in self.repositories.values_mut() {
            let folder = &mut draft.folder;
            if folder.pending.as_deref() != Some(path) {
                continue;
            }
            folder.pending = None;
            match result {
                Ok(view) => {
                    folder.error = None;
                    folder.prefill = Some(view.clone());
                    folder.view = Some(view);
                }
                Err(error) => folder.error = Some(error),
            }
            return;
        }
    }
    /// The service answers an initialization with the folder's new inspection.
    pub(super) fn initialized(&mut self, view: &Value) {
        if let Some(draft) = self.repositories.get_mut(&self.repository) {
            draft.folder.view = Some(view.clone());
            draft.folder.prefill = Some(view.clone());
            draft.folder.error = None;
        }
    }
}

impl Workbench {
    fn folder_draft(&mut self) -> Option<&mut RepositoryDraft> {
        self.settings
            .repositories
            .get_mut(&self.settings.repository)
    }
    /// Opens the system folder picker. Cancelling changes nothing.
    pub(super) fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.folder_draft() else {
            return;
        };
        draft.folder.picker = Picker::Open;
        let answer = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose repository folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = match answer.await {
                Ok(Ok(paths)) => Ok(paths),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("the picker closed without an answer".into()),
            };
            let _ = this.update(cx, |this, cx| {
                this.folder_picked(result);
                cx.notify();
            });
        })
        .detach();
    }
    /// Applies the picker's answer: a folder is inspected, a cancellation
    /// changes nothing, and a failure keeps the typed path as the fallback.
    pub(in crate::workbench) fn folder_picked(
        &mut self,
        result: Result<Option<Vec<PathBuf>>, String>,
    ) {
        let Some(draft) = self.folder_draft() else {
            return;
        };
        match result {
            Ok(Some(paths)) if !paths.is_empty() => {
                let path = paths[0].to_string_lossy().into_owned();
                draft.folder.picker = Picker::Idle;
                draft.folder.picked = Some(path.clone());
                self.inspect_folder(path);
            }
            Ok(_) => draft.folder.picker = Picker::Cancelled,
            Err(error) => {
                let reason = error.trim().trim_end_matches('.').to_owned();
                draft.folder.picker = Picker::Unavailable(if reason.is_empty() {
                    "no reason was given".into()
                } else {
                    reason
                });
            }
        }
    }
    /// Asks the local service to inspect `path`. Read-only.
    pub(in crate::workbench) fn inspect_folder(&mut self, path: String) {
        let id = self.settings.repository.clone();
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("path", path.trim());
        if id != NEW {
            query.append_pair("repository", &id);
        }
        let request = format!("{INSPECT}?{}", query.finish());
        let sent = self
            .client
            .as_ref()
            .ok_or_else(|| "No service connection.".to_owned())
            .and_then(|c| c.read(request.clone()));
        let Some(draft) = self.folder_draft() else {
            return;
        };
        match sent {
            Ok(()) => {
                draft.folder.pending = Some(request);
                draft.folder.error = None;
            }
            Err(error) => draft.folder.error = Some(error),
        }
    }
    /// Writes a picked path and an inspection's prefill into the form.
    pub(super) fn apply_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self
            .settings
            .repositories
            .get_mut(&self.settings.repository)
        else {
            return;
        };
        if let Some(path) = draft.folder.picked.take() {
            set(&draft.fields.inputs["path"], &path, window, cx);
        }
        let Some(view) = draft.folder.prefill.take() else {
            return;
        };
        let path = answered(&view).to_owned();
        if !path.is_empty() {
            set(&draft.fields.inputs["path"], &path, window, cx);
        }
        let state = view["state"].as_str().unwrap_or("");
        if state != "repository" && state != "notRepository" {
            return;
        }
        let name = draft.fields.text("name", cx);
        let suggested = view["name"].as_str().unwrap_or("");
        if !suggested.is_empty()
            && (name.trim().is_empty() || draft.folder.auto_name.as_deref() == Some(name.as_str()))
        {
            set(&draft.fields.inputs["name"], suggested, window, cx);
            draft.folder.auto_name = Some(suggested.to_owned());
        }
        let target = draft.fields.text("targetBranch", cx);
        let untouched = target.trim().is_empty()
            || draft.folder.auto_target.as_deref() == Some(target.as_str())
            || (draft.revision.is_none() && draft.original["targetBranch"] == target.as_str());
        let suggested = if state == "repository" {
            let known = view["branches"]
                .as_array()
                .is_some_and(|b| b.iter().any(|b| b == target.as_str()));
            (untouched || !known).then(|| view["defaultBranch"].as_str().unwrap_or(""))
        } else {
            untouched.then(|| view["initialBranch"].as_str().unwrap_or(""))
        };
        if let Some(branch) = suggested.filter(|b| !b.is_empty()) {
            set(&draft.fields.inputs["targetBranch"], branch, window, cx);
            draft.folder.auto_target = Some(branch.to_owned());
        }
    }
    fn confirm_initialize(
        &self,
        folder: String,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let (folder, branch) = (folder.clone(), branch.clone());
            dialog
                .title(crate::accessibility::text("Initialize a Git repository here?"))
                .description(crate::accessibility::text(format!(
                    "Crewroom will run git init in {folder} on branch {branch} and create one empty commit. Existing files are not staged or committed."
                )))
                .show_cancel(true)
                .cancel_text("Don't initialize")
                .ok_text("Initialize repository")
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.send(
                            INITIALIZE.into(),
                            json!({"path":folder,"targetBranch":branch,"confirmed":true}),
                            true,
                            cx,
                        )
                    });
                    true
                })
        });
    }
    /// The folder chooser, inspection result and branch and remote choices.
    pub(super) fn folder_block(&self, locked: bool, cx: &mut Context<Self>) -> Div {
        let draft = &self.settings.repositories[&self.settings.repository];
        let folder = &draft.folder;
        let path = draft.fields.text("path", cx).trim().to_owned();
        let mut block = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("choose-repository-folder")
                        .label("Choose folder")
                        .disabled(locked || folder.picker == Picker::Open)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.choose_folder(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("inspect-repository-folder")
                        .label("Inspect path")
                        .disabled(locked || path.is_empty() || folder.inspecting())
                        .on_click(cx.listener(|this, _, _, cx| {
                            let path = this.settings.repositories[&this.settings.repository]
                                .fields
                                .text("path", cx);
                            this.inspect_folder(path);
                            cx.notify();
                        })),
                ),
        );
        block = block.child(match &folder.picker {
            Picker::Idle => named_hint(
                "repository-picker-state",
                "Choose the checkout folder, or type its path and choose Inspect path.",
            ),
            Picker::Open => named_hint(
                "repository-picker-state",
                "Waiting for the system folder picker…",
            ),
            Picker::Cancelled => named_hint(
                "repository-picker-state",
                "Folder choice cancelled. Nothing changed.",
            ),
            Picker::Unavailable(reason) => named_hint(
                "repository-picker-state",
                format!(
                    "The system folder picker is unavailable: {reason}. Type the checkout path above and choose Inspect path."
                ),
            ),
        });
        if folder.inspecting() {
            block = block.child(named_hint(
                "repository-inspection",
                "Inspecting the folder…",
            ));
            return block;
        }
        if let Some(error) = &folder.error {
            block = block.child(named_hint(
                "repository-inspection",
                format!("Inspection failed: {error} The form is unchanged."),
            ));
            return block;
        }
        let Some(view) = &folder.view else {
            return block;
        };
        if answered(view) != path {
            return block.child(named_hint(
                "repository-inspection",
                "The path changed since it was inspected. Choose Inspect path.",
            ));
        }
        match view["state"].as_str() {
            Some("repository") => {
                if let Some(notice) = view["notice"].as_str() {
                    block = block.child(named_hint("repository-root-notice", notice));
                }
                let target = draft.fields.text("targetBranch", cx);
                let default = view["defaultBranch"].as_str().unwrap_or("");
                block = block.child(named_hint(
                    "repository-default-branch",
                    format!(
                        "Git checkout · default target {default} ({})",
                        match view["defaultSource"].as_str() {
                            Some("remote") => "the remote's default branch",
                            Some("current") => "the current branch",
                            _ => "a local branch",
                        }
                    ),
                ));
                let mut branches = div().flex().flex_wrap().gap_2();
                for (index, branch) in records(view, "branches").iter().enumerate() {
                    let branch = branch.as_str().unwrap_or("").to_owned();
                    let chosen = branch.clone();
                    branches = branches.child(
                        Button::new(SharedString::from(format!("target-branch-{index}")))
                            .label(branch.clone())
                            .accessibility_label(format!("Target branch {branch}"))
                            .selected(target.trim() == branch)
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(draft) = this
                                    .settings
                                    .repositories
                                    .get_mut(&this.settings.repository)
                                {
                                    set(&draft.fields.inputs["targetBranch"], &chosen, window, cx);
                                }
                                cx.notify();
                            })),
                    );
                }
                block = block
                    .child(crate::accessibility::text("Choose the target branch"))
                    .child(branches);
                let remotes = records(view, "remotes");
                if remotes.is_empty() {
                    block = block.child(named_hint(
                        "repository-remote-none",
                        "No remotes. Pull requests need a publish remote; add one to this checkout later.",
                    ));
                }
                for (index, remote) in remotes.iter().enumerate() {
                    let host = match remote["kind"].as_str() {
                        Some("github") | Some("bitbucket") => {
                            remote["label"].as_str().unwrap_or("").to_owned()
                        }
                        Some("other") => {
                            format!("{} (other host)", remote["host"].as_str().unwrap_or(""))
                        }
                        _ => "unrecognised address (not shown)".into(),
                    };
                    block = block.child(named_hint(
                        format!("repository-remote-{index}"),
                        format!(
                            "{} · {} · {host}",
                            if remote["publish"] == true {
                                "Publish remote"
                            } else {
                                "Remote"
                            },
                            remote["name"].as_str().unwrap_or("")
                        ),
                    ));
                }
            }
            Some("notRepository") => {
                let branch = draft.fields.text("targetBranch", cx).trim().to_owned();
                let entries = view["entries"].as_u64().unwrap_or(0);
                let target = answered(view).to_owned();
                block = block
                    .child(named_hint(
                        "repository-inspection",
                        "This folder is not a Git repository. Initialize one here, or choose a folder inside an existing checkout.",
                    ))
                    .child(named_hint(
                        "repository-initialize-detail",
                        format!(
                            "Initialize runs git init on branch {} and creates one empty commit. {}",
                            if branch.is_empty() { "(choose a target branch)" } else { &branch },
                            if entries == 0 {
                                "The folder is empty.".to_owned()
                            } else {
                                format!("Its {entries} existing entries are not staged or committed.")
                            }
                        ),
                    ))
                    .child(
                        Button::new("initialize-repository")
                            .label("Initialize repository here")
                            .disabled(locked || branch.is_empty() || self.blocked())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_initialize(target.clone(), branch.clone(), window, cx)
                            })),
                    );
            }
            _ => {
                block = block
                    .child(named_hint(
                        "repository-inspection",
                        format!(
                            "Can't use this folder: {}",
                            view["reason"].as_str().unwrap_or("")
                        ),
                    ))
                    .child(named_hint(
                        "repository-remedy",
                        view["remedy"].as_str().unwrap_or("").to_owned(),
                    ));
            }
        }
        block
    }
    pub(super) fn save_blockers(&self, problems: &Problems) -> Option<Div> {
        blockers::line(
            "save-repository-blockers",
            "save this repository",
            &problems.blockers,
        )
    }
}
