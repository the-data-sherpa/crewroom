use super::*;
pub(super) struct PlaybookDraft {
    pub(super) fields: Fields,
    revision: u64,
    original: Value,
}
impl PlaybookDraft {
    fn new(value: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mut input = value.clone();
        input["steps"] = json!(
            records(value, "steps")
                .iter()
                .map(|s| string(s, "title"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let mut draft = Self {
            fields: Fields::new(
                &input,
                &[
                    ("id", false),
                    ("name", false),
                    ("description", true),
                    ("steps", true),
                ],
                window,
                cx,
            ),
            revision: value["revision"].as_u64().unwrap_or(0),
            original: Value::Null,
        };
        draft.original = draft.value(cx);
        draft
    }
    pub(super) fn value(&self, cx: &App) -> Value {
        let mut v = self.fields.values(cx);
        v["revision"] = json!(self.revision);
        v["steps"] = json!(
            self.fields
                .text("steps", cx)
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .enumerate()
                .map(|(i, s)| json!({"id":format!("step-{}",i+1),"title":s}))
                .collect::<Vec<_>>()
        );
        v
    }
    pub(super) fn dirty(&self, cx: &App) -> bool {
        self.value(cx) != self.original
    }
}
impl Workbench {
    pub(super) fn saved_playbook(
        &mut self,
        v: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = string(v, "id");
        self.settings.playbook = id.clone();
        self.settings
            .playbooks
            .insert(id, PlaybookDraft::new(v, window, cx));
        let mut rows = records(&self.workspace, "playbooks");
        if let Some(r) = rows.iter_mut().find(|r| r["id"] == v["id"]) {
            *r = v.clone();
        } else {
            rows.push(v.clone());
        }
        self.workspace["playbooks"] = json!(rows);
        self.notice = "Playbook saved. Run checklists keep their copied version.".into();
    }
    pub(in crate::workbench) fn playbooks(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let catalog = records(&self.workspace, "playbooks");
        let mut panel=section("Playbooks").child(hint("Reusable checklists for delivery and research. A run keeps a copy. Checklist progress never establishes readiness or replaces required checks and review."))
            .child(Button::new("new-playbook").label("Create playbook").disabled(self.busy).on_click(cx.listener(|this,_,window,cx|{let id=format!("playbook-{}",uuid::Uuid::new_v4());this.settings.playbooks.insert(id.clone(),PlaybookDraft::new(&json!({"id":id,"revision":0}),window,cx));this.settings.playbook=id;cx.notify();})));
        let drafts = self
            .settings
            .playbooks
            .iter()
            .filter(|(key, d)| d.revision == 0 && !catalog.iter().any(|p| p["id"] == **key))
            .map(|(key, d)| (d.fields.text("name", cx), key.clone()))
            .collect();
        panel = panel.child(self.unsaved_drafts("playbook", drafts, &self.settings.playbook, cx));
        if catalog.is_empty() {
            panel=panel.child(hint("No playbooks yet. Create a checklist to select before queueing or propose through the coordinator."));
        }
        for p in &catalog {
            let id = string(p, "id");
            let key = id.clone();
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("playbook-{id}")))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(crate::accessibility::text(format!(
                        "{} · revision {} · {} steps",
                        string(p, "name"),
                        p["revision"],
                        records(p, "steps").len()
                    )))
                    .child(hint(string(p, "description")))
                    .child(
                        Button::new(SharedString::from(format!("edit-playbook-{id}")))
                            .label("Edit playbook")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.playbook = key.clone();
                                cx.notify();
                            })),
                    ),
            );
        }
        let id = self.settings.playbook.clone();
        if id.is_empty() {
            return panel;
        }
        if !self.settings.playbooks.contains_key(&id) {
            if let Some(v) = catalog.iter().find(|p| p["id"] == id) {
                self.settings
                    .playbooks
                    .insert(id.clone(), PlaybookDraft::new(v, window, cx));
            } else {
                return panel;
            }
        }
        let draft = &self.settings.playbooks[&id];
        let mut form = section("Playbook editor").child(hint(format!(
            "Revision {}{}",
            draft.revision,
            if draft.dirty(cx) {
                " · Unsaved changes"
            } else {
                ""
            }
        )));
        if catalog
            .iter()
            .any(|p| p["id"] == id && p["revision"] != draft.revision)
        {
            form = form.child(hint(
                "Saved playbook changed. Your draft is retained; reload before saving.",
            ));
        }
        for (key, label) in [
            ("name", "Playbook name"),
            ("description", "Use when"),
            ("steps", "Steps · one per line"),
        ] {
            form = form.child(draft.fields.render("playbook", key, label, self.busy));
        }
        form = form.child(self.comparison_entry(
            "playbook",
            "playbook",
            &id,
            &draft.value(cx),
            draft.dirty(cx),
            None,
            cx,
        ));
        let target = id.clone();
        panel.child(
            form.child(
                Button::new("save-playbook")
                    .label("Save playbook")
                    .disabled(self.blocked())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let body = this.settings.playbooks[&this.settings.playbook].value(cx);
                        this.send("/playbooks".into(), body, false, cx);
                    })),
            )
            .child(
                Button::new("reload-playbook")
                    .label("Reload saved playbook")
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.reload_setting("playbook", target.clone(), w, cx)
                    })),
            ),
        )
    }
}
