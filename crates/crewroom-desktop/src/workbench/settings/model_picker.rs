//! Catalog-backed controls. Eligibility belongs to the service; inputs retain
//! saved intent even when an offering disappears or a refresh fails.
use super::*;
use gpui_kit::component::{
    IndexPath,
    searchable_list::SearchableListChange,
    select::{SearchableVec, Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
};

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq)]
struct Choice {
    key: String,
    label: String,
    detail: String,
    blocked: bool,
}
impl SelectItem for Choice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone().into()
    }
    fn value(&self) -> &String {
        &self.key
    }
    fn disabled(&self) -> bool {
        self.blocked
    }
    fn matches(&self, query: &str) -> bool {
        format!("{} {} {}", self.label, self.detail, self.key)
            .to_lowercase()
            .contains(&query.to_lowercase())
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let body = div()
            .id(SharedString::from(format!("model-choice-{}", self.key)))
            .flex()
            .flex_col()
            .whitespace_normal()
            .min_w_0()
            .w_full()
            .child(crate::accessibility::text(self.label.clone()))
            .child(hint(self.detail.clone()));
        #[cfg(test)]
        {
            use gpui_kit::test::TestSupportExt;
            body.test_support()
        }
        #[cfg(not(test))]
        body
    }
}
pub(super) struct Picker {
    input: Entity<InputState>,
    state: Entity<SelectState<Choices>>,
    choices: Vec<Choice>,
    selected: String,
    width: f32,
}
struct Choices {
    items: SearchableVec<Choice>,
    width: f32,
    height: gpui_kit::Pixels,
}
impl Choices {
    fn new(items: Vec<Choice>, width: f32, window: &mut Window, cx: &mut App) -> Self {
        use gpui_kit::{AvailableSpace, size};
        // Select virtualizes equal-height rows. Measure the longest wrapped row
        // at the actual menu width, including space for padding and the check.
        let width = (width - 56.).max(120.);
        let height = items
            .iter()
            .map(|item| {
                div()
                    .text_base()
                    .w(px(width))
                    .child(item.render(window, cx))
                    .into_any_element()
                    .layout_as_root(
                        size(
                            AvailableSpace::Definite(px(width)),
                            AvailableSpace::MinContent,
                        ),
                        window,
                        cx,
                    )
                    .height
            })
            .max()
            .unwrap_or(px(40.));
        Self {
            items: SearchableVec::new(items),
            width,
            height,
        }
    }
}
impl SelectDelegate for Choices {
    type Item = Choice;
    fn items_count(&self, section: usize) -> usize {
        self.items.items_count(section)
    }
    fn item(&self, index: IndexPath) -> Option<&Choice> {
        self.items.item(index)
    }
    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SelectItem<Value = V>,
        V: PartialEq,
    {
        self.items.position(value)
    }
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_kit::Task<()> {
        self.items.perform_search(query, window, cx)
    }
    fn on_will_change(
        &mut self,
        selection: &mut Vec<(IndexPath, Choice)>,
        changes: &[SearchableListChange],
    ) {
        // Keyboard confirmation can reach disabled rows. Veto the complete
        // replacement, including its deselection, so saved intent is retained.
        if changes.iter().any(|change| match change {
            SearchableListChange::Select { index } => {
                self.item(*index).is_none_or(|item| item.blocked)
            }
            SearchableListChange::Deselect { .. } => false,
        }) {
            return;
        }
        self.items.on_will_change(selection, changes);
    }
    fn render_item(
        &self,
        _: IndexPath,
        item: &Choice,
        checked: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui_kit::AnyElement> {
        // The stock row truncates all descendants. Keep complete blockers and
        // long model names readable, including in a narrow searchable menu.
        Some(
            div()
                .flex()
                .min_w_0()
                .w(px(self.width))
                .h(self.height)
                .gap_2()
                .child(div().flex_1().min_w_0().child(item.render(window, cx)))
                .child(if checked { "✓" } else { "" })
                .into_any_element(),
        )
    }
}
fn route_label(route: &Value) -> String {
    format!(
        "{} / {} / {}",
        string(route, "harness"),
        string(route, "provider"),
        string(route, "selector")
    )
}
impl Workbench {
    #[allow(clippy::too_many_arguments)] // Shared editor fields plus GPUI window/context.
    pub(in crate::workbench::settings) fn model_picker(
        &mut self,
        id: &str,
        label: &'static str,
        input: Entity<InputState>,
        effort: &str,
        inherit: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = input.read(cx).value().to_string();
        let menu_width = (f32::from(window.viewport_size().width) - 64.).clamp(240., 680.);
        let rows = records(&self.workspace["modelDiscovery"], "choices");
        let mut choices = Vec::new();
        if let Some(inherited) = &inherit {
            choices.push(Choice {
                key: String::new(),
                label: "Use job model".into(),
                detail: format!("Inherited selection: {inherited}"),
                blocked: false,
            });
        }
        for row in &rows {
            let routes = records(row, "routes")
                .iter()
                .map(route_label)
                .collect::<Vec<_>>()
                .join(" → ");
            let eligibility = &row["eligibility"][effort];
            let blocked = eligibility["eligible"] != true;
            let reasons = eligibility["reasons"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            choices.push(Choice {
                key: string(row, "key"),
                label: format!("{} · {routes}", string(row, "displayName")),
                detail: if blocked {
                    format!("Unavailable · {reasons}")
                } else {
                    format!("Eligible · effort {effort}")
                },
                blocked,
            });
        }
        if !selected.is_empty() && !choices.iter().any(|c| c.key == selected) {
            let catalog_selection =
                selected.starts_with("offering:") || selected.starts_with("equivalent:");
            choices.push(Choice {
                key:selected.clone(),
                label:format!("{}: {selected}", if catalog_selection {"Unavailable saved selection"} else {"Legacy selection"}),
                detail:if catalog_selection {"Not in this catalog. Refresh or restore its configuration; your selection is retained."} else {"Uses the saved legacy policy. Choose a listed model to use catalog validation."}.into(),
                blocked:true,
            });
        }
        let changed_input = self
            .settings
            .pickers
            .get(id)
            .is_none_or(|p| p.input != input);
        if changed_input {
            let state = cx.new(|cx| {
                SelectState::new(
                    Choices::new(choices.clone(), menu_width, window, cx),
                    None,
                    window,
                    cx,
                )
                .searchable(true)
            });
            let target = input.clone();
            cx.subscribe_in(&state, window, move |_, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    target.update(cx, |field, cx| field.set_value(value.clone(), window, cx));
                    cx.notify();
                }
            })
            .detach();
            self.settings.pickers.insert(
                id.into(),
                Picker {
                    input,
                    state,
                    choices: vec![],
                    selected: String::new(),
                    width: 0.,
                },
            );
        }
        let picker = self.settings.pickers.get_mut(id).unwrap();
        if picker.choices != choices || picker.selected != selected || picker.width != menu_width {
            picker.state.update(cx, |state, cx| {
                state.set_items(
                    Choices::new(choices.clone(), menu_width, window, cx),
                    window,
                    cx,
                );
                state.set_selected_value(&selected, window, cx);
            });
            picker.choices = choices;
            picker.selected = selected.clone();
            picker.width = menu_width;
        }
        let mut field = div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .gap_2()
            .child(crate::accessibility::text(label))
            .child(
                Select::new(&picker.state)
                    .id(SharedString::from(id.to_owned()))
                    .accessibility_label(label)
                    .placeholder("Choose a model")
                    .search_placeholder("Search models or routes")
                    .menu_width(px(menu_width))
                    .menu_max_h(px(320.))
                    .w_full()
                    .min_w_0()
                    .disabled(self.busy),
            )
            .child(hint(format!("Effective effort: {effort}")));
        let effective = if selected.is_empty() {
            inherit.as_deref().unwrap_or("")
        } else {
            &selected
        };
        if let Some(choice) = picker.choices.iter().find(|c| c.key == effective) {
            field = field.child(hint(choice.detail.clone()));
        }
        if let Some(row) = rows.iter().find(|row| row["key"] == effective) {
            for route in records(row, "routes") {
                let caps = &route["capabilities"];
                field = field.child(hint(format!(
                    "{} · Access: {} · Tools: {} · Inputs: {} · Profile: {}",
                    route_label(&route),
                    string(&route, "access"),
                    caps["tools"]
                        .as_bool()
                        .map(|v| if v { "supported" } else { "unsupported" })
                        .unwrap_or("unknown"),
                    caps["inputs"]
                        .as_array()
                        .map(|v| v
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or("unknown".into()),
                    route["executionProfile"].as_str().unwrap_or("unverified")
                )));
            }
            if row["key"]
                .as_str()
                .is_some_and(|key| key.starts_with("equivalent:"))
            {
                field = field.child(hint("Uses the first eligible route in the displayed preference order. Capability and access differences remain separate."));
            }
        } else if !effective.is_empty() {
            field = field.child(hint(format!("Retained selection: {effective}. Availability is checked by the service before admission.")));
        }
        field
    }
    pub(in crate::workbench::settings) fn model_refresh(&self, cx: &mut Context<Self>) -> Div {
        let catalog = &self.workspace["modelDiscovery"];
        let refreshing = catalog["state"] == "refreshing";
        let mut panel = div().flex().flex_col().min_w_0().gap_2()
            .child(Button::new("refresh-model-catalog").label(if refreshing { "Refreshing models…" } else { "Refresh models" })
                .disabled(self.blocked() || refreshing || !catalog.is_object())
                .on_click(cx.listener(|this,_,_,cx| this.send("/model-catalog/refresh".into(),json!({}),false,cx))))
            .child(hint("Lists models without starting work. Refresh evidence lasts one hour. Configured access does not prove inference."));
        if !catalog.is_object() {
            return panel.child(hint(
                "Model discovery is unavailable. Configure a runner to browse models.",
            ));
        }
        for harness in records(catalog, "harnesses") {
            let last = harness["lastSuccess"]
                .as_i64()
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| t.to_rfc3339())
                .unwrap_or("never".into());
            let state = if harness["configured"] == false {
                "unavailable"
            } else if harness["outcome"] == "success" && harness["fresh"] != true {
                "stale"
            } else {
                harness["outcome"].as_str().unwrap_or("unavailable")
            };
            panel = panel.child(hint(format!(
                "{} · {state} · Last success: {last} · {} models",
                string(&harness, "harness"),
                records(&harness, "offerings").len()
            )));
            if let Some(error) = harness["error"].as_str() {
                panel = panel.child(hint(error.to_owned()));
            }
        }
        for key in ["choiceError", "persistenceError"] {
            if let Some(error) = catalog[key].as_str() {
                panel = panel.child(hint(error.to_owned()));
            }
        }
        if records(catalog, "choices").is_empty() {
            panel=panel.child(hint("No selectable models yet. Refresh the catalog or check the unavailable harness configuration."));
        }
        panel
    }
}
