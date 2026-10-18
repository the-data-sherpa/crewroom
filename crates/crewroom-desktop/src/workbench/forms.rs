//! Shared native text fields, with independent entity-backed drafts.
use super::*;

pub(super) struct Fields {
    pub(super) inputs: HashMap<&'static str, Entity<InputState>>,
    pub(super) areas: HashMap<&'static str, Entity<TextareaState>>,
}
impl Fields {
    pub(super) fn new(
        value: &Value,
        keys: &[(&'static str, bool)],
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) -> Self {
        let mut fields = Self {
            inputs: HashMap::new(),
            areas: HashMap::new(),
        };
        for &(key, multiline) in keys {
            let text = match &value[key] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if multiline {
                fields.areas.insert(
                    key,
                    cx.new(|cx| TextareaState::new(window, cx).default_value(text)),
                );
            } else {
                fields.inputs.insert(
                    key,
                    cx.new(|cx| InputState::new(window, cx).default_value(text)),
                );
            }
        }
        fields
    }
    pub(super) fn text(&self, key: &str, cx: &App) -> String {
        if let Some(input) = self.inputs.get(key) {
            input.read(cx).value().to_string()
        } else {
            self.areas[key].read(cx).value().to_string()
        }
    }
    pub(super) fn render(&self, prefix: &str, key: &str, label: &'static str, busy: bool) -> Div {
        let id = SharedString::from(format!("{prefix}-{key}"));
        let field = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(crate::accessibility::readable(
                format!("{prefix}-{key}-label"),
                label,
            ));
        if let Some(input) = self.inputs.get(key) {
            field.child(Input::new(input).id(id).aria_label(label).disabled(busy))
        } else {
            field.child(
                Textarea::new(&self.areas[key])
                    .accessibility_id(id)
                    .aria_label(label)
                    .h(px(if key == "instructions" { 180. } else { 90. }))
                    .disabled(busy),
            )
        }
    }
    pub(super) fn values(&self, cx: &App) -> Value {
        let mut value = json!({});
        for key in self.inputs.keys().chain(self.areas.keys()) {
            value[*key] = json!(self.text(key, cx));
        }
        value
    }
}
