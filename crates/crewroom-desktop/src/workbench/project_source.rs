//! Settings → Repositories → Ticket source (FOR-47): the source suggested from
//! a project's publish remote, one explicit confirmation, a "source no longer
//! matches remote" warning, and a change that names the work blocking it. The
//! local service reads the remote and decides; this view never switches a
//! source by itself.
use super::*;

/// Per-repository inputs: the GitHub repository (prefilled from the
/// suggestion) and a Jira site the operator types when it is not listed.
pub(super) struct SourceInputs {
    pub(super) repository: Entity<InputState>,
    pub(super) site: Entity<InputState>,
}
impl SourceInputs {
    fn new(view: &Value, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let repository = view["saved"]["repository"]
            .as_str()
            .or_else(|| view["suggestion"]["repository"].as_str())
            .unwrap_or("")
            .to_owned();
        let site = view["saved"]["site"].as_str().unwrap_or("").to_owned();
        Self {
            repository: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("owner/repository")
                    .default_value(repository)
            }),
            site: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("https://your-site.atlassian.net")
                    .default_value(site)
            }),
        }
    }
}

/// Completion text for a confirmation; the row below carries the detail.
pub(super) fn notice(path: &str) -> Option<String> {
    (path.starts_with("/repositories/") && path.ends_with("/ticket-source")).then(|| {
        "Ticket source saved for this project. Tickets imported earlier keep their source.".into()
    })
}

fn line(id: String, value: impl Into<SharedString>, color: u32) -> Div {
    div()
        .text_sm()
        .text_color(rgb(color))
        .child(crate::accessibility::named(id, value))
}

impl Workbench {
    /// The ticket-source block shown in an active repository's row.
    pub(super) fn ticket_source_block(
        &mut self,
        repository: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = string(repository, "id");
        let view = &repository["projectSource"];
        let mut block = div().flex().flex_col().gap_2();
        if view.is_null() {
            return block.child(line(
                format!("ticket-source-{id}-state"),
                "Ticket source unavailable until the service reads this repository.",
                0x9eacb9,
            ));
        }
        let state = view["state"].as_str().unwrap_or("");
        block = block
            .child(line(
                format!("ticket-source-{id}-remote"),
                format!(
                    "Publish remote: {}",
                    view["remote"]["label"].as_str().unwrap_or("unknown")
                ),
                0x9eacb9,
            ))
            .child(line(
                format!("ticket-source-{id}-state"),
                view["label"].as_str().unwrap_or("").to_owned(),
                0x9eacb9,
            ));
        if let Some(warning) = view["warning"].as_str() {
            block = block.child(line(
                format!("ticket-source-{id}-warning"),
                warning.to_owned(),
                0xf0b45b,
            ));
        }
        let blocked = view["changeBlocked"].as_str();
        if let Some(reason) = blocked {
            block = block.child(line(
                format!("ticket-source-{id}-blocked"),
                reason.to_owned(),
                0xf0b45b,
            ));
        }
        let open = state != "confirmed" || self.source_open.contains(&id);
        if !open {
            let target = id.clone();
            return block.child(
                Button::new(SharedString::from(format!("change-ticket-source-{id}")))
                    .label("Change ticket source")
                    .disabled(self.blocked() || blocked.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.source_open.insert(target.clone());
                        cx.notify();
                    })),
            );
        }
        if !self.source_inputs.contains_key(&id) {
            let inputs = SourceInputs::new(view, window, cx);
            self.source_inputs.insert(id.clone(), inputs);
        }
        let inputs = &self.source_inputs[&id];
        let disabled = self.blocked() || blocked.is_some();
        let suggested = view["suggestion"]["tickets"].as_str();
        let path = format!("/repositories/{id}/ticket-source");
        let github_path = path.clone();
        let target = id.clone();
        block = block
            .child(crate::accessibility::text("GitHub repository for issues"))
            .child(
                Input::new(&inputs.repository)
                    .id(SharedString::from(format!("ticket-source-{id}-repository")))
                    .disabled(disabled),
            )
            .child({
                let button = Button::new(SharedString::from(format!("confirm-github-source-{id}")))
                    .label("Confirm GitHub Issues")
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let repository = this.source_inputs[&target]
                            .repository
                            .read(cx)
                            .value()
                            .trim()
                            .to_owned();
                        if repository.is_empty() {
                            this.fail_at(
                                "Enter the GitHub repository as owner/name.",
                                feedback::Place::Top,
                            );
                            cx.notify();
                            return;
                        }
                        this.send(
                            github_path.clone(),
                            json!({"tickets":"github","repository":repository}),
                            false,
                            cx,
                        );
                    }));
                if suggested == Some("github") {
                    button.primary()
                } else {
                    button
                }
            });
        let sites: Vec<String> = view["jiraSites"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        block = block.child(crate::accessibility::text(if sites.is_empty() {
            "Jira site (no Jira site is known yet; enter its address)"
        } else {
            "Jira site: pick one, or enter another address"
        }));
        let mut picker = div().flex().flex_wrap().gap_2();
        for (index, site) in sites.into_iter().enumerate() {
            let site_path = path.clone();
            let label = format!("Confirm Jira {site}");
            picker = picker.child(
                Button::new(SharedString::from(format!(
                    "confirm-jira-site-{id}-{index}"
                )))
                .label(label)
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(
                        site_path.clone(),
                        json!({"tickets":"jira","site":site}),
                        false,
                        cx,
                    );
                })),
            );
        }
        let target = id.clone();
        let typed_path = path;
        block
            .child(picker)
            .child(
                Input::new(&inputs.site)
                    .id(SharedString::from(format!("ticket-source-{id}-site")))
                    .disabled(disabled),
            )
            .child(
                Button::new(SharedString::from(format!("confirm-jira-source-{id}")))
                    .label("Confirm Jira site")
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let site = this.source_inputs[&target]
                            .site
                            .read(cx)
                            .value()
                            .trim()
                            .to_owned();
                        if site.is_empty() {
                            this.fail_at(
                                "Pick or enter the Jira site; it cannot be inferred from the remote.",
                                feedback::Place::Top,
                            );
                            cx.notify();
                            return;
                        }
                        this.send(
                            typed_path.clone(),
                            json!({"tickets":"jira","site":site}),
                            false,
                            cx,
                        );
                    })),
            )
    }
}
