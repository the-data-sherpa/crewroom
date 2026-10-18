use super::*;
pub(super) struct PrincipleDraft {
    pub(super) fields: Fields,
    revision: u64,
    original: Value,
}
impl PrincipleDraft {
    fn new(value: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mut draft = Self {
            fields: Fields::new(
                value,
                &[("id", false), ("name", false), ("text", true)],
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
        let mut value = self.fields.values(cx);
        value["revision"] = json!(self.revision);
        value
    }
    pub(super) fn dirty(&self, cx: &App) -> bool {
        self.value(cx) != self.original
    }
}
impl Workbench {
    pub(super) fn saved_principle(
        &mut self,
        value: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = string(value, "id");
        self.settings.principle = id.clone();
        self.settings
            .principles
            .insert(id, PrincipleDraft::new(value, window, cx));
        let mut rows = records(&self.workspace, "principles");
        if let Some(row) = rows.iter_mut().find(|r| r["id"] == value["id"]) {
            *row = value.clone();
        } else {
            rows.push(value.clone());
        }
        self.workspace["principles"] = json!(rows);
        self.notice = "Principle saved. Existing assignments keep their copied text.".into();
    }
    pub(in crate::workbench) fn principles(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let catalog = records(&self.workspace, "principles");
        let mut page=section("Principles").child(hint("Reusable guidance in your user configuration. Attach principles to roles; each new assignment keeps a copy of their text and revision."))
            .child(Button::new("new-principle").primary().label("Create principle").disabled(self.busy).on_click(cx.listener(|this,_,window,cx| {let id=format!("principle-{}",uuid::Uuid::new_v4());this.settings.principles.insert(id.clone(),PrincipleDraft::new(&json!({"id":id,"name":"","text":"","revision":0}),window,cx));this.settings.principle=id;cx.notify();})));
        let drafts = self
            .settings
            .principles
            .iter()
            .filter(|(key, d)| d.revision == 0 && !catalog.iter().any(|p| p["id"] == **key))
            .map(|(key, d)| (d.fields.text("name", cx), key.clone()))
            .collect();
        page = page.child(self.unsaved_drafts("principle", drafts, &self.settings.principle, cx));
        if catalog.is_empty() {
            page = page.child(hint(
                "No principles yet. Create guidance you want to reuse across roles.",
            ));
        }
        for p in &catalog {
            let id = string(p, "id");
            let key = id.clone();
            let uses = records(&self.workspace, "roles")
                .iter()
                .filter(|r| {
                    r["principles"]
                        .as_array()
                        .is_some_and(|a| a.contains(&p["id"]))
                })
                .count();
            page = page.child(
                div()
                    .id(SharedString::from(format!("principle-card-{id}")))
                    .p_3()
                    .border_1()
                    .border_color(rgb(0x293642))
                    .rounded_md()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(crate::accessibility::text(format!(
                        "{} · revision {} · {uses} roles",
                        string(p, "name"),
                        p["revision"]
                    )))
                    .child(hint(string(p, "text")))
                    .child(
                        Button::new(SharedString::from(format!("edit-principle-{id}")))
                            .label("Edit principle")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.principle = key.clone();
                                cx.notify();
                            })),
                    ),
            );
        }
        let id = self.settings.principle.clone();
        if id.is_empty() {
            return page;
        }
        let current = catalog.iter().find(|p| p["id"] == id);
        if !self.settings.principles.contains_key(&id) {
            if let Some(value) = current {
                self.settings
                    .principles
                    .insert(id.clone(), PrincipleDraft::new(value, window, cx));
            } else {
                self.settings.principle.clear();
                return page;
            }
        }
        let draft = &self.settings.principles[&id];
        let mut form = section("Principle editor").child(hint(format!(
            "{} · revision {}{}",
            id,
            draft.revision,
            if draft.dirty(cx) {
                " · Unsaved changes"
            } else {
                ""
            }
        )));
        if current.is_some_and(|p| p["revision"] != draft.revision) {
            form = form.child(hint(
                "Saved principle changed. Your draft is retained; reload before saving.",
            ));
        }
        form = form
            .child(
                draft
                    .fields
                    .render("principle", "name", "Principle name", self.busy),
            )
            .child(
                draft
                    .fields
                    .render("principle", "text", "Guidance", self.busy),
            );
        form = form.child(self.comparison_entry(
            "principle",
            "principle",
            &id,
            &draft.value(cx),
            draft.dirty(cx),
            None,
            cx,
        ));
        let key = id.clone();
        form = form
            .children(self.feedback(crate::workbench::feedback::Place::SaveRow))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("save-principle")
                            .primary()
                            .label("Save principle")
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let value =
                                    this.settings.principles[&this.settings.principle].value(cx);
                                this.send("/principles".into(), value, false, cx);
                            })),
                    )
                    .child(
                        Button::new("reload-principle")
                            .label("Reload saved principle…")
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.reload_setting("principle", key.clone(), w, cx)
                            })),
                    )
                    .child(
                        Button::new("close-principle")
                            .label("Close · keep draft")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.principle.clear();
                                cx.notify();
                            })),
                    ),
            );
        if let Some(current) = current {
            let revision = current["revision"].as_u64().unwrap();
            let key = id.clone();
            form=form.child(hint("Deletion is refused while any role references this principle. Saved assignment copies are retained."))
                .child(Button::new("delete-principle").label("Delete principle…").disabled(self.blocked()).on_click(cx.listener(move|_,_,window,cx|{let view=cx.entity().downgrade();let key=key.clone();window.open_alert_dialog(cx,move|dialog,_,_|{let view=view.clone();let key=key.clone();dialog.title(crate::accessibility::text("Delete this principle?")).description(crate::accessibility::text("Detach it from all roles first. Existing assignments retain their copies.")).show_cancel(true).ok_text("Delete principle").on_ok(move|_,_,cx|{let _=view.update(cx,|this,cx|this.send(format!("/principles/{key}/delete"),json!({"revision":revision}),true,cx));true})});})));
        }
        page.child(form)
    }
}
