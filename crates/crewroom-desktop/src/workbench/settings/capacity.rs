//! FOR-76: per-project run capacity and the optional workspace ceiling.
//! The service owns admission; these controls only record the limits.
use super::*;

const MAX: u64 = 16;

pub(super) fn project_view<'a>(workspace: &'a Value, repository: &str) -> Option<&'a Value> {
    workspace["queue"]["capacity"]["projects"]
        .as_array()?
        .iter()
        .find(|p| p["repositoryId"] == repository)
}

/// "1 of 2 running" style occupancy for one project.
pub(super) fn project_summary(workspace: &Value, repository: &str) -> String {
    match project_view(workspace, repository) {
        Some(view) => format!(
            "Run capacity: {} · {} of {} running",
            view["capacity"].as_u64().unwrap_or(1),
            view["occupied"].as_u64().unwrap_or(0),
            view["capacity"].as_u64().unwrap_or(1)
        ),
        None => "Run capacity: unavailable until the workflow service reports it".into(),
    }
}

pub(super) fn ceiling_summary(workspace: &Value) -> String {
    let capacity = &workspace["queue"]["capacity"];
    if !capacity.is_object() {
        return "Workspace ceiling: unavailable until the workflow service reports it".into();
    }
    let occupied = capacity["occupied"].as_u64().unwrap_or(0);
    match capacity["workspaceCeiling"].as_u64() {
        Some(ceiling) => format!("Workspace ceiling: {occupied} of {ceiling} running"),
        None => format!("Workspace ceiling: none · {occupied} running across projects"),
    }
}

impl Workbench {
    pub(super) fn capacity_block(&self, repo: &Value, cx: &mut Context<Self>) -> Div {
        let target = string(repo, "id");
        let current = project_view(&self.workspace, &target)
            .and_then(|v| v["capacity"].as_u64())
            .unwrap_or(1);
        let known = project_view(&self.workspace, &target).is_some();
        let change = |label: &str, id: String, next: u64, enabled: bool| {
            let target = target.clone();
            Button::new(SharedString::from(id))
                .label(label.to_owned())
                .disabled(self.blocked() || !known || !enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(
                        "/execution-capacity".into(),
                        json!({"repositoryId":target,"capacity":next}),
                        true,
                        cx,
                    )
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(named_hint(
                format!("repository-capacity-{target}"),
                project_summary(&self.workspace, &target),
            ))
            .child(hint(
                "Tickets started while this project is full wait in the queue and start automatically when a slot frees.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(change(
                        "Fewer parallel runs",
                        format!("capacity-down-{target}"),
                        current.saturating_sub(1).max(1),
                        current > 1,
                    ))
                    .child(change(
                        "More parallel runs",
                        format!("capacity-up-{target}"),
                        (current + 1).min(MAX),
                        current < MAX,
                    )),
            )
    }
    pub(super) fn ceiling_block(&self, cx: &mut Context<Self>) -> Div {
        let capacity = &self.workspace["queue"]["capacity"];
        let known = capacity.is_object();
        let ceiling = capacity["workspaceCeiling"].as_u64();
        let send = |id: &'static str, label: &'static str, value: Value, enabled: bool| {
            Button::new(id)
                .label(label)
                .disabled(self.blocked() || !known || !enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(
                        "/execution-capacity".into(),
                        json!({"workspaceCeiling":value.clone()}),
                        true,
                        cx,
                    )
                }))
        };
        let row = div().flex().flex_wrap().gap_2();
        let row = match ceiling {
            None => row.child(send(
                "ceiling-set",
                "Limit all projects",
                json!(capacity["occupied"].as_u64().unwrap_or(0).clamp(1, MAX)),
                true,
            )),
            Some(n) => row
                .child(send(
                    "ceiling-down",
                    "Lower ceiling",
                    json!(n.saturating_sub(1).max(1)),
                    n > 1,
                ))
                .child(send(
                    "ceiling-up",
                    "Raise ceiling",
                    json!((n + 1).min(MAX)),
                    n < MAX,
                ))
                .child(send("ceiling-remove", "Remove ceiling", Value::Null, true)),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(named_hint("workspace-ceiling", ceiling_summary(&self.workspace)))
            .child(hint("An optional limit on runs across every project. Lowering a limit never stops running work."))
            .child(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_report_occupancy_and_an_unavailable_service_truthfully() {
        let workspace = json!({"queue":{"capacity":{"workspaceCeiling":null,"occupied":2,"projects":[{"repositoryId":"a","name":"Alpha","capacity":2,"occupied":1}]}}});
        assert_eq!(
            project_summary(&workspace, "a"),
            "Run capacity: 2 · 1 of 2 running"
        );
        assert_eq!(
            ceiling_summary(&workspace),
            "Workspace ceiling: none · 2 running across projects"
        );
        let limited =
            json!({"queue":{"capacity":{"workspaceCeiling":3,"occupied":2,"projects":[]}}});
        assert_eq!(
            ceiling_summary(&limited),
            "Workspace ceiling: 2 of 3 running"
        );
        let offline = json!({});
        assert!(project_summary(&offline, "a").contains("unavailable"));
        assert!(ceiling_summary(&offline).contains("unavailable"));
    }
}
