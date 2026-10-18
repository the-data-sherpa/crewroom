//! Operator decisions use service revisions and durable request identities.
mod conversation;
mod insertion;
mod learning;
mod packages;
mod refinement;
use super::forms::Fields;
use super::*;
pub(super) use conversation::Send;
use gpui_kit::component::checkbox::Checkbox;
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct Collaboration {
    pub(in crate::workbench) forms: HashMap<String, Form>,
}
pub(in crate::workbench) struct Form {
    pub(in crate::workbench) fields: Fields,
    pub(in crate::workbench) original: Value,
    pub(in crate::workbench) edit: bool,
    pub(in crate::workbench) basis: Value,
    pub(in crate::workbench) rows: Vec<String>,
    pub(in crate::workbench) original_rows: Vec<String>,
    pub(in crate::workbench) role: String,
    choices: HashSet<String>,
}
impl Collaboration {
    pub(in crate::workbench) fn form(
        &mut self,
        key: &str,
        value: &Value,
        fields: &[(&'static str, bool)],
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        self.forms.entry(key.into()).or_insert_with(|| {
            let fields = Fields::new(value, fields, window, cx);
            Form {
                original: fields.values(cx),
                fields,
                edit: false,
                basis: value.clone(),
                rows: Vec::new(),
                original_rows: Vec::new(),
                role: String::new(),
                choices: HashSet::new(),
            }
        });
    }
    pub(super) fn saved(&mut self, path: &str) {
        self.forms.remove(path);
        if path.ends_with("/implementation-plans") || path.ends_with("/answers") {
            self.forms
                .retain(|key, _| !key.starts_with(&format!("{path}/")));
        }
    }
    pub(super) fn unsaved(&self, cx: &App) -> bool {
        self.forms.values().any(|f| {
            f.fields.values(cx) != f.original || !f.choices.is_empty() || f.rows != f.original_rows
        })
    }
}
impl Workbench {
    fn decided_draft(&self, path: &str, cx: &Context<Self>) -> Div {
        let Some(form) = self.collaboration.forms.get(path) else {
            return div();
        };
        if form.fields.values(cx) == form.original {
            return div();
        }
        let mut draft = section("Retained local draft")
            .child(hint("This proposal has a recorded decision. Your unsaved text remains below for copying or further editing."));
        for (key, label) in [
            ("brief", "Draft brief"),
            ("criteria", "Draft criteria"),
            ("reason", "Draft reason"),
        ] {
            if form.fields.inputs.contains_key(key) || form.fields.areas.contains_key(key) {
                draft = draft.child(form.fields.render(
                    &format!("retained-{path}"),
                    key,
                    label,
                    self.busy,
                ));
            }
        }
        let key = path.to_owned();
        draft.child(
            Button::new(SharedString::from(format!("discard-draft-{path}")))
                .label("Discard local draft")
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.collaboration.forms.remove(&key);
                    this.pending_keys.remove(&key);
                    cx.notify();
                })),
        )
    }
}
fn ended(run: &Value) -> bool {
    run["desiredState"] == "cancelled"
        || ["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""))
}
fn criteria(text: String) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn text_list(values: &[Value]) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Name criteria lines that differ between a run's captured revision and the accepted one.
pub(super) fn criteria_changes(captured: &[Value], accepted: &[Value]) -> Vec<String> {
    let lines = |values: &[Value]| {
        values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (captured, accepted) = (lines(captured), lines(accepted));
    let mut changes: Vec<_> = captured
        .iter()
        .filter(|c| !accepted.contains(c))
        .map(|c| format!("Removed criterion: {c}"))
        .collect();
    changes.extend(
        accepted
            .iter()
            .filter(|a| !captured.contains(a))
            .map(|a| format!("Added criterion: {a}")),
    );
    if changes.is_empty() && captured != accepted {
        changes.push("Criteria reordered.".into());
    }
    changes
}

#[cfg(test)]
mod tests;
