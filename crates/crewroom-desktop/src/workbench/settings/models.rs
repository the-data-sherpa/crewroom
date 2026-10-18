use super::*;
pub(super) struct ModelDraft {
    pub fields: Fields,
    pub budget: String,
    revision: u64,
    original: Value,
}
impl ModelDraft {
    pub(super) fn new(value: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mut draft = Self {
            fields: Fields::new(
                &value["jobs"],
                &[
                    ("code", false),
                    ("judgment", false),
                    ("review", false),
                    ("fast", false),
                ],
                window,
                cx,
            ),
            budget: value["budget"].as_str().unwrap_or("large").into(),
            revision: value["revision"].as_u64().unwrap_or(0),
            original: Value::Null,
        };
        draft.original = draft.value(cx);
        draft
    }
    fn value(&self, cx: &App) -> Value {
        json!({"jobs":self.fields.values(cx),"budget":self.budget,"revision":self.revision})
    }
    pub(super) fn dirty(&self, cx: &App) -> bool {
        self.value(cx) != self.original
    }
}
impl Workbench {
    pub(in crate::workbench) fn models(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if self.settings.models.is_none() {
            self.settings.models = Some(ModelDraft::new(&self.workspace["modelJobs"], window, cx));
        }
        let draft = self.settings.models.as_ref().unwrap();
        let mut form = section("Models & budget").child(hint("Choose models for new assignments. Role overrides take precedence; existing assignments keep their model and effort."));
        if self.workspace["modelJobs"]["legacyDefault"] == true {
            form = form.child(hint(
                "Legacy defaults remain active until you save models and a budget.",
            ));
        }
        if self.workspace["capabilities"]["execution"] != true {
            form = form.child(hint("Runner unavailable. You can save selections; model availability cannot be verified until the runner is configured."));
        }
        if self.workspace["modelJobs"]["revision"]
            .as_u64()
            .unwrap_or(0)
            != draft.revision
        {
            form = form.child(hint(
                "Saved model jobs changed. Your draft is retained; reload before saving.",
            ));
        }
        let mut budgets = div().flex().flex_wrap().gap_2();
        for budget in crewroom_core::model_jobs::BUDGETS {
            budgets = budgets.child(
                Button::new(SharedString::from(format!("budget-{budget}")))
                    .label(budget)
                    .selected(draft.budget == budget)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.models.as_mut().unwrap().budget = budget.into();
                        cx.notify();
                    })),
            );
        }
        form = form.child(budgets).child(hint(
            "Budget sets reasoning effort, not a spending limit. Unsupported efforts block launch.",
        ));
        let budget = draft.budget.clone();
        let fields = draft.fields.inputs.clone();
        form = form.child(self.model_refresh(cx));
        for (job, label) in [
            ("code", "Code model"),
            ("judgment", "Judgment model"),
            ("review", "Review model"),
            ("fast", "Fast model"),
        ] {
            let effort = crewroom_core::model_jobs::effort(&budget, job).unwrap_or("unknown");
            let field = if self.workspace["modelDiscovery"].is_object() {
                self.model_picker(
                    &format!("job-{job}"),
                    label,
                    fields[job].clone(),
                    effort,
                    None,
                    window,
                    cx,
                )
            } else {
                self.settings
                    .models
                    .as_ref()
                    .unwrap()
                    .fields
                    .render("job", job, label, self.busy)
                    .child(hint(format!("Effort: {effort}")))
            };
            form = form.child(
                div()
                    .id(SharedString::from(format!("model-job-{job}")))
                    .min_w_0()
                    .child(field),
            );
        }
        form.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("save-model-jobs")
                        .primary()
                        .label("Save models & budget")
                        .disabled(self.blocked())
                        .on_click(cx.listener(|this, _, _, cx| {
                            let value = this.settings.models.as_ref().unwrap().value(cx);
                            this.send("/model-jobs".into(), value, false, cx);
                        })),
                )
                .child(
                    Button::new("reload-model-jobs")
                        .label("Reload saved selections…")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.reload_setting("models", String::new(), window, cx);
                        })),
                ),
        )
    }
}
