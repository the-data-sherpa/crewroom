//! Settings → Runner (FOR-83). The local service reports each runner
//! prerequisite and the saved configuration without secrets; secrets typed
//! here are sent once and stored encrypted by the service.
use super::*;

pub(in crate::workbench) const READ: &str = "/runner";
const FIELDS: [(&str, &str); 10] = [
    ("root", "Runtime root folder"),
    (
        "mailUrl",
        "Rust Agent Mail URL (http://127.0.0.1:PORT/mcp/)",
    ),
    ("mailProjectRoot", "Rust Agent Mail project folder"),
    ("dockerSocket", "Docker engine socket"),
    (
        "evaluationImage",
        "Evaluation image (optional, pinned digest)",
    ),
    ("ompBinary", "OMP executable (optional)"),
    ("codexBinary", "Codex executable (optional)"),
    ("codexAuthFile", "Codex auth file"),
    ("claudeBinary", "Claude Code executable (optional)"),
    ("claudeHome", "Claude Code home folder"),
];

/// The form, rebuilt from each newly read view while it has no edits.
pub(in crate::workbench) struct RunnerForm {
    fields: Fields,
    advanced: Entity<TextareaState>,
    mail: Entity<InputState>,
    provider: Entity<InputState>,
    provider_key: Entity<InputState>,
    original: (Value, String),
}
impl RunnerForm {
    fn new(view: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let keys: Vec<(&'static str, bool)> = FIELDS.iter().map(|(k, _)| (*k, false)).collect();
        let fields = Fields::new(&view["form"], &keys, window, cx);
        let text = advanced_text(&view["advanced"]);
        let advanced = cx.new(|cx| TextareaState::new(window, cx).default_value(text.clone()));
        let masked =
            |placeholder: &'static str, window: &mut Window, cx: &mut Context<Workbench>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder(placeholder)
                })
            };
        let mail = masked(
            if view["secrets"]["mail"] == "stored" {
                "Saved encrypted · leave blank to keep"
            } else {
                "Paste the Mail bearer token"
            },
            window,
            cx,
        );
        let provider_key = masked("Paste the provider key", window, cx);
        let provider = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Provider name from models.providers")
        });
        let original = (fields.values(cx), text);
        Self {
            fields,
            advanced,
            mail,
            provider,
            provider_key,
            original,
        }
    }
    fn dirty(&self, cx: &App) -> bool {
        self.fields.values(cx) != self.original.0
            || self.advanced.read(cx).value().as_ref() != self.original.1
            || !self.mail.read(cx).value().is_empty()
            || !self.provider_key.read(cx).value().is_empty()
    }
    /// The request body. Secrets are included only when typed.
    fn body(&self, cx: &App) -> Result<Value, String> {
        let text = self.advanced.read(cx).value().to_string();
        let advanced: Value = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text)
                .map_err(|e| format!("Advanced settings are not valid JSON: {e}"))?
        };
        if !advanced.is_object() {
            return Err("Advanced settings must be a JSON object.".into());
        }
        let mut secrets = json!({});
        let mail = self.mail.read(cx).value().trim().to_owned();
        if !mail.is_empty() {
            secrets["mail"] = json!(mail);
        }
        let key = self.provider_key.read(cx).value().trim().to_owned();
        if !key.is_empty() {
            let name = self.provider.read(cx).value().trim().to_owned();
            if name.is_empty() {
                return Err("Enter the provider name the key belongs to.".into());
            }
            secrets["providers"] = json!({ name: key });
        }
        Ok(json!({"fields": self.fields.values(cx), "advanced": advanced, "secrets": secrets}))
    }
}
fn advanced_text(value: &Value) -> String {
    match value {
        Value::Object(o) if !o.is_empty() => {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
        _ => "{\n  \"catalog\": [],\n  \"policy\": {}\n}".into(),
    }
}
#[derive(Default)]
pub(in crate::workbench) struct RunnerPage {
    pub view: Option<Result<Value, String>>,
    /// A read is queued or should be (stale view).
    pub reading: bool,
    pub stale: bool,
    pub form: Option<RunnerForm>,
    /// The view the form was built from.
    form_source: Value,
    /// Efforts the operator picked per offering (FOR-87), keyed by
    /// harness, provider and selector. Absent: the saved profile's efforts.
    pub efforts: std::collections::HashMap<String, Vec<String>>,
}
impl RunnerPage {
    pub(in crate::workbench) fn read(&mut self, result: Result<Value, String>) {
        self.reading = false;
        self.view = Some(result);
    }
    pub(in crate::workbench) fn unsaved(&self, cx: &App) -> bool {
        self.form.as_ref().is_some_and(|f| f.dirty(cx))
    }
    /// After a save or apply the next view replaces the form.
    pub(in crate::workbench) fn saved(&mut self) {
        self.form = None;
        self.stale = true;
        self.efforts.clear();
    }
    /// The efforts shown as chosen for an offering.
    fn chosen(&self, offering: &Value) -> Vec<String> {
        self.efforts
            .get(&offering_key(offering))
            .cloned()
            .unwrap_or_else(|| {
                offering["profile"]["efforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect()
            })
    }
}
fn offering_key(offering: &Value) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        string(offering, "harness"),
        string(offering, "provider"),
        string(offering, "selector")
    )
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str().map(str::to_owned))
        .collect()
}
/// One offering line, readable without colour: whether agents may run on it.
pub(in crate::workbench) fn offering_line(offering: &Value) -> String {
    let mut line = format!(
        "{} · {}/{}",
        offering["displayName"].as_str().unwrap_or(""),
        offering["provider"].as_str().unwrap_or(""),
        offering["selector"].as_str().unwrap_or("")
    );
    if let Some(model) = offering["resolvedModel"].as_str()
        && model != offering["selector"].as_str().unwrap_or("")
    {
        line.push_str(&format!(" · reports {model}"));
    }
    if offering["profile"].is_object() {
        line.push_str(&format!(
            " · Approved for efforts {}",
            strings(&offering["profile"]["efforts"]).join(", ")
        ));
    } else {
        line.push_str(" · Not approved");
    }
    if offering["fresh"] != true {
        line.push_str(" · discovery is stale");
    }
    line
}
fn state_tone(state: &str) -> u32 {
    match state {
        "ready" => 0x84d7bc,
        "failing" => 0xf28b82,
        _ => 0xf1bf73,
    }
}
fn state_label(state: &str) -> &'static str {
    match state {
        "ready" => "Ready",
        "failing" => "Failing",
        _ => "Missing",
    }
}
/// One prerequisite line, readable without colour.
pub(in crate::workbench) fn prerequisite_line(item: &Value) -> String {
    let mut line = format!(
        "{} · {} · {}",
        item["label"].as_str().unwrap_or(""),
        state_label(item["state"].as_str().unwrap_or("")),
        item["detail"].as_str().unwrap_or("")
    );
    if item["required"] == false {
        line.push_str(" · optional");
    }
    line
}
impl Workbench {
    pub(in crate::workbench) fn runner_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let page = &mut self.settings.runner;
        if (page.view.is_none() || page.stale)
            && !page.reading
            && self.connected
            && let Some(client) = &self.client
            && client.read(READ.into()).is_ok()
        {
            page.reading = true;
            page.stale = false;
        }
        let mut panel = section("Runner").child(hint(
            "What the agent runner needs and what this machine has. Ready means configured and reachable; model access is confirmed only when an agent runs.",
        ));
        let view = match &self.settings.runner.view {
            None => return panel.child(named_hint("runner-loading", "Checking the runner…")),
            Some(Err(error)) => {
                return panel
                    .child(named_hint(
                        "runner-unavailable",
                        format!("Runner status unavailable: {error}"),
                    ))
                    .child(
                        Button::new("runner-check")
                            .label("Check again")
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.runner.stale = true;
                                cx.notify();
                            })),
                    );
            }
            Some(Ok(view)) => view.clone(),
        };
        let page = &mut self.settings.runner;
        if page.form.is_none()
            || (page.form.as_ref().is_some_and(|f| !f.dirty(cx)) && page.form_source != view)
        {
            page.form = Some(RunnerForm::new(&view, window, cx));
            page.form_source = view.clone();
        }
        panel = panel
            .child(named_hint(
                "runner-source",
                format!(
                    "Configuration: {}",
                    view["source"]["label"].as_str().unwrap_or("")
                ),
            ))
            .child(named_hint(
                "runner-active",
                if view["active"] == true {
                    "Runner running in this service."
                } else {
                    "Runner not running: no agent can start."
                },
            ));
        if let Some(error) = view["error"].as_str() {
            panel = panel.child(named_hint("runner-error", format!("Problem: {error}")));
        }
        if view["pending"] == true {
            panel = panel.child(named_hint(
                "runner-pending",
                format!(
                    "Saved changes are not applied yet. {}",
                    view["restart"]["reason"].as_str().unwrap_or("")
                ),
            ));
        }
        for run in records(&view, "liveRuns") {
            panel = panel.child(named_hint(
                format!("runner-live-{}", string(&run, "id")),
                format!(
                    "Owns live processes: {} ({})",
                    run["title"].as_str().unwrap_or("untitled run"),
                    run["status"].as_str().unwrap_or("")
                ),
            ));
        }
        panel = panel.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("runner-apply")
                        .label("Apply now")
                        .disabled(
                            self.blocked()
                                || view["pending"] != true
                                || view["restart"]["available"] != true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send("/runner/apply".into(), json!({}), false, cx)
                        })),
                )
                .child(
                    Button::new("runner-check")
                        .label("Check again")
                        .disabled(self.blocked() || self.settings.runner.reading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.runner.stale = true;
                            cx.notify();
                        })),
                ),
        );
        let mut list = div()
            .id("runner-prerequisites")
            .flex()
            .flex_col()
            .gap_2()
            .child(crate::accessibility::text(if view["ready"] == true {
                "Prerequisites · all required items ready"
            } else {
                "Prerequisites"
            }));
        for item in records(&view, "prerequisites") {
            let id = string(&item, "id");
            let state = item["state"].as_str().unwrap_or("missing");
            let mut row = div()
                .id(SharedString::from(format!("runner-item-{id}")))
                .p_3()
                .border_1()
                .border_color(rgb(0x293642))
                .rounded_md()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            div()
                                .px_1p5()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(state_tone(state)))
                                .text_xs()
                                .text_color(rgb(state_tone(state)))
                                .child(crate::accessibility::named(
                                    format!("runner-state-{id}"),
                                    state_label(state),
                                )),
                        )
                        .child(crate::accessibility::named(
                            format!("runner-line-{id}"),
                            prerequisite_line(&item),
                        )),
                );
            if let Some(remedy) = item["remedy"].as_str() {
                row = row.child(named_hint(
                    format!("runner-remedy-{id}"),
                    format!("Fix: {remedy}"),
                ));
            }
            list = list.child(row);
        }
        panel = panel.child(list);
        let profiles = self.profiles_section(&view, cx);
        let editable = view["editable"] == true;
        let mut form = section("Runner configuration").child(hint(
            "Saved owner-only on this machine. The Mail token and provider keys are stored encrypted and never shown again.",
        ));
        if let Some(reason) = view["editReason"].as_str() {
            form = form.child(named_hint("runner-read-only", reason.to_owned()));
        }
        let locked = self.blocked() || !editable;
        let draft = self.settings.runner.form.as_ref().unwrap();
        for (key, label) in FIELDS {
            form = form.child(draft.fields.render("runner", key, label, locked));
            if key == "mailProjectRoot" {
                form = form
                    .child(crate::accessibility::text("Rust Agent Mail token"))
                    .child(named_hint(
                        "runner-mail-secret",
                        match view["secrets"]["mail"].as_str() {
                            Some("stored") => "A token is saved encrypted.",
                            Some("inline") => "The token is in the configuration file.",
                            Some("unreadable") => "The saved token is unreadable; enter it again.",
                            _ => "No token saved.",
                        },
                    ))
                    .child(
                        Input::new(&draft.mail)
                            .id("runner-mail-token")
                            .disabled(locked),
                    );
            }
        }
        let providers = view["secrets"]["providers"]
            .as_object()
            .map(|p| {
                p.iter()
                    .map(|(name, source)| format!("{name}: {}", source.as_str().unwrap_or("none")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        form = form
            .child(crate::accessibility::text("Advanced (JSON): catalog, policy, models.providers and other runner keys"))
            .child(
                Textarea::new(&draft.advanced)
                    .accessibility_id("runner-advanced")
                    .aria_label("Advanced runner settings (JSON)")
                    .h(px(220.))
                    .disabled(locked),
            )
            .child(hint("Provider keys are never shown here. Add a provider's apiKey in the JSON, or enter it below; it moves to the encrypted store."))
            .child(named_hint(
                "runner-provider-secrets",
                if providers.is_empty() {
                    "No OMP provider is defined.".to_owned()
                } else {
                    format!("Provider keys: {providers}")
                },
            ))
            .child(Input::new(&draft.provider).id("runner-provider-name").disabled(locked))
            .child(Input::new(&draft.provider_key).id("runner-provider-key").disabled(locked))
            .children(self.feedback(crate::workbench::feedback::Place::SaveRow))
            .child(
                div().flex().flex_wrap().gap_2().child(
                    Button::new("save-runner")
                        .primary()
                        .label("Save runner settings")
                        .disabled(locked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let Some((body, secrets)) = this.settings.runner.form.as_ref().map(|f| {
                                (f.body(cx), [f.mail.clone(), f.provider_key.clone()])
                            }) else {
                                return;
                            };
                            match body {
                                Ok(body) => {
                                    // Drop typed secrets from the form once sent.
                                    for input in secrets {
                                        input.update(cx, |s, cx| s.set_value("", window, cx));
                                    }
                                    this.send("/runner/settings".into(), body, false, cx);
                                }
                                Err(error) => {
                                    this.fail_at(error, feedback::Place::SaveRow);
                                    cx.notify();
                                }
                            }
                        })),
                ),
            );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(panel)
            .child(profiles)
            .child(form)
    }
    /// FOR-87: discovered offerings per configured harness. The operator
    /// approves each one explicitly with efforts from its advertised set.
    fn profiles_section(&self, view: &Value, cx: &mut Context<Self>) -> Div {
        let listing = &view["profiles"];
        let mut section = section("Execution profiles")
            .child(hint(listing["note"].as_str().unwrap_or(
            "Approving lets agents run on a model. Access is confirmed only when an agent runs.",
        )));
        let dirty = self.settings.runner.unsaved(cx);
        let locked = self.blocked() || view["editable"] != true || dirty;
        if let Some(reason) = view["editReason"].as_str() {
            section = section.child(named_hint("runner-profiles-read-only", reason.to_owned()));
        } else if dirty {
            section = section.child(named_hint(
                "runner-profiles-unsaved",
                "Save or undo the runner configuration changes below before approving models.",
            ));
        }
        if listing["discovery"] == "unavailable" {
            section = section.child(named_hint(
                "runner-profiles-discovery",
                format!(
                    "Model discovery is unavailable: {}",
                    listing["discoveryReason"].as_str().unwrap_or("")
                ),
            ));
        }
        for harness in records(listing, "harnesses") {
            let name = string(&harness, "harness");
            section = section.child(crate::accessibility::text(
                harness["label"].as_str().unwrap_or("").to_owned(),
            ));
            if let Some(note) = harness["note"].as_str() {
                section = section.child(named_hint(
                    format!("runner-profiles-note-{name}"),
                    note.to_owned(),
                ));
            }
            for (index, offering) in records(&harness, "offerings").into_iter().enumerate() {
                let id = format!("profile-{name}-{index}");
                let chosen = self.settings.runner.chosen(&offering);
                let mut row = div()
                    .id(SharedString::from(id.clone()))
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x293642))
                    .rounded_md()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(crate::accessibility::named(
                        format!("{id}-line"),
                        offering_line(&offering),
                    ));
                if let Some(reason) = offering["reason"].as_str() {
                    row = row.child(named_hint(
                        format!("{id}-reason"),
                        format!("Cannot approve: {reason}"),
                    ));
                }
                let approvable = offering["approvable"] == true;
                let mut efforts = div().flex().flex_wrap().gap_2();
                for effort in strings(&offering["advertisedEfforts"]) {
                    let picked = chosen.contains(&effort);
                    let target = offering.clone();
                    let toggled = effort.clone();
                    efforts = efforts.child(
                        Button::new(SharedString::from(format!("{id}-effort-{effort}")))
                            .label(if picked {
                                format!("Effort {effort} · chosen")
                            } else {
                                format!("Effort {effort}")
                            })
                            .selected(picked)
                            .disabled(locked || !approvable)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let page = &mut this.settings.runner;
                                let mut next = page.chosen(&target);
                                if let Some(at) = next.iter().position(|e| *e == toggled) {
                                    next.remove(at);
                                } else {
                                    next.push(toggled.clone());
                                }
                                page.efforts.insert(offering_key(&target), next);
                                cx.notify();
                            })),
                    );
                }
                let approved = offering["profile"].is_object();
                let target = offering.clone();
                efforts = efforts.child(
                    Button::new(SharedString::from(format!("{id}-approve")))
                        .primary()
                        .label(if approved {
                            "Update approval"
                        } else {
                            "Approve for agents"
                        })
                        .disabled(locked || !approvable || chosen.is_empty())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let efforts = this.settings.runner.chosen(&target);
                            this.send(
                                "/runner/profile".into(),
                                json!({"harness":target["harness"],"provider":target["provider"],
                                    "selector":target["selector"],"approve":true,"efforts":efforts}),
                                false,
                                cx,
                            )
                        })),
                );
                if approved {
                    efforts = efforts.child(remove_button(
                        &format!("{id}-remove"),
                        &offering,
                        locked,
                        cx,
                    ));
                }
                section = section.child(row.child(efforts));
            }
        }
        let unmatched = records(listing, "unmatched");
        if !unmatched.is_empty() {
            section = section.child(crate::accessibility::text(
                "Saved profiles that discovery did not list",
            ));
        }
        for (index, profile) in unmatched.into_iter().enumerate() {
            let id = format!("profile-unmatched-{index}");
            section = section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(crate::accessibility::named(
                        format!("{id}-line"),
                        format!(
                            "{} · {}/{} · efforts {}",
                            profile["harness"].as_str().unwrap_or("unknown harness"),
                            profile["provider"].as_str().unwrap_or(""),
                            profile["selector"].as_str().unwrap_or(""),
                            strings(&profile["efforts"]).join(", ")
                        ),
                    ))
                    .when(profile["harness"].is_string(), |row| {
                        row.child(remove_button(&format!("{id}-remove"), &profile, locked, cx))
                    }),
            );
        }
        section
    }
}
fn remove_button(id: &str, target: &Value, locked: bool, cx: &mut Context<Workbench>) -> Button {
    let target = target.clone();
    Button::new(SharedString::from(id.to_owned()))
        .label("Remove approval")
        .disabled(locked)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.send(
                "/runner/profile".into(),
                json!({"harness":target["harness"],"provider":target["provider"],
                    "selector":target["selector"],"approve":false}),
                false,
                cx,
            )
        }))
}
#[cfg(test)]
mod tests;
