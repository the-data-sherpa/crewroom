//! Native plan controls capture intent; the service alone admits execution.
use super::*;

#[derive(Default)]
pub(super) struct Planning {
    coordinator_roles: HashMap<String, String>,
    pub(super) playbooks: HashMap<String, String>,
    pub(super) playbook_open: std::collections::HashSet<String>,
}
fn open_run(run: &Value) -> bool {
    !["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""))
}
pub(super) fn hold_reason(plan: &Value, workspace: &Value) -> String {
    plan["holdReason"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if workspace["capabilities"]["execution"] == true {
                "Eligible · scheduling"
            } else {
                "Runner not configured"
            }
            .into()
        })
}
impl Workbench {
    pub(super) fn brief_dirty(&self, task: &Value, cx: &App) -> bool {
        self.drafts.get(&string(task, "id")).is_some_and(|d| {
            d.brief.read(cx).value().as_ref() != task["brief"].as_str().unwrap_or("")
                || d.criteria
                    .read(cx)
                    .value()
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    != records(task, "criteria")
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
        })
    }
    /// The version of the accepted plan Queue would reuse, when one is current.
    pub(super) fn accepted_plan_version(&self, task_id: &str) -> Option<u64> {
        self.workspace["intake"][task_id]["currentPlan"]["version"].as_u64()
    }
    /// The role chosen for a ticket's coordinator: the operator's choice, else the saved
    /// launch's, else the enabled "coordinator" role, else the first enabled role.
    pub(super) fn coordinator_role(&self, task_id: &str, current: Option<&Value>) -> String {
        if let Some(role) = self.planning.coordinator_roles.get(task_id) {
            return role.clone();
        }
        let roles: Vec<_> = records(&self.workspace, "roles")
            .into_iter()
            .filter(|r| r["enabled"] == true)
            .collect();
        current
            .and_then(|r| {
                records(r.get("launch").unwrap_or(&Value::Null), "team")
                    .into_iter()
                    .find(|m| m["coordinator"] == true)
            })
            .map(|m| string(&m, "roleId"))
            .or_else(|| {
                roles
                    .iter()
                    .find(|r| r["id"] == "coordinator")
                    .or(roles.first())
                    .map(|r| string(r, "id"))
            })
            .unwrap_or_default()
    }
    /// The default launch: one coordinator with the ticket's brief, in a new worktree.
    pub(super) fn coordinator_launch(&self, snapshot: &Value, role: &str) -> Value {
        let mut member = json!({"name":"Coordinator","roleId":role,"coordinator":true,"input":json!({"brief":snapshot["brief"],"criteria":snapshot["criteria"]}).to_string()});
        self.capture_model_preview(&mut member, role);
        json!({"team":[member],"repository":{"mode":"new"}})
    }
    pub(super) fn plan_controls(
        &mut self,
        task: &Value,
        runs: &[Value],
        cx: &mut Context<Self>,
    ) -> Div {
        let task_id = string(task, "id");
        let current = runs.iter().find(|r| open_run(r));
        if current.is_some_and(|run| run["purpose"].as_str().unwrap_or("coding") != "coding") {
            return div();
        }
        let repos = records(&self.workspace, "repositories");
        let target = repos.iter().find(|r| r["id"] == task["repositoryId"]);
        let retired = target.is_some_and(|r| !r["retiredAt"].is_null());
        let mut panel = section("Project");
        if let Some(repo) = target {
            panel = panel.child(hint(format!(
                "Saved target: {}{} · {}",
                string(repo, "name"),
                if retired { " · Retired" } else { "" },
                string(repo, "targetBranch")
            )));
        } else {
            panel = panel.child(hint(
                "No project selected. The ticket is listed under Unassigned.",
            ));
        }
        if let Some(version) = self.accepted_plan_version(&task_id)
            && repos
                .iter()
                .any(|r| r["retiredAt"].is_null() && r["id"] != task["repositoryId"])
        {
            panel = panel.child(hint(format!(
                "Using another project sets accepted plan {version} aside. Queue will start scoping again unless you accept a new plan version."
            )));
        }
        for repo in repos.iter().filter(|r| r["retiredAt"].is_null()) {
            let id = string(repo, "id");
            let task_id = task_id.clone();
            let revision = task["revision"].clone();
            panel = panel
                .child(crate::accessibility::readable(
                    format!("repository-choice-{id}"),
                    format!("{} · {}", string(repo, "name"), string(repo, "path")),
                ))
                .child(
                    Button::new(SharedString::from(format!("assign-{id}")))
                        .label(if task["repositoryId"] == id {
                            "Selected project"
                        } else {
                            "Use project"
                        })
                        .selected(task["repositoryId"] == id)
                        .disabled(self.blocked() || current.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                format!("/tasks/{task_id}/repository"),
                                json!({"repositoryId":id,"revision":revision}),
                                false,
                                cx,
                            )
                        })),
                );
        }
        if repos.iter().all(|r| !r["retiredAt"].is_null()) {
            panel = panel
                .child(hint("No active projects. Add one to choose a project."))
                .child(
                    Button::new("plan-add-repository")
                        .label("Add a project")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel = Panel::Repositories;
                            cx.notify();
                        })),
                );
        } else {
            panel = panel.children(super::blockers::line(
                "target-blockers",
                "change the target",
                &super::blockers::change_target(current.is_some()),
            ));
        }
        if retired {
            panel = panel.child(hint(
                "Selected project is retired. Choose an active project before you select Queue run.",
            ));
        }
        let roles: Vec<_> = records(&self.workspace, "roles")
            .into_iter()
            .filter(|r| r["enabled"] == true)
            .collect();
        let selection = self.coordinator_role(&task_id, current);
        self.planning
            .coordinator_roles
            .insert(task_id.clone(), selection.clone());
        let editable = current.is_none_or(|r| r.get("position").is_some());
        let mut choices = div().id("launch-role-choices").flex().flex_wrap().gap_2();
        for role in &roles {
            let id = string(role, "id");
            let task_id = task_id.clone();
            choices = choices.child(
                Button::new(SharedString::from(format!("launch-role-{id}")))
                    .label(string(role, "name"))
                    .selected(selection == id)
                    .disabled(self.busy || !editable)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.planning
                            .coordinator_roles
                            .insert(task_id.clone(), id.clone());
                        cx.notify();
                    })),
            );
        }
        let selected_available = roles.iter().any(|r| r["id"] == selection);
        panel = panel
            .child(crate::accessibility::text("Role for queued coordinator"))
            .child(choices)
            .child(hint("The coordinator selects specialists for the ticket. After queuing, use Add a specialist in Conversation to request a particular specialty."))
            .child(
                Button::new("plan-manage-agent-roles")
                    .label("Configure agent roles")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Panel::Roles;
                        cx.notify();
                    })),
            );
        panel = panel.child(self.role_model_preview(&selection));
        if !selected_available {
            panel=panel.child(hint("The selected role is unavailable. Choose an enabled role before saving launch details."));
        }
        let mut blockers: Vec<String> = Vec::new();
        if target.is_none() {
            blockers.push("choose a project".into());
        }
        if retired {
            blockers.push("choose an active project".into());
        }
        if let Some(source) = super::blockers::reacceptance(task) {
            blockers.push(super::blockers::accept_source(source));
        } else if self.brief_dirty(task, cx) {
            blockers.push("save or discard brief changes".into());
        }
        if current.is_none() && !blockers.is_empty() {
            panel = panel.child(hint(format!("To queue: {}.", blockers.join(", "))));
        }
        if let Some(run) = current {
            panel = panel.child(hint(format!(
                "Captured brief revision {} · repository mode {}",
                run["taskRevision"],
                run["launch"]["repository"]["mode"]
                    .as_str()
                    .unwrap_or("new")
            )));
            if editable {
                let queued = records(&self.workspace["queue"], "plans")
                    .into_iter()
                    .find(|p| p["id"] == run["id"])
                    .unwrap_or(Value::Null);
                panel = panel.child(hint(format!(
                    "Queued plan, not running. {} · Hold: {}",
                    if run["launch"].is_object() {
                        "Launch details saved"
                    } else {
                        "Launch details missing"
                    },
                    hold_reason(&queued, &self.workspace)
                )));
            } else {
                panel = panel.child(hint(
                    "This run has started. Its captured launch team cannot be edited here.",
                ));
            }
        } else {
            panel=panel.child(hint("Queue requests scoping if a current accepted plan is missing. After plan acceptance, coding starts when its launch is valid and a coding slot is available."));
        }
        if self.workspace["capabilities"]["execution"] != true {
            panel = panel.child(hint(
                "Runner not configured. You can save a queued plan; no agent will start.",
            ));
        }
        let snapshot = current
            .map(|r| r["taskSnapshot"].clone())
            .unwrap_or_else(|| task.clone());
        let mut launch = current
            .and_then(|r| r.get("launch"))
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| self.coordinator_launch(&snapshot, &selection));
        if let Some(team) = launch["team"].as_array_mut() {
            for member in team.iter_mut().filter(|m| m["coordinator"] == true) {
                member["roleId"] = json!(selection);
            }
            for member in team {
                let role = string(member, "roleId");
                self.capture_model_preview(member, &role);
            }
        }
        let path = current
            .map(|r| format!("/runs/{}/launch", string(r, "id")))
            .unwrap_or_else(|| format!("/tasks/{task_id}/runs"));
        let playbook = self
            .planning
            .playbooks
            .get(&task_id)
            .cloned()
            .unwrap_or_default();
        if current.is_none() {
            panel = panel.child(crate::accessibility::text("Playbook (optional)"));
            let mut choices = records(&self.workspace, "playbooks");
            choices.insert(0, json!({"id":"","name":"Coordinator may propose one"}));
            for p in choices {
                let pid = string(&p, "id");
                let selected = playbook == pid;
                let task = task_id.clone();
                panel = panel.child(
                    Button::new(SharedString::from(format!("queue-playbook-{pid}")))
                        .label(string(&p, "name"))
                        .selected(selected)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.planning.playbooks.insert(task.clone(), pid.clone());
                            cx.notify();
                        })),
                );
            }
        }
        let mut body = if current.is_some() {
            json!({"launch":launch})
        } else {
            json!({"revision":task["revision"],"launch":launch})
        };
        if current.is_none() && !playbook.is_empty() {
            body["playbookId"] = json!(playbook);
        }
        panel.child(
            Button::new("queue-plan")
                .primary()
                .label(if current.is_some() {
                    "Save launch details"
                } else {
                    "Queue run"
                })
                .disabled(
                    self.blocked()
                        || !editable
                        || !selected_available
                        || (current.is_none() && !blockers.is_empty()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(path.clone(), body.clone(), true, cx)
                })),
        )
    }
    pub(super) fn queue_panel(&self, cx: &mut Context<Self>) -> Div {
        let queue = &self.workspace["queue"];
        let plans = records(queue, "plans");
        let mut panel=section("Coding queue").child(hint("Local queue order is independent of Jira order. A queued plan has not started its agents."));
        if queue["slot"].is_object() {
            panel = panel.child(hint(format!(
                "Coding slot: {} · {}",
                string(&queue["slot"], "key"),
                string(&queue["slot"], "reason")
            )));
        }
        if let Some(intakes) = self.workspace["intake"].as_object() {
            for (task_id, view) in intakes.iter().filter(|(_, v)| v["intake"].is_object()) {
                let id = task_id.clone();
                let title = records(&self.workspace, "tasks")
                    .into_iter()
                    .find(|t| t["id"] == *task_id)
                    .map(|t| string(&t, "title"))
                    .unwrap_or_else(|| "Queued idea".into());
                panel = panel.child(
                    section(format!("{title} · {}", string(view, "state")))
                        .id(SharedString::from(format!("queued-intake-{id}")))
                        .child(hint("Intake has no coding assignment."))
                        .child(
                            Button::new(SharedString::from(format!("open-intake-{id}")))
                                .label("Review intake")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected = Some(id.clone());
                                    this.panel = Panel::Ticket;
                                    this.tab = "Brief";
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        if plans.is_empty() {
            panel = panel.child(hint("No coding plans queued."));
        }
        for (index, plan) in plans.iter().enumerate() {
            let id = string(plan, "id");
            let key = string(&plan["taskSnapshot"], "key");
            let task_id = string(plan, "taskId");
            let mut row = section(format!(
                "{key} · {}",
                string(&plan["taskSnapshot"], "title")
            ))
            .id(SharedString::from(format!("queue-row-{id}")))
            .child(hint(format!(
                "{} · Hold: {}",
                if plan["launch"].is_object() {
                    "Launch details saved"
                } else {
                    "Launch details missing"
                },
                hold_reason(plan, &self.workspace)
            )));
            let repository = records(&self.workspace, "repositories")
                .into_iter()
                .find(|r| r["id"] == plan["taskSnapshot"]["repositoryId"]);
            let coordinator = records(&plan["launch"], "team")
                .into_iter()
                .find(|m| m["coordinator"] == true);
            row = row.child(hint(format!(
                "Repository: {} · Coordinator: {}",
                repository
                    .as_ref()
                    .map(|r| string(r, "name"))
                    .unwrap_or_else(|| "unavailable".into()),
                coordinator
                    .as_ref()
                    .map(|r| string(r, "roleId"))
                    .unwrap_or_else(|| "not selected".into())
            )));
            row = row.child(
                Button::new(SharedString::from(format!("open-plan-{id}")))
                    .label("Open ticket")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(task_id.clone());
                        this.panel = Panel::Ticket;
                        this.tab = "Brief";
                        cx.notify();
                    })),
            );
            let mut controls = div().flex().flex_wrap().gap_2();
            for (direction, label, other) in [
                ("up", "Move up", index.checked_sub(1)),
                (
                    "down",
                    "Move down",
                    (index + 1 < plans.len()).then_some(index + 1),
                ),
            ] {
                let mut ids: Vec<_> = plans.iter().map(|r| r["id"].clone()).collect();
                if let Some(other) = other {
                    ids.swap(index, other);
                }
                let body = json!({"expectedRevision":queue["revision"],"runIds":ids});
                controls = controls.child(
                    Button::new(SharedString::from(format!("queue-{direction}-{id}")))
                        .label(label)
                        .disabled(self.blocked() || other.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send("/queue/order".into(), body.clone(), true, cx)
                        })),
                );
            }
            controls = controls.child(
                Button::new(SharedString::from(format!("remove-{id}")))
                    .label("Cancel queued plan…")
                    .disabled(self.blocked())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_queued_plan_cancel(&id, window, cx)
                    })),
            );
            panel = panel.child(row.child(controls));
        }
        panel
    }
    pub(super) fn dependencies(&self, task: &Value, cx: &mut Context<Self>) -> Div {
        let id = string(task, "id");
        let depends = records(task, "dependsOn");
        let tasks = records(&self.workspace, "tasks");
        let revision = task["dependencyRevision"].as_u64().unwrap_or(0);
        let mut panel=section("Ticket dependencies").child(hint("Each dependency waits for a provider-verified merge. Declarations survive source refresh and apply to queued plans; running work is unchanged."));
        if depends.is_empty() {
            panel = panel.child(hint("No dependencies declared."));
        }
        for dep in &depends {
            let dep_id = dep.as_str().unwrap_or("");
            let target = tasks.iter().find(|t| t["id"] == *dep);
            let state = &self.workspace["dependencyStates"][dep_id];
            let next: Vec<_> = depends.iter().filter(|d| *d != dep).cloned().collect();
            let task_id = id.clone();
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("dependency-row-{dep_id}")))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(hint(format!(
                        "{} · {} · {}",
                        target
                            .map(|t| string(t, "key"))
                            .unwrap_or_else(|| dep_id.into()),
                        state["state"].as_str().unwrap_or("unknown"),
                        state["reason"]
                            .as_str()
                            .unwrap_or("Awaiting delivery evidence")
                    )))
                    .child(
                        Button::new(SharedString::from(format!("remove-dependency-{dep_id}")))
                            .label("Remove dependency")
                            .disabled(self.blocked())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(
                                    format!("/tasks/{task_id}/dependencies"),
                                    json!({"dependencyRevision":revision,"dependsOn":next}),
                                    true,
                                    cx,
                                )
                            })),
                    ),
            );
        }
        let mut choices = div().flex().flex_col().gap_2();
        for target in tasks
            .iter()
            .filter(|t| t["id"] != task["id"] && !depends.contains(&t["id"]))
        {
            let target_id = string(target, "id");
            let mut next = depends.clone();
            next.push(target["id"].clone());
            let task_id = id.clone();
            choices = choices
                .child(crate::accessibility::readable(
                    format!("dependency-choice-{target_id}"),
                    string(target, "title"),
                ))
                .child(
                    Button::new(SharedString::from(format!("add-dependency-{target_id}")))
                        .label(format!("Depend on {}", string(target, "key")))
                        .disabled(self.blocked() || depends.len() >= 12)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                format!("/tasks/{task_id}/dependencies"),
                                json!({"dependencyRevision":revision,"dependsOn":next}),
                                true,
                                cx,
                            )
                        })),
                );
        }
        panel.child(choices)
    }
}

#[cfg(test)]
mod tests;
