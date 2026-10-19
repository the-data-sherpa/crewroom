use super::*;

/// Read both old assignment shapes and additive catalog selections without
/// resolving either through today's settings.
pub(super) fn details(model: &Value) -> Div {
    let route = &model["resolved"];
    let selected = &model["selection"];
    let mut panel = div().flex().flex_col().min_w_0().gap_1();
    if !route.is_object() {
        return panel.child(hint("Model preview is unavailable."));
    }
    let label = selected["offering"]["displayName"]
        .as_str()
        .or(route["selector"].as_str())
        .unwrap_or("unavailable");
    panel = panel
        .child(hint(format!(
            "Model: {label} · effort {}",
            route["effort"].as_str().unwrap_or("unknown")
        )))
        .child(hint(format!(
            "Route: {} / {} / {}",
            route["adapter"].as_str().unwrap_or("omp"),
            string(route, "provider"),
            string(route, "id")
        )));
    if selected.is_object() {
        panel = panel
            .child(hint(format!(
                "Selection: {} · {} · Effort: {}",
                string(selected, "intendedModel"),
                string(selected, "modelSource").replace('_', " "),
                string(selected, "effortSource").replace('_', " ")
            )))
            .child(hint(format!(
                "Billing: {}",
                if selected["offering"]["billing"].is_string() {
                    super::settings::billing_label(&selected["offering"]["billing"])
                } else {
                    "not recorded (selected before billing routes were detected)"
                }
            )))
            .child(hint(string(selected, "reason")))
            .child(hint(format!(
                "Executable: {} · {} · Profile: {}",
                string(&selected["identity"], "binary"),
                string(&selected["identity"], "version"),
                string(&selected["offering"], "executionProfile")
            )))
            .child(hint(format!(
                "Catalog revision: {} · Policy: {}",
                selected["catalogRevision"],
                string(selected, "policyRevision")
            )))
            .child(hint(format!(
                "Requested selector: {} · Observed model: {} ({})",
                string(&selected["offering"], "selector"),
                selected["offering"]["observedModel"]["model"]
                    .as_str()
                    .unwrap_or("not observed"),
                selected["offering"]["observedModel"]["source"]
                    .as_str()
                    .unwrap_or("no discovery observation")
            )));
    }
    panel
}
impl Workbench {
    pub(super) fn role_model_preview(&self, role: &str) -> Div {
        let facts = &self.workspace["roleCatalog"][role];
        if let Some(error) = facts["modelError"].as_str() {
            return div().child(hint(format!("Model unavailable: {error}")));
        }
        details(&facts["model"])
    }
    /// FOR-79: where the project's settings change the role, admission
    /// verifies the project's preview, so capture that one.
    pub(super) fn capture_model_preview(&self, input: &mut Value, role: &str, project: &str) {
        let facts = &self.workspace["roleCatalog"][role];
        let model = match facts["projects"].get(project) {
            Some(scoped) => &scoped["model"],
            None => &facts["model"],
        };
        let preview = &model["selection"]["previewRevision"];
        input.as_object_mut().unwrap().remove("modelPreview");
        if preview.is_string() {
            input["modelPreview"] = preview.clone();
        }
    }
}
