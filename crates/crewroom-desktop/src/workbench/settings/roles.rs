use super::*;

/// Creation defaults for a new role; discarding a never-saved role restores them.
fn new_role() -> Value {
    json!({"id":format!("specialist-{}", uuid::Uuid::new_v4()),"name":"","category":"Quality","purpose":"","instructions":"","job":"judgment","model":"","effort":"default","enabled":true})
}

impl Workbench {
    fn new_role_draft(
        &mut self,
        source: Option<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = format!("{NEW}-{}", uuid::Uuid::new_v4());
        let mut value = source.unwrap_or_else(new_role);
        if !string(&value, "name").is_empty() {
            value["name"] = json!(format!("{} copy", string(&value, "name")));
        }
        value["id"] = json!(format!("specialist-{}", uuid::Uuid::new_v4()));
        value.as_object_mut().unwrap().remove("draftProposalId");
        value.as_object_mut().unwrap().remove("revision");
        value.as_object_mut().unwrap().remove("learningProposalId");
        let draft = RoleDraft::new(&value, window, cx);
        self.settings.roles.insert(key.clone(), draft);
        self.settings.role = key;
        self.settings.role_open = true;
        cx.notify();
    }

    pub(in crate::workbench) fn roles(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let catalog = records(&self.workspace, "roles");
        let mut page = section("Agent roles")
            .child(hint("Reusable specialties in your user configuration. Pick when explains which work each role suits. Changes apply to new assignments only."))
            .child(hint(format!("{} agent roles · {} enabled", catalog.len(), catalog.iter().filter(|r| r["enabled"]==true).count())))
            .child(Button::new("new-role").primary().label("Create agent role").disabled(self.busy)
                .on_click(cx.listener(|this, _, window, cx| this.new_role_draft(None, window, cx))));
        page = page.child(self.specialist_drafting(window, cx));
        let drafts = self
            .settings
            .roles
            .iter()
            .filter(|(key, _)| key.starts_with(NEW))
            .map(|(key, d)| (d.fields.text("name", cx), key.clone()))
            .collect();
        let open = if self.settings.role_open {
            self.settings.role.clone()
        } else {
            String::new()
        };
        page = page.child(self.unsaved_drafts("role", drafts, &open, cx));
        if self.selected.is_some() {
            page = page.child(
                Button::new("roles-back-to-ticket")
                    .label("Back to ticket")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Panel::Ticket;
                        cx.notify();
                    })),
            );
        }
        let mut groups: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
        for role in catalog {
            groups
                .entry(string(&role, "category"))
                .or_default()
                .push(role);
        }
        for (category, roles) in groups {
            let mut cards = div().flex().flex_wrap().gap_3();
            for role in roles {
                let id = string(&role, "id");
                let edit = id.clone();
                let duplicate = role.clone();
                let info = &self.workspace["roleCatalog"][&id];
                let model = &info["model"]["resolved"];
                let model_label = if model["selector"].is_string() {
                    format!(
                        "{} · {}",
                        string(model, "selector"),
                        string(model, "effort")
                    )
                } else {
                    info["modelError"]
                        .as_str()
                        .unwrap_or("Model resolution unavailable")
                        .into()
                };
                let lock = string(info, "lockReason");
                let status = if !lock.is_empty() {
                    format!("Core role · {lock}")
                } else if role["enabled"] == true {
                    "Enabled".into()
                } else {
                    "Disabled for new assignments".into()
                };
                cards = cards.child(
                    div()
                        .id(SharedString::from(format!("role-summary-{id}")))
                        .flex_1()
                        .min_w(px(240.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(0x293642))
                        .bg(rgb(0x101b24))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(crate::accessibility::text(string(&role, "name"))),
                        )
                        .child(hint(format!("Pick when: {}", string(&role, "purpose"))))
                        .child(hint(format!("{} job · {model_label}", string(info, "job"))))
                        .when(info["warning"].is_string(), |el| {
                            el.child(hint(string(info, "warning")))
                        })
                        .child(hint(status))
                        .child(hint(format!(
                            "Revision {} · {} runs · {} pending proposals",
                            role["revision"].as_u64().unwrap_or(1),
                            info["runs"].as_u64().unwrap_or(0),
                            info["proposals"].as_u64().unwrap_or(0)
                        )))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new(SharedString::from(format!("edit-role-{id}")))
                                        .label("Edit")
                                        .accessibility_label(format!(
                                            "Edit {}",
                                            string(&role, "name")
                                        ))
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.settings.role = edit.clone();
                                            this.settings.role_open = true;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("duplicate-role-{id}")))
                                        .label("Duplicate")
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.new_role_draft(Some(duplicate.clone()), window, cx)
                                        })),
                                ),
                        ),
                );
            }
            page = page.child(
                div()
                    .id(SharedString::from(format!("role-category-{category}")))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(crate::accessibility::text(category))
                    .child(cards),
            );
        }
        if !self.settings.role_open {
            return page;
        }
        let editor = self.role_editor(window, cx);
        if window.viewport_size().width < px(1200.) {
            return editor;
        }
        let height = (f32::from(window.viewport_size().height) - 180.).max(300.);
        div()
            .flex()
            .items_start()
            .gap_4()
            .child(
                div()
                    .id("role-catalog-scroll")
                    .flex_1()
                    .min_w_0()
                    .h(px(height))
                    .overflow_y_scroll()
                    .child(page),
            )
            .child(
                div()
                    .id("role-editor-sheet")
                    .w(px(520.))
                    .flex_shrink_0()
                    .h(px(height))
                    .overflow_y_scroll()
                    .child(editor),
            )
    }

    fn role_preview(&mut self, cx: &mut Context<Self>) -> Div {
        let mut panel = section("Assembled-brief preview");
        let tasks = records(&self.workspace, "tasks");
        if self.settings.preview_task.is_empty() {
            self.settings.preview_task = self
                .selected
                .clone()
                .or_else(|| tasks.first().map(|t| string(t, "id")))
                .unwrap_or_default();
        }
        let mut choices = div().flex().flex_col().min_w_0().gap_2();
        for task in &tasks {
            let id = string(task, "id");
            let selected = self.settings.preview_task == id;
            choices = choices.child(
                Button::new(SharedString::from(format!("preview-ticket-{id}")))
                    .w_full()
                    .min_w_0()
                    .label(format!(
                        "{} · {}",
                        string(task, "key"),
                        string(task, "title")
                    ))
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.preview_task = id.clone();
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(choices).child(
            Button::new("preview-role")
                .label("Refresh preview")
                .disabled(self.blocked() || tasks.is_empty())
                .on_click(cx.listener(|this, _, _, cx| {
                    let role = this.settings.roles[&this.settings.role].value(cx);
                    let task = this.settings.preview_task.clone();
                    this.settings.preview_source = Some((role.clone(), task.clone()));
                    this.send(
                        "/role-preview".into(),
                        json!({"role":role,"taskId":task}),
                        false,
                        cx,
                    );
                })),
        );
        if tasks.is_empty() {
            return panel.child(hint(
                "Create a ticket to preview its accepted brief with this role.",
            ));
        }
        if let Some(preview) = &self.settings.preview {
            let current = self.settings.roles[&self.settings.role].value(cx);
            let current_task = tasks.iter().find(|t| t["id"] == self.settings.preview_task);
            let fresh = self
                .settings
                .preview_source
                .as_ref()
                .is_some_and(|(role, task)| {
                    *role == current && *task == self.settings.preview_task
                })
                && current_task.is_some_and(|t| *t == preview["task"])
                && crewroom_core::principles::snapshot(
                    &json!({}),
                    &current,
                    &records(&self.workspace, "principles"),
                )
                .is_ok_and(|b| b["principles"] == preview["assignment"]["brief"]["principles"]);
            let fresh = fresh
                && preview["catalogRevision"] == self.workspace["modelDiscovery"]["revision"]
                && preview["modelJobsRevision"] == self.workspace["modelJobs"]["revision"];
            panel = panel.child(super::super::model_details::details(
                &preview["assignment"]["model"],
            ));
            if let Some(error) = preview["modelError"].as_str() {
                panel = panel.child(hint(error.to_owned()));
            }
            panel = panel
                .child(hint(if fresh {
                    "Read-only preview of the current inputs."
                } else {
                    "Preview is out of date. Refresh it to include current inputs."
                }))
                .child(hint(string(preview, "scope")))
                .child(crate::accessibility::named(
                    "assembled-role-preview",
                    string(preview, "prompt"),
                ));
        }
        panel
    }

    fn role_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let id = self.settings.role.clone();
        let current = records(&self.workspace, "roles")
            .into_iter()
            .find(|r| r["id"] == id);
        if !self.settings.roles.contains_key(&id) {
            let mut value = current.clone().unwrap_or_else(|| {
                if id.starts_with(NEW) {
                    new_role()
                } else {
                    json!({})
                }
            });
            if current.is_some() && value["revision"].is_null() {
                value["revision"] = json!(1);
            }
            self.settings
                .roles
                .insert(id.clone(), RoleDraft::new(&value, window, cx));
        }
        let draft = &self.settings.roles[&id];
        let role_job = json!({"job":draft.job});
        let selected_job = crewroom_core::model_jobs::job(&role_job).0.to_owned();
        let model_input = draft.fields.inputs["model"].clone();
        let effort =
            if draft.effort == "default" && self.workspace["modelJobs"]["legacyDefault"] == true {
                self.workspace["modelJobs"]["legacyEffort"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned()
            } else if draft.effort == "default" {
                crewroom_core::model_jobs::effort(
                    self.workspace["modelJobs"]["budget"]
                        .as_str()
                        .unwrap_or("large"),
                    &selected_job,
                )
                .unwrap_or("unknown")
                .to_owned()
            } else {
                draft.effort.clone()
            };
        let inherited = self.workspace["modelJobs"]["jobs"][&selected_job]
            .as_str()
            .unwrap_or("unconfigured")
            .to_owned();
        let mut picker = self
            .model_picker(
                "role-model",
                "Model override (optional)",
                model_input,
                &effort,
                Some(inherited),
                window,
                cx,
            )
            .child(hint("Choose \"Use job model\" to inherit the job model."));
        for (index, status) in self.catalog_status().into_iter().enumerate() {
            picker = picker.child(
                div()
                    .id(SharedString::from(format!("role-catalog-status-{index}")))
                    .child(hint(status)),
            );
        }
        let draft = &self.settings.roles[&id];
        let mut form = section("Role editor").child(
            Button::new("close-role-editor")
                .label("Close editor · keep draft")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.role_open = false;
                    cx.notify();
                })),
        );
        form = form
            .child(crate::accessibility::text(if id.starts_with(NEW) {
                "New agent role".to_owned()
            } else {
                format!("Edit {}", draft.fields.text("name", cx))
            }))
            .child(hint("Changes apply to new assignments. Existing assignments keep their saved instructions and model. Drafts stay here when you switch roles or pages."));
        if let Some(revision) = draft.revision {
            form = form.child(hint(format!(
                "Editing revision {revision}{}",
                if draft.dirty(cx) {
                    " · Unsaved changes"
                } else {
                    ""
                }
            )));
        }
        if let Some(current) = &current
            && Some(current["revision"].as_u64().unwrap_or(1)) != draft.revision
        {
            form = form.child(hint(format!(
                "Current definition is revision {}. Your draft is retained; reload before saving.",
                current["revision"].as_u64().unwrap_or(1)
            )));
        }
        form = form.child(crate::accessibility::text("Attached principles"));
        let mut principles = records(&self.workspace, "principles");
        for id in &draft.principles {
            if !principles.iter().any(|p| p["id"] == *id) {
                principles.push(json!({"id":id,"name":format!("Unavailable: {id} · detach or restore before starting"),"text":""}));
            }
        }
        if principles.is_empty() {
            form = form.child(hint(
                "Create reusable guidance in Settings → Principles, then attach it here.",
            ));
        }
        for principle in principles {
            let pid = string(&principle, "id");
            let toggle = pid.clone();
            form = form.child(
                Checkbox::new(SharedString::from(format!("role-principle-{pid}")))
                    .label(string(&principle, "name"))
                    .checked(draft.principles.contains(&pid))
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, checked, _, cx| {
                        let ids = &mut this
                            .settings
                            .roles
                            .get_mut(&this.settings.role)
                            .unwrap()
                            .principles;
                        ids.retain(|id| id != &toggle);
                        if *checked {
                            ids.push(toggle.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        let role_job = json!({"job":draft.job});
        let (job, warning) = crewroom_core::model_jobs::job(&role_job);
        let selected_job = job.to_owned();
        let mut jobs = div().flex().flex_wrap().gap_2();
        for job in crewroom_core::model_jobs::JOBS {
            jobs = jobs.child(
                Button::new(SharedString::from(format!("role-job-{job}")))
                    .label(job)
                    .selected(selected_job == job)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings
                            .roles
                            .get_mut(&this.settings.role)
                            .unwrap()
                            .job = json!(job);
                        cx.notify();
                    })),
            );
        }
        form = form
            .child(crate::accessibility::text("Model job"))
            .child(jobs);
        if let Some(warning) = warning {
            form = form.child(hint(warning));
        }
        for (key, label) in [
            ("id", "Role ID"),
            ("name", "Role name"),
            ("category", "Category"),
            ("purpose", "Pick when"),
            ("instructions", "Agent instructions"),
        ] {
            form = form.child(draft.fields.render(
                "role",
                key,
                label,
                self.busy || (key == "id" && !id.starts_with(NEW)),
            ));
        }

        form = form.child(picker);
        let mut efforts = div().flex().flex_wrap().gap_2();
        for effort in [
            "default", "off", "minimal", "low", "medium", "high", "xhigh", "max",
        ] {
            efforts = efforts.child(
                Button::new(SharedString::from(format!("effort-{effort}")))
                    .label(if effort == "default" {
                        "Use budget"
                    } else {
                        effort
                    })
                    .selected(draft.effort == effort)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings
                            .roles
                            .get_mut(&this.settings.role)
                            .unwrap()
                            .effort = effort.into();
                        cx.notify();
                    })),
            );
        }
        form = form
            .child(crate::accessibility::text("Effort override (optional)"))
            .child(efforts)
            .child(
                Checkbox::new("role-enabled")
                    .label("Enabled for new assignments")
                    .checked(draft.enabled)
                    .disabled(
                        self.busy
                            || ["coordinator", "triage", "implementer", "reviewer"]
                                .contains(&id.as_str()),
                    )
                    .on_click(cx.listener(|this, checked, _, cx| {
                        this.settings
                            .roles
                            .get_mut(&this.settings.role)
                            .unwrap()
                            .enabled = *checked;
                        cx.notify();
                    })),
            );
        form = form.child(self.comparison_entry(
            "role",
            "role",
            &id,
            &draft.value(cx),
            draft.dirty(cx),
            None,
            cx,
        ));
        form = form.child(self.model_refresh(cx));
        if ["coordinator", "triage", "implementer", "reviewer"].contains(&id.as_str()) {
            form = form.child(hint("Required by the coding workflow; must stay enabled."));
        }
        if let Some(role) = &current {
            form = form.child(self.role_learning(role, window, cx));
        }
        let preview = self.role_preview(cx);
        let target = id.clone();
        form.child(preview)
            .children(self.feedback(crate::workbench::feedback::Place::SaveRow))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("save-role")
                            .primary()
                            .label("Save agent role")
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let id = this.settings.role.clone();
                                let body = this.settings.roles[&id].value(cx);
                                this.settings.submitted_role = string(&body, "id");
                                this.send(
                                    if id.starts_with(NEW) {
                                        "/roles".into()
                                    } else {
                                        format!("/roles/{id}")
                                    },
                                    body,
                                    false,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("reload-role")
                            .label(if id.starts_with(NEW) {
                                "Discard new role"
                            } else {
                                "Reload saved role"
                            })
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.reload_setting("role", target.clone(), window, cx)
                            })),
                    ),
            )
    }
}
