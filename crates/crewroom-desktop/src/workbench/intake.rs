//! Native intake decisions use the service's currentness and keyed operator receipts.
use super::*;
fn lines(text: String) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn prose(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}
fn package_fields(package: &Value) -> Value {
    json!({"packageId":package["id"],"name":package["name"],"dependencies":prose(&package["dependsOn"]),"outputs":prose(&package["outputs"]),"criteria":records(package,"criteria").iter().filter_map(Value::as_u64).map(|i|(i+1).to_string()).collect::<Vec<_>>().join(", "),"validation":prose(&package["validation"])})
}
/// One work package as typed in the plan editor, before it becomes plan content.
pub(super) struct PackageDraft {
    pub id: String,
    pub name: String,
    pub depends_on: Vec<String>,
    pub outputs: Vec<String>,
    pub criteria: String,
    pub validation: Vec<String>,
}
/// A plan editor problem, keyed to the field it is shown beside: `goal`, `scope`,
/// `package-{index}-{field}` or `plan`.
pub(super) struct Problem {
    pub field: String,
    pub message: String,
}
fn problem(field: impl Into<String>, message: impl Into<String>) -> Problem {
    Problem {
        field: field.into(),
        message: message.into(),
    }
}
/// Names each problem by its field and work package, in the operator's words.
/// Mirrors the service schema (`crewroom_core::intake::plan::validate`), which stays authoritative.
pub(super) fn editor_problems(
    goal: &str,
    scope: &[String],
    packages: &[PackageDraft],
    criteria: usize,
) -> Vec<Problem> {
    let mut out = Vec::new();
    if goal.trim().is_empty() {
        out.push(problem("goal", "Goal: add a goal."));
    } else if goal.trim().chars().count() > 4000 {
        out.push(problem("goal", "Goal: keep it within 4,000 characters."));
    }
    if scope.is_empty() {
        out.push(problem("scope", "Scope: add at least one item."));
    }
    if packages.is_empty() {
        out.push(problem("plan", "Add at least one work package."));
    }
    let ids: Vec<&str> = packages.iter().map(|p| p.id.trim()).collect();
    let mut package_problems = false;
    let mut covered = std::collections::BTreeSet::new();
    for (index, package) in packages.iter().enumerate() {
        let n = index + 1;
        let mut add = |field: &str, message: String| {
            out.push(problem(format!("package-{index}-{field}"), message));
            package_problems = true;
        };
        let id = package.id.trim();
        if id.is_empty() {
            add(
                "packageId",
                format!("Work package {n} · Package ID: add an ID."),
            );
        } else if !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            add(
                "packageId",
                format!(
                    "Work package {n} · Package ID: use letters, numbers, hyphens or underscores."
                ),
            );
        } else if id.chars().count() > 64 {
            add(
                "packageId",
                format!("Work package {n} · Package ID: keep it within 64 characters."),
            );
        } else if let Some(first) = ids[..index].iter().position(|other| *other == id) {
            add(
                "packageId",
                format!(
                    "Work package {n} · Package ID: {id} is already used by work package {}.",
                    first + 1
                ),
            );
        }
        if package.name.trim().is_empty() {
            add("name", format!("Work package {n} · Name: add a name."));
        } else if package.name.trim().chars().count() > 200 {
            add(
                "name",
                format!("Work package {n} · Name: keep it within 200 characters."),
            );
        }
        for (i, dependency) in package.depends_on.iter().enumerate() {
            if !ids.contains(&dependency.as_str()) {
                add(
                    "dependencies",
                    format!("Work package {n} · Depends on: no work package has ID {dependency}."),
                );
            } else if package.depends_on[..i].contains(dependency) {
                add(
                    "dependencies",
                    format!("Work package {n} · Depends on: {dependency} is listed twice."),
                );
            }
        }
        if package.outputs.is_empty() {
            add(
                "outputs",
                format!("Work package {n} · Expected outputs: add at least one line."),
            );
        }
        let mut seen = Vec::new();
        for item in package.criteria.split(',').map(str::trim) {
            if item.is_empty() {
                continue;
            }
            match item.parse::<usize>() {
                Err(_) => add(
                    "criteria",
                    format!("Work package {n} · Criterion numbers: {item} is not a number."),
                ),
                Ok(k) if k == 0 || k > criteria => add(
                    "criteria",
                    format!("Work package {n} · Criterion {k} does not exist (1–{criteria})."),
                ),
                Ok(k) if seen.contains(&k) => add(
                    "criteria",
                    format!("Work package {n} · Criterion {k} is listed twice."),
                ),
                Ok(k) => {
                    seen.push(k);
                    covered.insert(k);
                }
            }
        }
        if package.criteria.split(',').all(|s| s.trim().is_empty()) {
            add(
                "criteria",
                format!("Work package {n} · Criterion numbers: add at least one."),
            );
        }
        if package.validation.is_empty() {
            add(
                "validation",
                format!("Work package {n} · Validation: add at least one check."),
            );
        }
    }
    if criteria == 0 {
        out.push(problem(
            "plan",
            "Add acceptance criteria to the brief before writing a plan.",
        ));
    } else if !packages.is_empty() {
        let missing: Vec<String> = (1..=criteria)
            .filter(|k| !covered.contains(k))
            .map(|k| k.to_string())
            .collect();
        match missing.len() {
            0 => {}
            1 => out.push(problem(
                "plan",
                format!(
                    "Criterion {} is not covered by any work package.",
                    missing[0]
                ),
            )),
            _ => out.push(problem(
                "plan",
                format!(
                    "Criteria {} are not covered by any work package.",
                    missing.join(", ")
                ),
            )),
        }
    }
    // A cycle is only meaningful once every package ID and dependency is valid.
    if !package_problems {
        let mut done: Vec<&str> = Vec::new();
        while done.len() < packages.len() {
            let ready: Vec<&str> = packages
                .iter()
                .filter(|p| !done.contains(&p.id.trim()))
                .filter(|p| p.depends_on.iter().all(|d| done.contains(&d.as_str())))
                .map(|p| p.id.trim())
                .collect();
            if ready.is_empty() {
                out.push(problem("plan", "Work package dependencies form a cycle."));
                break;
            }
            done.extend(ready);
        }
    }
    out
}
impl Workbench {
    pub(super) fn intake_controls(
        &mut self,
        task: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let task_id = string(task, "id");
        let view = self.workspace["intake"][&task_id].clone();
        if view.is_null() {
            return div();
        }
        let intake = &view["intake"];
        let source = &view["source"];
        let plans = records(&view, "plans");
        let latest = plans.last();
        let mut panel = section("Implementation plan").child(crate::accessibility::named(
            "intake-state",
            string(&view, "state"),
        ));
        if let Some(reason) = intake["holdReason"].as_str() {
            panel = panel.child(hint(reason.to_owned()));
        }
        if intake.is_object() {
            let id = string(intake, "id");
            let cancel = format!("/intakes/{id}/cancel");
            let revision = intake["revision"].clone();
            panel=panel.child(hint("This queued request can proceed after you accept a current plan. Scoping and coding remain separate steps."))
    .child(Button::new("cancel-intake").label("Cancel queued request").disabled(self.blocked()).on_click(cx.listener(move|this,_,_,cx|this.send(cancel.clone(),json!({"revision":revision}),true,cx))));
            panel = panel.child(self.intake_questions(intake, &view, window, cx));
        } else if view["currentPlan"].is_object() {
            panel=panel.child(hint("Queue will reuse this accepted plan. Normal launch and validation gates still apply."));
        } else {
            panel=panel.child(hint("Queue this job to scope it with a coordinator, or write a plan here. Saving a ticket or draft starts no agent."));
            // The editor needs a pinned source, and the basis needs a target repository.
            if !task["repositoryId"].is_string() {
                panel = panel.child(named_hint(
                    "plan-prerequisite",
                    "To write a plan here, choose a target repository below, then select Refresh repository basis.",
                ));
            } else if !source.is_object() {
                panel = panel.child(named_hint(
                    "plan-prerequisite",
                    "Select Refresh repository basis to pin the source a plan is written against.",
                ));
            }
        }
        if task["repositoryId"].is_string() {
            let id = task_id.clone();
            let revision = task["revision"].clone();
            panel = panel.child(
                Button::new("refresh-plan-source")
                    .label("Refresh repository basis")
                    .disabled(
                        self.blocked()
                            || self.brief_dirty(task, cx)
                            || task["needsReacceptance"] == true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(
                            format!("/tasks/{id}/plan-source"),
                            json!({"revision":revision}),
                            false,
                            cx,
                        )
                    })),
            );
            let blockers = super::blockers::refresh_basis(
                self.brief_dirty(task, cx),
                super::blockers::reacceptance(task),
            );
            panel = panel.children(super::blockers::line(
                "refresh-basis-blockers",
                "refresh the basis",
                &blockers,
            ));
        }
        if source.is_object() {
            panel = panel.child(hint(format!(
                "Requirements revision {} · source {}",
                source["taskRevision"],
                string(source, "commit")
            )));
            if intake.is_object()
                && (intake["sourceId"] != source["id"] || view["state"] == "Needs revalidation")
            {
                let path = format!("/intakes/{}/revalidate", string(intake, "id"));
                let body = json!({"revision":intake["revision"],"sourceId":source["id"]});
                panel=panel.child(hint("Pause scoping, refresh the repository basis, then revalidate. Previous answers stay in history."))
     .child(Button::new("revalidate-intake").label("Revalidate using this basis").disabled(self.blocked()||!["paused","awaiting_runner"].contains(&view["run"]["status"].as_str().unwrap_or(""))).on_click(cx.listener(move|this,_,_,cx|this.send(path.clone(),body.clone(),true,cx))));
            }
        }
        if let Some(plan) = latest {
            // An undecided plan's requirements render beside Accept, against the current brief.
            let compare = plan["decision"].is_null() && plan["requirements"].is_object();
            panel = panel.child(render_plan(plan, !compare));
            if plan["decision"].is_null() {
                // Without an active intake, no agent acts on a rejection.
                let operator_plan = !intake.is_object();
                let path = format!("/implementation-plans/{}/decision", string(plan, "id"));
                self.collaboration.form(
                    &path,
                    &json!({"reason":""}),
                    &[("reason", true)],
                    window,
                    cx,
                );
                let form = &self.collaboration.forms[&path];
                let reason = form.fields.text("reason", cx);
                panel = panel.child(form.fields.render(
                    &path,
                    "reason",
                    if operator_plan {
                        "Reason for rejecting (required to reject)"
                    } else {
                        "Reason for revision (required to request revision)"
                    },
                    self.busy,
                ));
                let waiting = records(intake, "questions")
                    .iter()
                    .any(|q| q["sourceId"] == plan["basis"]["id"] && q["answer"].is_null());
                let editor_path = format!("/tasks/{task_id}/implementation-plans");
                let dirty_plan = self.collaboration.forms.iter().any(|(key, f)| {
                    (key == &editor_path || key.starts_with(&format!("{editor_path}/")))
                        && (f.fields.values(cx) != f.original || f.rows != f.original_rows)
                });
                let stale = !plan["holdReason"].is_null()
                    || plan["basis"]["taskRevision"] != task["revision"];
                // Source re-acceptance disables Refresh, so guidance starts at Save brief.
                if task["needsReacceptance"] == true {
                    panel = panel.child(hint(format!(
                        "{} requirements changed since this plan.",
                        crewroom_core::ticket_source::source_name(task)
                    )));
                } else if stale {
                    panel = panel.child(hint(
                        "The task or repository basis changed since this plan.",
                    ));
                }
                let state = super::blockers::PlanDecision {
                    brief_dirty: self.brief_dirty(task, cx),
                    needs_reacceptance: super::blockers::reacceptance(task),
                    stale,
                    waiting,
                    dirty_plan,
                    reason_missing: reason.trim().is_empty(),
                    operator_plan,
                };
                let reject_label = if operator_plan {
                    "Reject plan"
                } else {
                    "Request revision"
                };
                for (action, blockers) in [
                    ("decide", state.decide()),
                    ("accept", state.accept()),
                    (
                        if operator_plan {
                            "reject the plan"
                        } else {
                            "request revision"
                        },
                        state.reject(),
                    ),
                ] {
                    panel = panel.children(super::blockers::line(
                        format!("plan-blockers-{action}"),
                        action,
                        &blockers,
                    ));
                }
                let mut actions = div().flex().flex_wrap().gap_2();
                for (decision, label) in [("accept", "Accept plan"), ("reject", reject_label)] {
                    let mut body = json!({"decision":decision,"revision":task["revision"],"sourceId":plan["basis"]["id"]});
                    if !reason.trim().is_empty() {
                        body["reason"] = json!(reason.trim());
                    }
                    let path = path.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!("plan-{decision}")))
                            .label(label)
                            .disabled(
                                self.blocked()
                                    || self.brief_dirty(task, cx)
                                    || stale
                                    || (decision == "accept" && (waiting || dirty_plan))
                                    || (decision == "reject" && reason.trim().is_empty()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(path.clone(), body.clone(), true, cx)
                            })),
                    );
                }
                if compare {
                    panel = panel.child(requirements_change(task, &plan["requirements"]));
                }
                panel=panel.child(actions).child(hint(if intake.is_object(){"Accepting releases this queued request after scoping has stopped. Coding starts when its normal queue gates allow it."}else{"Accepting saves the plan. Queue remains a separate action."}));
                if operator_plan {
                    panel = panel.child(hint(
                        "No agent will revise a rejected plan. Use Edit as a new plan version to write the next one.",
                    ));
                }
            }
        }
        if source.is_object() {
            panel = panel.child(self.intake_plan_editor(task, latest, source, window, cx));
        }
        if plans.len() > 1 {
            let mut history = section("Plan history");
            for plan in plans.iter().rev().skip(1) {
                history = history.child(render_plan(plan, true));
            }
            panel = panel.child(history);
        }
        for previous in records(&view, "history") {
            if previous["id"] != intake["id"] {
                panel = panel.child(
                    section("Intake history")
                        .id(SharedString::from(format!(
                            "intake-history-row-{}",
                            string(&previous, "id")
                        )))
                        .child(crate::accessibility::named(
                            format!("intake-history-{}", string(&previous, "id")),
                            format!("Request {}", string(&previous, "state")),
                        ))
                        .child(hint(format!(
                            "Request {} · scoping run {} · coding run {}",
                            string(&previous, "id"),
                            string(&previous, "scopingRunId"),
                            previous["codingRunId"].as_str().unwrap_or("None")
                        )))
                        .child(self.intake_questions(&previous, &view, window, cx)),
                );
            }
        }
        panel
    }
    fn intake_questions(
        &mut self,
        intake: &Value,
        view: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let path = format!("/intakes/{}/answers", string(intake, "id"));
        let mut panel = div().flex().flex_col().gap_2();
        let mut answers = Vec::new();
        for q in records(intake, "questions") {
            let mut row = section(string(&q, "prompt"))
                .id(SharedString::from(format!(
                    "intake-question-{}-{}",
                    string(intake, "id"),
                    string(&q, "id")
                )))
                .child(hint(string(&q, "rationale")));
            if q["answer"].is_object() {
                let message = records(&self.workspace, "messages")
                    .into_iter()
                    .find(|m| m["id"] == q["answer"]["steeringId"])
                    .unwrap_or(Value::Null);
                row = row
                    .child(crate::accessibility::text(string(&q["answer"], "body")))
                    .child(hint(format!(
                        "Answer saved · delivery {}",
                        message["delivery"].as_str().unwrap_or("unconfirmed")
                    )));
            } else if q["sourceId"] == intake["sourceId"]
                && !["cancelled", "handed_off"].contains(&intake["state"].as_str().unwrap_or(""))
            {
                let key = format!("{path}/{}", string(&q, "id"));
                self.collaboration.form(
                    &key,
                    &json!({"answer":""}),
                    &[("answer", true)],
                    window,
                    cx,
                );
                row = row.child(self.collaboration.forms[&key].fields.render(
                    &key,
                    "answer",
                    "Your answer",
                    self.busy,
                ));
                let mut choices = div().flex().flex_wrap().gap_2();
                for (index, choice) in records(&q, "choices").iter().enumerate() {
                    let key = key.clone();
                    let choice = choice.as_str().unwrap_or("").to_owned();
                    choices = choices.child(
                        Button::new(SharedString::from(format!(
                            "{}-choice-{index}",
                            string(&q, "id")
                        )))
                        .label(choice.clone())
                        .disabled(self.busy)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.collaboration.forms[&key].fields.areas["answer"]
                                    .update(cx, |input, cx| {
                                        input.set_value(choice.clone(), window, cx)
                                    });
                                cx.notify();
                            },
                        )),
                    );
                }
                row = row.child(choices);
                answers.push(json!({"id":q["id"],"body":self.collaboration.forms[&key].fields.text("answer",cx)}));
            } else {
                row = row.child(hint(if q["sourceId"] != intake["sourceId"] {
                    "Superseded by a newer source basis."
                } else {
                    "No answer recorded before this request closed."
                }));
                let key = format!("{path}/{}", string(&q, "id"));
                if let Some(form) = self.collaboration.forms.get(&key) {
                    let draft = form.fields.text("answer", cx);
                    if !draft.is_empty() {
                        row = row.child(
                            section("Unsent local draft")
                                .child(crate::accessibility::named(
                                    format!(
                                        "unsent-answer-{}-{}",
                                        string(intake, "id"),
                                        string(&q, "id")
                                    ),
                                    draft,
                                ))
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "discard-answer-{}-{}",
                                        string(intake, "id"),
                                        string(&q, "id")
                                    )))
                                    .label("Discard local draft")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.collaboration.forms.remove(&key);
                                        cx.notify();
                                    })),
                                ),
                        );
                    }
                }
            }
            panel = panel.child(row);
        }
        if !answers.is_empty() {
            let disabled = self.blocked()
                || answers.iter().any(|a| string(a, "body").trim().is_empty())
                || view["state"] != "Waiting for you";
            let body = json!({"revision":intake["revision"],"sourceId":intake["sourceId"],"answers":answers});
            panel = panel.child(
                Button::new("answer-intake")
                    .primary()
                    .label("Send answers and continue scoping")
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(path.clone(), body.clone(), true, cx)
                    })),
            );
        }
        panel
    }
    fn intake_plan_editor(
        &mut self,
        task: &Value,
        plan: Option<&Value>,
        source: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let path = format!("/tasks/{}/implementation-plans", string(task, "id"));
        let content=plan.map(|p|p["content"].clone()).unwrap_or_else(||json!({"schemaVersion":1,"goal":task["brief"],"scope":[],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"implementation","name":"Implementation","dependsOn":[],"outputs":[],"criteria":(0..records(task,"criteria").len()).collect::<Vec<_>>(),"validation":[]}]}));
        let was_open = self.collaboration.forms.get(&path).is_some_and(|f| f.edit);
        let changed = self.collaboration.forms.get(&path).is_some_and(|f| {
            f.basis["predecessorId"] != plan.map(|p| p["id"].clone()).unwrap_or(Value::Null)
                || f.basis["revision"] != task["revision"]
                || f.basis["sourceId"] != source["id"]
        });
        let dirty = self.collaboration.forms.iter().any(|(key, f)| {
            (key == &path || key.starts_with(&format!("{path}/")))
                && (f.fields.values(cx) != f.original || f.rows != f.original_rows)
        });
        if changed && !dirty {
            self.collaboration
                .forms
                .retain(|key, _| key != &path && !key.starts_with(&format!("{path}/")));
        }
        if !self.collaboration.forms.contains_key(&path) {
            let mut fields = json!({"goal":content["goal"],"scope":prose(&content["scope"]),"exclusions":prose(&content["exclusions"]),"assumptions":prose(&content["assumptions"]),"decisions":prose(&content["decisions"])});
            fields["content"] = content.clone();
            fields["revision"] = task["revision"].clone();
            fields["sourceId"] = source["id"].clone();
            fields["predecessorId"] = plan.map(|p| p["id"].clone()).unwrap_or(Value::Null);
            if let Some(r) = plan.and_then(|p| p.get("requirements")) {
                fields["requirements"] = r.clone();
            }
            self.collaboration.form(
                &path,
                &fields,
                &[
                    ("goal", true),
                    ("scope", true),
                    ("exclusions", true),
                    ("assumptions", true),
                    ("decisions", true),
                ],
                window,
                cx,
            );
            let form = self.collaboration.forms.get_mut(&path).unwrap();
            form.edit = was_open;
            form.rows = (0..records(&content, "workPackages").len())
                .map(|i| i.to_string())
                .collect();
            form.original_rows = form.rows.clone();
        }
        let open = self.collaboration.forms[&path].edit;
        let toggle = path.clone();
        let mut panel = div().flex().flex_col().gap_2().child(
            Button::new("edit-implementation-plan")
                .label(if open {
                    "Hide plan editor"
                } else if plan.is_some() {
                    "Edit as a new plan version"
                } else {
                    "Write a plan"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let form = this.collaboration.forms.get_mut(&toggle).unwrap();
                    form.edit = !form.edit;
                    cx.notify();
                })),
        );
        if !open {
            return panel;
        }
        let basis = self.collaboration.forms[&path].basis.clone();
        if basis["sourceId"] != source["id"]
            || basis["revision"] != task["revision"]
            || basis["predecessorId"] != plan.map(|p| p["id"].clone()).unwrap_or(Value::Null)
        {
            let key = path.clone();
            let revision = task["revision"].clone();
            let source = source["id"].clone();
            let predecessor = plan.map(|p| p["id"].clone()).unwrap_or(Value::Null);
            panel=panel.child(hint("The basis changed. Your edits remain here. Review the current requirements and source before rebinding this draft."))
    .child(Button::new("rebind-plan-draft").label("Use current basis for this draft").disabled(self.blocked()).on_click(cx.listener(move|this,_,_,cx|{let form=this.collaboration.forms.get_mut(&key).unwrap();form.basis["revision"]=revision.clone();form.basis["sourceId"]=source.clone();form.basis["predecessorId"]=predecessor.clone();cx.notify();})));
        }
        let rows = self.collaboration.forms[&path].rows.clone();
        let mut drafts = Vec::new();
        for row_id in &rows {
            let key = format!("{path}/packages/{row_id}");
            // Added rows carry their default package ID as the row ID.
            let initial=row_id.parse::<usize>().ok().and_then(|i|basis["content"]["workPackages"].as_array().and_then(|a|a.get(i))).cloned().unwrap_or(json!({"id":row_id,"name":"","dependsOn":[],"outputs":[],"criteria":[],"validation":[]}));
            self.collaboration.form(
                &key,
                &package_fields(&initial),
                &[
                    ("packageId", false),
                    ("name", false),
                    ("dependencies", true),
                    ("outputs", true),
                    ("criteria", false),
                    ("validation", true),
                ],
                window,
                cx,
            );
            let fields = &self.collaboration.forms[&key].fields;
            drafts.push(PackageDraft {
                id: fields.text("packageId", cx),
                name: fields.text("name", cx),
                depends_on: lines(fields.text("dependencies", cx)),
                outputs: lines(fields.text("outputs", cx)),
                criteria: fields.text("criteria", cx),
                validation: lines(fields.text("validation", cx)),
            });
        }
        let form = &self.collaboration.forms[&path];
        let mut new_content = json!({"schemaVersion":1,"goal":form.fields.text("goal",cx)});
        for field in ["scope", "exclusions", "assumptions", "decisions"] {
            new_content[field] = json!(lines(form.fields.text(field, cx)));
        }
        new_content["workPackages"] = json!(drafts.iter().map(|d| {
            let criteria: Vec<Value> = d.criteria.split(',').map(|s| s.trim().parse::<u64>().ok().and_then(|n| n.checked_sub(1)).map_or(Value::Null, |v| json!(v))).collect();
            json!({"id":d.id,"name":d.name,"dependsOn":d.depends_on,"outputs":d.outputs,"criteria":criteria,"validation":d.validation})
        }).collect::<Vec<_>>());
        let criteria = if basis["requirements"].is_object() {
            records(&basis["requirements"], "criteria")
        } else {
            records(task, "criteria")
        };
        let mut problems = editor_problems(
            &form.fields.text("goal", cx),
            &lines(form.fields.text("scope", cx)),
            &drafts,
            criteria.len(),
        );
        // The service schema stays authoritative for limits the editor does not check.
        if problems.is_empty()
            && let Err(error) = crewroom_core::intake::plan::validate(&new_content, &criteria)
        {
            problems.push(Problem {
                field: "plan".into(),
                message: error.to_string(),
            });
        }
        let invalid = !problems.is_empty();
        // A pristine editor reports nothing; problems appear once the operator edits.
        let edited = self.collaboration.forms.iter().any(|(key, f)| {
            (key == &path || key.starts_with(&format!("{path}/")))
                && (f.fields.values(cx) != f.original || f.rows != f.original_rows)
        });
        if !edited {
            problems.clear();
        }
        let shown = |field: &str| {
            problems
                .iter()
                .enumerate()
                .filter(|(_, p)| p.field == field)
                .map(|(i, p)| named_hint(format!("plan-problem-{i}"), p.message.clone()))
                .collect::<Vec<_>>()
        };
        let form = &self.collaboration.forms[&path];
        for (key, label) in [
            ("goal", "Goal"),
            ("scope", "Scope · one item per line"),
            ("exclusions", "Excluded work · one item per line"),
            ("assumptions", "Assumptions · one per line"),
            ("decisions", "Decisions · one per line"),
        ] {
            panel = panel
                .child(form.fields.render(&path, key, label, self.busy))
                .children(shown(key));
        }
        for (index, row_id) in rows.iter().enumerate() {
            let key = format!("{path}/packages/{row_id}");
            let form = &self.collaboration.forms[&key];
            let mut row = section(format!("Work package {}", index + 1))
                .id(SharedString::from(format!("plan-editor-{key}")));
            for (field, label) in [
                ("packageId", "Package ID"),
                ("name", "Name"),
                ("dependencies", "Depends on package IDs · one per line"),
                ("outputs", "Expected outputs · one per line"),
                (
                    "criteria",
                    "Criterion numbers · comma separated, starting at 1",
                ),
                ("validation", "Validation · one check per line"),
            ] {
                row = row
                    .child(form.fields.render(&key, field, label, self.busy))
                    .children(shown(&format!("package-{index}-{field}")));
            }
            let parent = path.clone();
            let remove = row_id.clone();
            row = row.child(
                Button::new(SharedString::from(format!("remove-package-{row_id}")))
                    .label("Remove work package")
                    .disabled(self.blocked())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.collaboration
                            .forms
                            .get_mut(&parent)
                            .unwrap()
                            .rows
                            .retain(|r| r != &remove);
                        cx.notify();
                    })),
            );
            panel = panel.child(row);
        }
        let parent = path.clone();
        let used: Vec<String> = drafts.iter().map(|d| d.id.trim().to_owned()).collect();
        panel = panel.child(
            Button::new("add-plan-package")
                .label("Add work package")
                .disabled(self.blocked())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let forms = &mut this.collaboration.forms;
                    let rows = &forms[&parent].rows;
                    // The next unused number; removed rows keep their drafts under their key.
                    let id = (rows.len() + 1..)
                        .map(|n| format!("package-{n}"))
                        .find(|id| {
                            !used.contains(id)
                                && !rows.contains(id)
                                && !forms.contains_key(&format!("{parent}/packages/{id}"))
                        })
                        .unwrap();
                    forms.get_mut(&parent).unwrap().rows.push(id);
                    cx.notify();
                })),
        );
        panel = panel.children(shown("plan"));
        if invalid && !edited {
            panel = panel.child(hint(
                "Save becomes available when every required field is complete.",
            ));
        }
        let mut body = json!({"revision":basis["revision"],"sourceId":basis["sourceId"],"predecessorId":basis["predecessorId"],"content":new_content});
        if let Some(r) = basis.get("requirements") {
            body["requirements"] = r.clone();
        }
        panel = panel.children(super::blockers::line(
            "save-plan-blockers",
            "save the plan",
            &super::blockers::save_plan(self.brief_dirty(task, cx)),
        ));
        panel.child(
            Button::new("save-implementation-plan")
                .label("Save plan draft")
                .disabled(self.blocked() || invalid || self.brief_dirty(task, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(path.clone(), body.clone(), true, cx)
                })),
        )
    }
}
fn criteria_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "criterion" } else { "criteria" })
}
fn requirements_card(
    id: String,
    title: &str,
    brief: &str,
    criteria: &Value,
) -> gpui_kit::Stateful<Div> {
    section(title.to_owned())
        .id(SharedString::from(id))
        .flex_1()
        .min_w(px(240.))
        .child(crate::accessibility::text(brief.to_owned()))
        .child(crate::accessibility::text(prose(criteria)))
}
/// Accepting a plan that carries requirements also replaces the accepted brief and criteria.
fn requirements_change(task: &Value, proposed: &Value) -> Div {
    let revision = task["revision"].as_u64().unwrap_or(1);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(hint(format!(
            "Accepting also replaces your accepted brief and {} with this plan's proposed requirements (revision {revision} → {}).",
            criteria_count(records(task, "criteria").len()),
            revision + 1
        )))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(requirements_card(
                    "requirements-current".into(),
                    "Current requirements",
                    task["brief"].as_str().unwrap_or(""),
                    &task["criteria"],
                ))
                .child(requirements_card(
                    "requirements-proposed".into(),
                    "Proposed requirements",
                    proposed["brief"].as_str().unwrap_or(""),
                    &proposed["criteria"],
                )),
        )
}
fn render_plan(plan: &Value, show_requirements: bool) -> impl IntoElement {
    let content = &plan["content"];
    let mut panel = section(format!(
        "Plan {} · {}",
        plan["version"],
        match plan["decision"]["decision"].as_str() {
            Some("accept") => "Accepted",
            Some("reject") => "Rejected",
            Some(_) => "Decided",
            // A held plan cannot be decided; its hold reason says why.
            None if plan["holdReason"].is_string() => "Set aside",
            None => "Awaiting decision",
        }
    ))
    .id(SharedString::from(format!(
        "implementation-plan-{}",
        string(plan, "id")
    )))
    .child(crate::accessibility::text(string(content, "goal")));
    if let Some(reason) = plan["holdReason"].as_str() {
        panel = panel.child(hint(reason.to_owned()));
    }
    for (field, label) in [
        ("scope", "Scope"),
        ("exclusions", "Excluded work"),
        ("assumptions", "Assumptions"),
        ("decisions", "Decisions"),
    ] {
        if !records(content, field).is_empty() {
            panel = panel.child(
                section(label)
                    .id(SharedString::from(format!(
                        "plan-{}-{field}",
                        string(plan, "id")
                    )))
                    .child(crate::accessibility::text(prose(&content[field]))),
            );
        }
    }
    for package in records(content, "workPackages") {
        panel = panel.child(
            section(format!(
                "{} · {}",
                string(&package, "id"),
                string(&package, "name")
            ))
            .id(SharedString::from(format!(
                "plan-package-{}-{}",
                string(plan, "id"),
                string(&package, "id")
            )))
            .child(hint(format!(
                "Depends on: {}",
                if records(&package, "dependsOn").is_empty() {
                    "None".into()
                } else {
                    prose(&package["dependsOn"])
                }
            )))
            .child(crate::accessibility::text(format!(
                "Outputs\n{}\nValidation\n{}",
                prose(&package["outputs"]),
                prose(&package["validation"])
            )))
            .child(hint(format!(
                "Criteria: {}",
                records(&package, "criteria")
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|i| (i + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        );
    }
    if show_requirements && plan["requirements"].is_object() {
        // Only an accepted plan adopted its requirements; others remain proposals.
        let title = if plan["decision"]["decision"] == "accept" {
            "Requirements adopted with acceptance"
        } else {
            "Proposed requirements"
        };
        panel = panel.child(requirements_card(
            format!("plan-requirements-{}", string(plan, "id")),
            title,
            plan["requirements"]["brief"].as_str().unwrap_or(""),
            &plan["requirements"]["criteria"],
        ));
    }
    panel.child(hint(format!(
        "Requirements revision {} · source {} · plan {}",
        plan["basis"]["taskRevision"],
        string(&plan["basis"], "commit"),
        string(plan, "sha256")
    )))
}
