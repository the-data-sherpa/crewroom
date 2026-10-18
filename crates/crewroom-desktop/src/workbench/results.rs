//! Revision-scoped evidence. Displayed receipts never substitute for current assessment.
mod evidence;
pub(super) use evidence::check_label;
mod publication;
mod recovery;
#[cfg(test)]
mod tests;
use super::*;
use gpui_kit::Stateful;
pub(super) struct Detail {
    pub path: String,
    pub label: String,
    pub result: Option<Result<Value, String>>,
    pub content: Option<Entity<TextareaState>>,
    /// The control that opened the evidence; Close returns focus there.
    pub return_focus: Option<gpui_kit::FocusHandle>,
    /// A new opening scrolls the evidence into view, then focuses its content once loaded.
    reveal: bool,
    scrolled: bool,
    anchor: gpui_kit::ScrollAnchor,
}
pub(super) fn value(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| {
        if v.is_null() {
            "Unavailable".into()
        } else {
            v.to_string()
        }
    })
}
fn text_lines(v: &Value) -> String {
    v.as_array()
        .map(|a| a.iter().map(value).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}
pub(super) fn short(v: &Value) -> String {
    let s = value(v);
    s.chars().take(12).collect()
}
/// The one short revision form shared by the facts bar, Results and PR delivery.
pub(super) fn revision_label(candidates: &[Value], head: &Value) -> String {
    match candidates
        .iter()
        .position(|c| c["revision"]["head"] == *head)
    {
        Some(i) => format!("Revision {} · {}", i + 1, short(head)),
        None => format!("Unlisted revision · {}", short(head)),
    }
}
/// The candidate the readiness assessment covers, named in the short form.
pub(super) fn assessed_revision(result: &Value) -> Option<String> {
    let candidates = records(result, "candidates");
    let head = &result["assessment"]["candidate"]["revision"]["head"];
    let head = if head.is_null() {
        &candidates.last()?["revision"]["head"]
    } else {
        head
    };
    Some(revision_label(&candidates, head))
}
impl Workbench {
    /// Opening from a button remembers it as the focus return target; Reload keeps the opener.
    pub(super) fn load_detail(
        &mut self,
        path: String,
        label: String,
        reload: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected {
            return;
        }
        let return_focus = match &self.observation.detail {
            Some(detail) if reload => detail.return_focus.clone(),
            _ => window.focused(cx),
        };
        let result = self
            .client
            .as_ref()
            .ok_or_else(|| "Service connection unavailable".into())
            .and_then(|c| c.read(path.clone()));
        self.observation.detail = Some(Detail {
            path,
            label,
            result: result.err().map(Err),
            content: None,
            return_focus,
            reveal: !reload,
            scrolled: reload,
            anchor: gpui_kit::ScrollAnchor::for_handle(self.details_scroll.clone()),
        });
        cx.notify();
    }
    pub(super) fn artifact_button(
        &self,
        run: &str,
        artifact: &Value,
        label: &str,
        id: String,
        cx: &Context<Self>,
    ) -> Div {
        let Some(artifact_id) = artifact["id"].as_str() else {
            return hint("Artifact is not recorded.");
        };
        let path = format!("/runs/{run}/result/artifacts/{artifact_id}");
        let label = label.to_owned();
        div().child(
            Button::new(SharedString::from(id))
                .label(label.clone())
                .disabled(!self.connected)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.load_detail(path.clone(), label.clone(), false, window, cx)
                })),
        )
    }
    /// Saved evidence renders after the panel's other records, so opening it reveals it.
    pub(super) fn detail_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let shown = div().id("saved-evidence");
        let Some(detail) = &mut self.observation.detail else {
            return shown;
        };
        if !detail.scrolled {
            detail.scrolled = true;
            detail.anchor.scroll_to(window, cx);
        }
        let mut panel=section(detail.label.clone()).child(hint("Read-only saved evidence. It covers its named revision and does not establish current readiness."));
        match &detail.result {
            None => panel = panel.child(hint("Loading saved content…")),
            Some(Err(error)) => {
                detail.reveal = false;
                panel = panel.child(hint(error.clone()))
            }
            Some(Ok(data)) => {
                let body = data["content"]
                    .as_str()
                    .or_else(|| data["patch"].as_str())
                    .unwrap_or("No file changes from the recorded base.")
                    .to_owned();
                if detail.content.is_none() {
                    detail.content = Some(
                        cx.new(|cx| TextareaState::new(window, cx).default_value(body.clone())),
                    );
                }
                // Focus follows the opening only if the operator has not moved on meanwhile.
                if std::mem::take(&mut detail.reveal) && window.focused(cx) == detail.return_focus {
                    let content = detail.content.as_ref().unwrap();
                    content.update(cx, |content, cx| content.focus(window, cx));
                }
                if let Some(hash) = data["sha256"].as_str() {
                    panel = panel.child(hint(format!("Artifact SHA-256: {hash}")));
                }
                panel = panel
                    .child(
                        Textarea::new(detail.content.as_ref().unwrap())
                            .readonly(true)
                            .accessibility_id("saved-evidence-content")
                            .aria_label("Saved evidence content, read only")
                            .h(px(360.)),
                    )
                    .child(
                        Button::new("copy-saved-evidence")
                            .label("Copy saved content")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    body.clone(),
                                ))
                            }),
                    );
            }
        }
        let path = detail.path.clone();
        let label = detail.label.clone();
        let anchor = detail.anchor.clone();
        shown.anchor_scroll(Some(anchor)).child(
            panel.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("reload-evidence")
                            .label("Reload saved content")
                            .disabled(!self.connected)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.load_detail(path.clone(), label.clone(), true, window, cx)
                            })),
                    )
                    .child(
                        Button::new("close-evidence")
                            .label("Close saved content")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let opener = this.observation.detail.take();
                                if let Some(focus) = opener.and_then(|d| d.return_focus) {
                                    window.focus(&focus, cx);
                                }
                                cx.notify();
                            })),
                    ),
            ),
        )
    }
    pub(super) fn results_panel(
        &mut self,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if ["explore", "grill"].contains(&run["purpose"].as_str().unwrap_or("coding")) {
            return section("Refinement evidence").child(hint("Refinement proposals and saved notes are in Brief and History. This run has no coding or Jira delivery."));
        }
        let result = self.observation.result.clone();
        let mut panel = div().flex().flex_col().gap_4();
        if let Some(error) = self.observation.errors.get("result") {
            panel = panel.child(hint(format!("Evidence unavailable: {error}")));
        }
        if !self.observation.result_fresh {
            panel = panel.child(hint(
                "Evidence observation is not current. Retained records may be stale.",
            ));
        }
        if result.is_null() {
            // An error already explains the missing evidence; waiting would contradict it.
            if !self.observation.errors.contains_key("result") {
                panel = panel.child(hint("Waiting for saved evidence…"));
            }
            return panel;
        }
        if run["purpose"] == "research" {
            return panel
                .child(self.research_evidence(run, &result, cx))
                .child(self.usage_panel(&result["usage"]))
                .child(self.detail_panel(window, cx));
        }
        let id = string(run, "id");
        panel = panel.child(
            section("Change and evidence")
                .child(crate::accessibility::text({
                    let verdict =
                        if self.observation.result_fresh && result["assessment"]["ready"] == true {
                            "Locally validated"
                        } else {
                            "Validation incomplete"
                        };
                    match assessed_revision(&result) {
                        Some(revision) => format!("{verdict} · {revision}"),
                        None => verdict.into(),
                    }
                }))
                .child(hint(string(&result["publication"], "reason")))
                .child(hint(text_lines(&result["assessment"]["blockers"]))),
        );
        if let Some(repo) = result.get("repository").filter(|r| r.is_object()) {
            let mut row = section("Run repository · captured at preparation")
                .child(crate::accessibility::text(string(repo, "name")))
                .child(hint(string(repo, "path")))
                .child(hint(format!(
                    "Target: {} · prefix {}",
                    value(&repo["targetBranch"]),
                    value(&repo["branchPrefix"])
                )));
            if let Some(current) = records(&self.workspace, "repositories")
                .iter()
                .find(|r| r["id"] == repo["id"])
            {
                if !current["retiredAt"].is_null() {
                    row=row.child(hint("This repository is now retired. The run retains its original configuration."));
                }
                if ["name", "path", "targetBranch", "branchPrefix", "checks"]
                    .iter()
                    .any(|k| current[*k] != repo[*k])
                {
                    row=row.child(hint("Current repository settings differ. These are the settings captured for this run."));
                }
            }
            panel = panel.child(row);
        }
        panel = panel
            .child(self.publication_panel(run, &result, cx))
            .child(self.recovery_panel(run, &result, window, cx));
        let candidates = records(&result, "candidates");
        let selected = self
            .observation
            .candidate
            .as_ref()
            .and_then(|id| candidates.iter().find(|c| c["id"] == *id))
            .or(candidates.last())
            .cloned();
        let mut choices = div().flex().flex_wrap().gap_2();
        for (i, candidate) in candidates.iter().enumerate() {
            let cid = string(candidate, "id");
            choices = choices.child(
                Button::new(SharedString::from(format!("candidate-{cid}")))
                    .label(format!(
                        "Revision {} · {}{}",
                        i + 1,
                        short(&candidate["revision"]["head"]),
                        if Some(candidate) == candidates.last() {
                            " · Latest"
                        } else {
                            ""
                        }
                    ))
                    .selected(selected.as_ref().is_some_and(|c| c["id"] == cid))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.observation.candidate = Some(cid.clone());
                        this.observation.detail = None;
                        cx.notify();
                    })),
            );
        }
        panel = panel.child(choices);
        if let Some(candidate) = selected {
            if Some(&candidate) != candidates.last() {
                panel=panel.child(hint("Historical candidate. Its checks and review do not establish readiness for a newer revision."));
            }
            panel = panel.child(self.candidate_evidence(&id, &candidate, &result, cx));
        } else {
            panel = panel.child(hint("No candidate revision has been recorded."));
        }
        panel
            .child(self.usage_panel(&result["usage"]))
            .child(self.detail_panel(window, cx))
    }
    pub(super) fn usage_panel(&self, usage: &Value) -> Div {
        let mut panel = section("Recorded usage");
        if usage.is_null() {
            return panel.child(hint(
                "Usage observation is unavailable. Missing reports are not zero usage.",
            ));
        }
        panel = panel
            .child(usage_numbers(&usage["total"]))
            .child(hint(string(usage, "note")))
            .child(hint(format!("Source: {}", string(usage, "source"))));
        for row in records(usage, "rows") {
            panel = panel.child(
                section(format!(
                    "{} · attempt {} · {}",
                    value(&row["name"]),
                    value(&row["generation"]),
                    value(&row["model"])
                ))
                .id(SharedString::from(format!(
                    "usage-attempt-{}",
                    value(&row["attemptId"])
                )))
                .child(usage_numbers(&row))
                .child(hint(text_lines(&row["unavailableReasons"]))),
            );
        }
        panel
    }
}
pub(super) fn usage_numbers(row: &Value) -> Div {
    let label = if row["awaitingStart"] == true {
        "Awaiting start"
    } else {
        match row["state"].as_str() {
            Some("actual") => "Provider-reported usage",
            Some("partial") => "Partial provider usage",
            _ => "Usage unavailable",
        }
    };
    let mut view = div().child(crate::accessibility::text(label));
    if !row["tokens"].is_null() {
        view = view.child(hint(format!(
            "Input {} · output {} · cache read {} · cache write {} · reasoning {} · total {}",
            value(&row["tokens"]["input"]),
            value(&row["tokens"]["output"]),
            value(&row["tokens"]["cacheRead"]),
            value(&row["tokens"]["cacheWrite"]),
            value(&row["tokens"]["reasoning"]),
            value(&row["tokens"]["total"])
        )));
    }
    let partial = row["state"] != "actual" || row["costResponses"] != row["responses"];
    let requests = match row["requestCounts"].as_str() {
        Some("unavailable") => "Provider request counts unavailable".into(),
        state => format!(
            "Reported responses: {} / {} provider requests{}",
            value(&row["actualResponses"]),
            value(&row["providerRequests"]),
            if state == Some("partial") {
                " · partial counts; native requests unavailable"
            } else {
                ""
            }
        ),
    };
    view.child(hint(format!(
        "{requests} · runner cost estimate: {}{} · provider billing unavailable",
        row["costUsd"]
            .as_f64()
            .map(|n| format!("${n:.6}"))
            .unwrap_or_else(|| "unavailable".into()),
        if partial && row["costUsd"].is_number() {
            " · partial"
        } else {
            ""
        }
    )))
    .child(hint(
        if row["costBasis"] == "cumulative-list-price-estimate" {
            "Includes cumulative list-price estimates; subscription charges unavailable".into()
        } else if row["requestCounts"] == "unavailable" {
            "Response cost coverage unavailable".into()
        } else {
            format!(
                "Runner pricing reports: {} / {} responses",
                value(&row["costResponses"]),
                value(&row["responses"])
            )
        },
    ))
}
