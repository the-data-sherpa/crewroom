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
    /// Reasons specific to this model; empty when nothing applies to it alone.
    detail: String,
    /// Route identities, searchable but not repeated in the row title.
    routes: String,
    blocked: bool,
    /// FOR-73: a harness group title row; never selectable or searched.
    header: bool,
}
impl Choice {
    fn header(title: &str) -> Self {
        Self {
            key: format!("group:{title}"),
            label: title.into(),
            detail: String::new(),
            routes: String::new(),
            blocked: true,
            header: true,
        }
    }
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
        if self.header {
            // Group titles frame the full list; a search lists matches only.
            return query.trim().is_empty();
        }
        format!(
            "{} {} {} {}",
            self.label, self.detail, self.routes, self.key
        )
        .to_lowercase()
        .contains(&query.to_lowercase())
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut body = div()
            .id(SharedString::from(format!("model-choice-{}", self.key)))
            .flex()
            .flex_col()
            .whitespace_normal()
            .min_w_0()
            .w_full()
            .child(if self.header {
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::accessibility::text(self.label.clone()))
            } else {
                div().child(crate::accessibility::text(self.label.clone()))
            });
        if !self.detail.is_empty() {
            body = body.child(hint(self.detail.clone()));
        }
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
/// Billing route as the service detected it; absent means not detected.
pub(in crate::workbench) fn billing_label(billing: &Value) -> &'static str {
    match billing.as_str() {
        Some("subscription") => "subscription",
        Some("api") => "API",
        _ => "unknown billing",
    }
}
fn route_label(route: &Value) -> String {
    format!(
        "{} / {} / {} · {}",
        string(route, "harness"),
        string(route, "provider"),
        string(route, "selector"),
        billing_label(&route["billing"])
    )
}
/// Catalog rows a saved selection names, resolved as the service resolves
/// it: an exact key, or a key saved before billing routes existed that names
/// harness, provider and model and matches each routed offering of that
/// model. One row means the selection resolves; several need a route choice.
fn saved_rows<'a>(selected: &str, rows: &'a [Value]) -> Vec<&'a Value> {
    if selected.is_empty() {
        return vec![];
    }
    if let Some(row) = rows.iter().find(|row| row["key"] == selected) {
        return vec![row];
    }
    rows.iter()
        .filter(|row| {
            row["billingRoute"]
                .as_str()
                .is_some_and(|route| row["key"] == format!("{selected}:{route}"))
        })
        .collect()
}
/// FOR-69 version basis the service reports for a harness: `verified` or
/// `compatible (probed <version>)`. None while the version may not launch.
pub(in crate::workbench) fn harness_compatibility(
    catalog: &Value,
    harness: &str,
) -> Option<String> {
    records(catalog, "harnesses")
        .iter()
        .find(|h| h["harness"] == harness)
        .and_then(|h| h["compatibility"].as_str())
        .map(String::from)
}
/// A pre-billing selection several billing routes offer: never pick one.
fn route_choice_required(rows: &[&Value]) -> String {
    format!(
        "{} is reachable through {} billing. Choose one billing route explicitly; your selection is retained.",
        string(rows[0], "displayName"),
        rows.iter()
            .map(|row| billing_label(&row["billingRoute"]))
            .collect::<Vec<_>>()
            .join(" and ")
    )
}
/// FOR-73 picker contents: rows grouped by harness, one line per model.
struct Listing {
    choices: Vec<Choice>,
    /// Reasons that apply to every listed model, each with its remedy.
    shared: Vec<String>,
    /// Legacy models in the catalog (shown only when expanded).
    legacy: usize,
    intent: bool,
}
const HARNESSES: [(&str, &str); 3] = [
    ("codex", "Codex"),
    ("claude-code", "Claude Code"),
    ("omp", "OMP"),
];
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}
fn efforts_label(value: &Value) -> String {
    let efforts = strings(value);
    if efforts.is_empty() {
        "efforts unknown".into()
    } else {
        efforts.join(", ")
    }
}
/// The remedy for a reason shared by every model, in operator words.
fn shared_remedy(reason: &str, catalog: &Value) -> String {
    let runner = &catalog["runner"];
    if runner["configured"] == false {
        return format!("Fix: {}", string(runner, "remedy"));
    }
    match reason {
        "Refresh this harness before admission." => "Fix: Refresh models.".into(),
        "Model access is unavailable or unverified." => {
            "Fix: Check each harness login in the runner configuration, then refresh models.".into()
        }
        "No verified adapter profile supports this offering." => {
            "Fix: Add verified model profiles to the runner configuration, then refresh models."
                .into()
        }
        _ => "Fix: Follow the harness fixes in Models & budget.".into(),
    }
}
/// Build the picker rows from the service catalog. Eligibility stays the
/// service's; this only arranges rows and removes reasons repeated on all.
fn listing(catalog: &Value, effort: &str, show_legacy: bool, keep: &[String]) -> Listing {
    let rows = records(catalog, "choices");
    let intent = catalog["selection"] == "intent";
    let blockers = |row: &Value| strings(&row["eligibility"][effort]["blockers"]);
    // Shared reasons: present on every runner-listed single model. Login rows
    // carry their own cause and equivalence groups repeat their members.
    let candidates: Vec<&Value> = rows
        .iter()
        .filter(|r| r["source"] != "omp-login" && r["billingRoute"].is_string())
        .collect();
    let mut shared: Vec<String> = Vec::new();
    if candidates.len() >= 2
        && candidates
            .iter()
            .all(|r| r["eligibility"][effort]["eligible"] != true)
    {
        shared = blockers(candidates[0]);
        for row in &candidates[1..] {
            let reasons = blockers(row);
            shared.retain(|r| reasons.contains(r));
        }
    }
    let mut legacy = 0;
    let mut choices = Vec::new();
    let row_choice = |row: &Value| -> Choice {
        let routes = records(row, "routes")
            .iter()
            .map(route_label)
            .collect::<Vec<_>>()
            .join(" → ");
        let eligibility = &row["eligibility"][effort];
        let ineligible = eligibility["eligible"] != true;
        let label = if row["billingRoute"].is_string() {
            let first = records(row, "routes")
                .into_iter()
                .next()
                .unwrap_or_default();
            let billing = billing_label(&row["billingRoute"]);
            let route = if row["source"] == "omp-login" {
                format!("{} login", string(&first, "provider"))
            } else if row["harness"] == "omp" {
                format!("{} · {billing}", string(&first, "provider"))
            } else {
                billing.to_owned()
            };
            let mut label = format!(
                "{} · {route} · {}",
                string(row, "displayName"),
                efforts_label(&row["efforts"])
            );
            if row["status"] == "legacy" {
                label.push_str(" · legacy");
            }
            label
        } else {
            format!("{} · {routes}", string(row, "displayName"))
        };
        let mut detail: Vec<String> = Vec::new();
        let unknown = strings(&row["unknownCapabilities"]);
        if !unknown.is_empty() {
            detail.push(format!(
                "Unknown: {} (not in Crewroom's reviewed model facts).",
                unknown.join(", ")
            ));
        }
        if ineligible {
            detail.extend(blockers(row).into_iter().filter(|r| !shared.contains(r)));
        }
        Choice {
            key: string(row, "key"),
            label,
            routes,
            detail: detail.join(" "),
            // Without a runner the service declares choices as saved intent:
            // selectable, still reported ineligible, re-checked at admission.
            blocked: ineligible && !intent,
            header: false,
        }
    };
    let needs_update: Vec<Value> = records(catalog, "harnesses")
        .iter()
        .flat_map(|h| records(h, "needsUpdate"))
        .collect();
    for (harness, title) in HARNESSES {
        let mut current = Vec::new();
        let mut older = Vec::new();
        for row in rows
            .iter()
            .filter(|r| r["harness"] == harness && r["billingRoute"].is_string())
        {
            let choice = row_choice(row);
            if row["status"] == "legacy" {
                legacy += 1;
                if show_legacy || keep.contains(&choice.key) {
                    older.push(choice);
                }
            } else {
                current.push(choice);
            }
        }
        // Models the installed harness predates: information only.
        for m in needs_update.iter().filter(|m| m["harness"] == harness) {
            let choice = Choice {
                key: format!(
                    "needs-update:{harness}:{}:{}",
                    string(m, "provider"),
                    string(m, "selector")
                ),
                label: format!(
                    "{} · needs {title} {} or newer · {}",
                    string(m, "displayName"),
                    string(m, "minimumHarnessVersion"),
                    efforts_label(&m["efforts"])
                ),
                detail: string(m, "reason"),
                routes: format!("{harness} / {}", string(m, "provider")),
                blocked: true,
                header: false,
            };
            if m["status"] == "legacy" {
                legacy += 1;
                if show_legacy {
                    older.push(choice);
                }
            } else {
                current.push(choice);
            }
        }
        if current.is_empty() && older.is_empty() {
            continue;
        }
        choices.push(Choice::header(title));
        choices.extend(current);
        choices.extend(older);
    }
    let groups: Vec<Choice> = rows
        .iter()
        .filter(|r| !r["billingRoute"].is_string())
        .map(row_choice)
        .collect();
    if !groups.is_empty() {
        choices.push(Choice::header("Equivalent models"));
        choices.extend(groups);
    }
    let shared = shared
        .into_iter()
        .map(|reason| {
            format!(
                "Applies to every listed model: {reason} {}",
                shared_remedy(&reason, catalog)
            )
        })
        .collect();
    Listing {
        choices,
        shared,
        legacy,
        intent,
    }
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
                routes: String::new(),
                blocked: false,
                header: false,
            });
        }
        let effective = if selected.is_empty() {
            inherit.as_deref().unwrap_or("")
        } else {
            &selected
        };
        let keep: Vec<String> = saved_rows(effective, &rows)
            .iter()
            .map(|row| string(row, "key"))
            .collect();
        let listing = listing(
            &self.workspace["modelDiscovery"],
            effort,
            self.settings.show_legacy_models,
            &keep,
        );
        choices.extend(listing.choices.iter().cloned());
        // A saved key resolving to exactly one row shows as that choice.
        let saved = saved_rows(&selected, &rows);
        let shown = match saved.as_slice() {
            [row] => string(row, "key"),
            _ => selected.clone(),
        };
        if saved.len() > 1 {
            choices.push(Choice {
                key: selected.clone(),
                routes: String::new(),
                label: format!(
                    "Choose a billing route: {}",
                    string(saved[0], "displayName")
                ),
                detail: route_choice_required(&saved),
                blocked: true,
                header: false,
            });
        } else if !selected.is_empty() && !choices.iter().any(|c| c.key == shown) {
            let catalog_selection =
                selected.starts_with("offering:") || selected.starts_with("equivalent:");
            choices.push(Choice {
                key:selected.clone(),
                routes:String::new(),
                label:format!("{}: {selected}", if catalog_selection {"Unavailable saved selection"} else {"Legacy selection"}),
                detail:if catalog_selection {"Not in this catalog. Refresh or restore its configuration; your selection is retained."} else {"Saved text kept as-is; it matches no listed model. It uses the saved legacy policy. Choose a listed model to use catalog validation."}.into(),
                blocked:true,
                header:false,
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
        if picker.choices != choices || picker.selected != shown || picker.width != menu_width {
            picker.state.update(cx, |state, cx| {
                state.set_items(
                    Choices::new(choices.clone(), menu_width, window, cx),
                    window,
                    cx,
                );
                state.set_selected_value(&shown, window, cx);
            });
            picker.choices = choices;
            picker.selected = shown.clone();
            picker.width = menu_width;
        }
        let mut field = div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .gap_2()
            .child(crate::accessibility::text(label));
        // FOR-73: reasons shared by every listed model appear once, with
        // their remedy, instead of on each row.
        if listing.intent {
            field = field.child(
                div()
                    .id(SharedString::from(format!("{id}-intent")))
                    .child(hint("Choices are saved as your intent and checked when the runner is configured.")),
            );
        }
        for (index, shared) in listing.shared.iter().enumerate() {
            field = field.child(
                div()
                    .id(SharedString::from(format!("{id}-shared-{index}")))
                    .child(hint(shared.clone())),
            );
        }
        field = field
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
        if listing.legacy > 0 {
            let showing = self.settings.show_legacy_models;
            field = field.child(
                Button::new(SharedString::from(format!("{id}-legacy")))
                    .label(if showing {
                        format!("Hide {} legacy models", listing.legacy)
                    } else {
                        format!("Show {} legacy models", listing.legacy)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.show_legacy_models = !this.settings.show_legacy_models;
                        cx.notify();
                    })),
            );
        }
        let resolved = saved_rows(effective, &rows);
        let effective_key = match resolved.as_slice() {
            [row] => string(row, "key"),
            _ => effective.to_owned(),
        };
        if let Some(choice) = picker.choices.iter().find(|c| c.key == effective_key) {
            let eligible = resolved
                .first()
                .is_some_and(|row| row["eligibility"][effort]["eligible"] == true);
            if !choice.detail.is_empty() {
                field = field.child(hint(choice.detail.clone()));
            } else if eligible {
                field = field.child(hint(format!("Eligible · effort {effort}")));
            }
        }
        if effective_key != effective {
            field = field.child(hint(format!(
                "Saved before billing routes: {effective}. It resolves to the only listed route; choose it again to record the route."
            )));
        }
        if let [row] = resolved.as_slice() {
            for route in records(row, "routes") {
                let caps = &route["capabilities"];
                field = field.child(hint(format!(
                    "{} · Version: {} · Access: {} · Tools: {} · Inputs: {} · Profile: {}",
                    route_label(&route),
                    harness_compatibility(
                        &self.workspace["modelDiscovery"],
                        &string(&route, "harness")
                    )
                    .unwrap_or_else(|| "not launchable".into()),
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
            // FOR-73: what runs differently on this harness version.
            for note in records(row, "degradedModes") {
                if let Some(note) = note.as_str() {
                    field = field.child(hint(note.to_owned()));
                }
            }
            if row["key"]
                .as_str()
                .is_some_and(|key| key.starts_with("equivalent:"))
            {
                field = field.child(hint("Uses the first eligible route in the displayed preference order. Capability and access differences remain separate."));
            }
        } else if resolved.len() > 1 {
            // A selected ambiguous key already shows this as its row detail.
            if selected.is_empty() {
                field = field.child(hint(route_choice_required(&resolved)));
            }
        } else if !effective.is_empty() {
            field = field.child(hint(format!("Retained selection: {effective}. Availability is checked by the service before admission.")));
        }
        field
    }
    /// Catalog-wide causes and remedies, in operator words. Empty when the
    /// catalog is available with a configured runner.
    pub(in crate::workbench::settings) fn catalog_status(&self) -> Vec<String> {
        let catalog = &self.workspace["modelDiscovery"];
        if !catalog.is_object() {
            return vec!["Model catalog unavailable: this service does not report model discovery. Fix: stop and restart the service with this Crewroom version.".into()];
        }
        let mut status = Vec::new();
        if catalog["state"] == "unavailable" {
            status.push(format!(
                "Model catalog unavailable: {} Fix: {}",
                string(catalog, "reason"),
                string(catalog, "remedy")
            ));
        }
        if catalog["runner"]["configured"] == false {
            status.push(format!(
                "Runner not configured: {} Fix: {}",
                string(&catalog["runner"], "reason"),
                string(&catalog["runner"], "remedy")
            ));
        }
        status
    }
    pub(in crate::workbench::settings) fn model_refresh(&self, cx: &mut Context<Self>) -> Div {
        let catalog = &self.workspace["modelDiscovery"];
        let refreshing = catalog["state"] == "refreshing";
        let available = catalog.is_object() && catalog["state"] != "unavailable";
        let mut panel = div().flex().flex_col().min_w_0().gap_2()
            .child(Button::new("refresh-model-catalog").label(if refreshing { "Refreshing models…" } else { "Refresh models" })
                .disabled(self.blocked() || refreshing || !available)
                .on_click(cx.listener(|this,_,_,cx| this.send("/model-catalog/refresh".into(),json!({}),false,cx))))
            .child(hint("Lists models without starting work. Refresh evidence lasts one hour. Configured access does not prove inference."));
        for (index, status) in self.catalog_status().into_iter().enumerate() {
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("models-catalog-status-{index}")))
                    .child(hint(status)),
            );
        }
        if !available {
            return panel;
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
            // Each harness row has its own identity so every row stays a
            // separate accessible label; hints inside use distinct call sites.
            let mut row = div()
                .id(SharedString::from(format!(
                    "model-harness-{}",
                    string(&harness, "harness")
                )))
                .flex()
                .flex_col()
                .min_w_0()
                .gap_1()
                .child(hint(format!(
                    "{} · {state}{} · Last success: {last} · {} models",
                    string(&harness, "harness"),
                    harness["compatibility"]
                        .as_str()
                        .map(|c| format!(" · {c}"))
                        .unwrap_or_default(),
                    records(&harness, "offerings").len()
                )));
            // A remedy for a missing executable replaces its generic error.
            if let Some(remedy) = harness["remedy"].as_str() {
                row = row.child(hint(remedy.to_owned()));
            } else if let Some(error) = harness["error"].as_str() {
                row = row.child(hint(error.to_owned()));
            }
            if let Some(note) = harness["note"].as_str() {
                row = row.child(hint(note.to_owned()));
            }
            if let Some(note) = harness["executionNote"].as_str() {
                row = row.child(hint(note.to_owned()));
            }
            // FOR-73: the version's capability record in operator words:
            // refusals first, then holds and degraded modes.
            for (index, note) in strings(&harness["refusals"])
                .into_iter()
                .chain(strings(&harness["degradedModes"]))
                .enumerate()
            {
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "model-capability-{}-{index}",
                            string(&harness, "harness")
                        )))
                        .child(hint(note)),
                );
            }
            let login = &harness["login"];
            if let Some(note) = login["note"].as_str().or(login["error"].as_str()) {
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "model-login-{}",
                            string(&harness, "harness")
                        )))
                        .child(hint(note.to_owned())),
                );
            }
            let updates = records(&harness, "needsUpdate").len();
            if updates > 0 {
                row = row.child(hint(format!(
                    "{updates} reviewed models need a newer {} release; the picker names the release.",
                    string(&harness, "harness")
                )));
            }
            // Detected from the login each harness is launched with; kinds
            // only, never credential values.
            for billing in records(&harness, "billing") {
                let detection = &billing["detection"];
                let routes = detection["routes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(billing_label)
                    .collect::<Vec<_>>()
                    .join(" and ");
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "model-billing-{}-{}",
                            string(&harness, "harness"),
                            string(&billing, "provider")
                        )))
                        .child(hint(format!(
                            "Billing ({}): {routes} · {}",
                            string(&billing, "provider"),
                            string(detection, "basis")
                        ))),
                );
            }
            panel = panel.child(row);
        }
        if let Some(error) = catalog["choiceError"].as_str() {
            panel = panel.child(hint(error.to_owned()));
        }
        if let Some(error) = catalog["persistenceError"].as_str() {
            panel = panel.child(hint(error.to_owned()));
        }
        if records(catalog, "choices").is_empty() {
            panel = panel.child(hint(
                "No models listed yet. Refresh models, or follow the harness fixes above.",
            ));
        }
        panel
    }
}
