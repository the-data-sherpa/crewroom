//! Optional instruction evidence. Catalog saves and learning decisions remain authoritative.
use super::*;
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct Comparisons {
    pub open: bool,
    pub return_panel: Option<Panel>,
    target: Option<(String, String)>,
    draft: Option<Value>,
    source: String,
    preflight: Option<Value>,
    selected: Option<Value>,
    history: Vec<Value>,
    pending_reads: HashSet<String>,
    expanded: HashSet<String>,
    start_after_create: bool,
    error: String,
}

/// Catalog metadata is not instruction content. The service rechecks this binding on adoption.
fn content(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        for key in [
            "revision",
            "history",
            "updatedAt",
            "createdAt",
            "provenance",
            "learningProposalId",
            "draftProposalId",
        ] {
            object.remove(key);
        }
    }
    if value.get("instructions").is_some() {
        if value.get("principles").is_none() {
            value["principles"] = json!([]);
        }
        if value.get("job").is_none() {
            value["job"] = Value::Null;
        }
    }
    value
}
fn target_of(value: &Value) -> &Value {
    &value["manifest"]["target"]
}
fn terminal(state: &str) -> bool {
    matches!(
        state,
        "completed" | "incomplete" | "unavailable" | "cancelled"
    )
}
/// Comparisons are found by the revision they cover; the ID stays secondary.
fn history_label(value: &Value) -> String {
    let at = string(value, "createdAt");
    let at = match (at.get(..10), at.get(11..16)) {
        (Some(day), Some(time)) if at.as_bytes()[10] == b'T' => format!("{day} {time}"),
        _ if at.is_empty() => "date unavailable".into(),
        _ => at,
    };
    format!(
        "Revision {} → proposed · {} · {at}",
        target_of(value)["baseRevision"]
            .as_u64()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "unavailable".into()),
        string(value, "state")
    )
}
fn coverage(value: &Value) -> String {
    let cells = records(value, "cells");
    let evaluated = cells
        .iter()
        .filter(|c| c["result"]["observed"] == true)
        .count();
    let unfinished = cells.iter().filter(|c| !c["result"].is_object()).count();
    let inconclusive = cells
        .iter()
        .filter(|cell| cell["result"]["observed"] == true)
        .filter(|cell| {
            let scores = &cell["result"]["scores"];
            scores["classification"] == "inconclusive"
                || [
                    "sourceCorrectness",
                    "completionClaim",
                    "falseCompletion",
                    "requiredChecksFresh",
                    "surfaceExercised",
                ]
                .iter()
                .any(|metric| scores[metric].as_bool().is_none())
        })
        .count();
    let assessment = if inconclusive > 0 {
        format!(
            " Inconclusive assessment: {inconclusive} observed cells have an inconclusive classification or unknown required metrics."
        )
    } else {
        String::new()
    };
    format!(
        "{evaluated}/{} evaluated cells · {unfinished} unfinished cells. {}{assessment}",
        cells.len(),
        if evaluated == 16 && cells.len() == 16 {
            "Descriptive observations from two repetitions per variant; no general improvement claim."
        } else {
            "Inconclusive coverage. Unfinished and unavailable cells are retained."
        }
    )
}
fn exact_draft(comparison: &Value, kind: &str, id: &str, draft: &Value) -> bool {
    let target = target_of(comparison);
    target["kind"] == kind
        && target["id"] == id
        && target["baseRevision"] == draft["revision"]
        && content(&target["proposed"]) == content(draft)
}
fn adoption_request(comparison: &Value, saved: &Value) -> Option<Value> {
    let target = target_of(comparison);
    let base = target["baseRevision"].as_u64()?;
    let adopted = saved["revision"].as_u64()?;
    if adopted != base.checked_add(1)?
        || content(saved) != content(&target["proposed"])
        || comparison["adoption"].is_object()
    {
        return None;
    }
    Some(
        json!({"baseRevision":base,"proposedHash":target["proposedHash"],"adoptedRevision":adopted}),
    )
}

impl Workbench {
    #[allow(clippy::too_many_arguments)] // Editor identity and native context stay explicit.
    pub(super) fn comparison_entry(
        &self,
        key: &str,
        kind: &str,
        id: &str,
        draft: &Value,
        dirty: bool,
        proposal: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let can_compare = dirty && draft["revision"].as_u64().is_some_and(|r| r > 0);
        let request = json!({"kind":kind,"targetId":id,"baseRevision":draft["revision"],"draft":draft,"proposalId":proposal});
        let source = key.to_owned();
        let kind = kind.to_owned();
        let target = id.to_owned();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(hint(
                "Comparison is optional. Save or decide through the existing controls when ready.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("compare-{key}")))
                            .label("Compare with current")
                            .disabled(self.blocked() || !can_compare)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.comparisons.source = source.clone();
                                this.open_comparison(request.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("comparison-history-{key}")))
                            .label("Comparison history")
                            .disabled(!self.connected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.comparisons.open = true;
                                this.comparisons.return_panel = Some(this.panel);
                                this.comparisons.target = Some((kind.clone(), target.clone()));
                                this.comparisons.preflight = None;
                                this.comparisons.source.clear();
                                this.comparisons.selected = None;
                                this.comparisons.error.clear();
                                this.read_comparison("/comparisons".into());
                                cx.notify();
                            })),
                    ),
            )
    }
    fn open_comparison(&mut self, request: Value, cx: &mut Context<Self>) {
        self.comparisons.open = true;
        self.comparisons.return_panel = Some(self.panel);
        self.comparisons.target = Some((string(&request, "kind"), string(&request, "targetId")));
        self.comparisons.draft = Some(request.clone());
        self.comparisons.preflight = None;
        self.comparisons.selected = None;
        self.comparisons.start_after_create = false;
        self.comparisons.error.clear();
        self.read_comparison("/comparisons".into());
        self.send("/comparisons/preflight".into(), request, true, cx);
    }
    fn read_comparison(&mut self, path: String) {
        if !self.connected || self.comparisons.pending_reads.contains(&path) {
            return;
        }
        if let Some(client) = &self.client {
            match client.read(path.clone()) {
                Ok(()) => {
                    self.comparisons.pending_reads.insert(path);
                }
                Err(error) => self.comparisons.error = error,
            }
        }
    }
    pub(super) fn comparison_read(&mut self, path: &str, result: Result<Value, String>) {
        self.comparisons.pending_reads.remove(path);
        match result {
            Ok(value) => {
                if path == "/comparisons" {
                    self.comparisons.history = value
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| records(&value, "comparisons"));
                } else if self
                    .comparisons
                    .selected
                    .as_ref()
                    .is_some_and(|v| path == format!("/comparisons/{}", string(v, "id")))
                {
                    self.comparisons.selected = Some(value);
                }
                self.comparisons.error.clear();
            }
            Err(error) => self.comparisons.error = error,
        }
    }
    pub(super) fn refresh_comparison(&mut self) {
        if self.comparisons.open
            && let Some(value) = &self.comparisons.selected
        {
            self.read_comparison(format!("/comparisons/{}", string(value, "id")));
        }
    }
    pub(super) fn comparison_result(&mut self, path: &str, value: &Value, cx: &mut Context<Self>) {
        if path == "/comparisons/preflight" {
            self.comparisons.preflight = Some(value.clone());
            self.notice = "Preflight ready. Review the frozen inputs before starting.".into();
        } else if path == "/comparisons" {
            self.comparisons.selected = Some(value.clone());
            self.comparisons.preflight = None;
            self.read_comparison("/comparisons".into());
            if std::mem::take(&mut self.comparisons.start_after_create) {
                self.send(
                    format!("/comparisons/{}/control", string(value, "id")),
                    json!({"action":"start"}),
                    true,
                    cx,
                );
            }
        } else if path.starts_with("/comparisons/") {
            if value["id"].is_string() && value["manifest"].is_object() {
                self.comparisons.selected = Some(value.clone());
            }
            self.refresh_comparison();
            self.read_comparison("/comparisons".into());
            self.notice = if path.ends_with("/adoption") {
                "Saved revision associated with this exact comparison."
            } else {
                "Control intent recorded. Confirmed execution and ownership are shown below."
            }
            .into();
        } else {
            // Save remains successful if association fails. The result panel can retry from the catalog.
            let matching = self
                .comparisons
                .selected
                .iter()
                .chain(self.comparisons.history.iter())
                .find_map(|comparison| {
                    let target = target_of(comparison);
                    let id = string(target, "id");
                    let saved = match string(target, "kind").as_str() {
                        "role" if path == format!("/roles/{id}") => {
                            value.as_array()?.iter().find(|r| r["id"] == id)?.clone()
                        }
                        "principle" if path == "/principles" && value["id"] == id => value.clone(),
                        "playbook" if path == "/playbooks" && value["id"] == id => value.clone(),
                        "role"
                            if target["proposalId"]
                                .as_str()
                                .is_some_and(|p| path == format!("/learning/{p}/decision")) =>
                        {
                            // Approval may be replayed after catalog synchronization; the visible retry remains available.
                            let mut role = target["proposed"].clone();
                            role["revision"] = value["decision"]["appliedVersion"].clone();
                            role
                        }
                        _ => return None,
                    };
                    adoption_request(comparison, &saved)
                        .map(|body| (string(comparison, "id"), body))
                });
            if let Some((id, body)) = matching {
                self.send(format!("/comparisons/{id}/adoption"), body, true, cx);
            }
        }
    }
    fn current_comparison_draft(&self, comparison: &Value, cx: &App) -> Option<Value> {
        let target = target_of(comparison);
        let id = string(target, "id");
        if let Some(proposal) = target["proposalId"].as_str() {
            let proposal = records(&self.workspace, "learning")
                .into_iter()
                .find(|p| p["id"] == proposal && p["status"] == "pending")?;
            let mut draft = records(&self.workspace, "roles")
                .into_iter()
                .find(|r| r["id"] == id)?;
            draft["instructions"] = proposal["change"].clone();
            draft["revision"] = proposal["baseVersion"].clone();
            return Some(draft);
        }
        if self.comparisons.source == "role-learning" && target["kind"] == "role" {
            return self.current_role_learning_draft(&id, cx);
        }
        self.settings
            .comparison_draft(&string(target, "kind"), &id, cx)
    }
    pub(super) fn comparison_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let mut panel = section("Instruction comparison").min_w_0()
            .child(hint("Evidence for one exact instruction revision. Your normal Save and Approve controls remain available."))
            .child(div().flex().flex_wrap().gap_2()
                .child(Button::new("close-comparison").label("Back to editor or decision").on_click(cx.listener(|this,_,_,cx| {this.comparisons.open=false;cx.notify();})))
                .child(Button::new("refresh-comparison").label("Refresh results and history").disabled(!self.connected).on_click(cx.listener(|this,_,_,cx| {this.refresh_comparison();this.read_comparison("/comparisons".into());cx.notify();}))));
        if !self.comparisons.error.is_empty() {
            panel = panel.child(hint(format!(
                "Refresh unavailable: {}. Retained results may be stale.",
                self.comparisons.error
            )));
        }
        if let Some(preflight) = self.comparisons.preflight.clone() {
            let manifest = &preflight["manifest"];
            let can_start = preflight["canStart"] == true;
            let mut preview=section("Review before starting")
                .child(hint("Four cases × two repetitions × current and proposed instructions = 16 evaluated workers. Coordinators and recovery attempts are additional."))
                .child(hint(format!("{} active seconds per cell · {} active minutes for the comparison. Cells run serially; elapsed time can be longer.", manifest["design"]["perCellActiveMs"].as_u64().map(|v|(v/1000).to_string()).unwrap_or_else(||"Unavailable".into()),manifest["design"]["totalActiveMs"].as_u64().map(|v|(v/60000).to_string()).unwrap_or_else(||"Unavailable".into()))))
                .child(hint("Model usage can incur charges. Time limits do not guarantee a token or monetary ceiling."))
                .child(hint(format!("Mode: {}",manifest["mode"].as_str().unwrap_or("unavailable"))))
                .child(instruction_panel("Current instructions",&manifest["target"]["current"]))
                .child(instruction_panel("Proposed instructions",&manifest["target"]["proposed"]))
                .child(execution_panel(&manifest["execution"]))
                .child(snapshot_panel("Case pack and preflight",&preflight["summary"]));
            for reason in records(&preflight, "reasons") {
                preview = preview.child(hint(
                    reason
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| reason.to_string()),
                ));
            }
            if !can_start {
                preview=preview.child(hint("Start is unavailable until preflight requirements are satisfied. Ordinary saving remains available."));
            }
            let frozen = manifest.clone();
            preview = preview
                .child(
                    Button::new("start-comparison")
                        .primary()
                        .label("Start comparison")
                        .disabled(self.blocked() || !can_start)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.comparisons.start_after_create = true;
                            this.send("/comparisons".into(), json!({"manifest":frozen}), true, cx);
                        })),
                )
                .child(
                    Button::new("retry-comparison-preflight")
                        .label("Refresh preflight")
                        .disabled(self.blocked())
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(body) = this.comparisons.draft.clone() {
                                this.open_comparison(body, cx);
                            }
                        })),
                );
            preview = preview.child(
                Button::new("inspect-comparison-manifest")
                    .label(if self.comparisons.expanded.contains("manifest") {
                        "Hide frozen input details"
                    } else {
                        "Inspect frozen input details"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.comparisons.expanded.remove("manifest") {
                            this.comparisons.expanded.insert("manifest".into());
                        }
                        cx.notify();
                    })),
            );
            if self.comparisons.expanded.contains("manifest") {
                preview = preview.child(snapshot_panel("Frozen inputs", manifest));
            }
            panel = panel.child(preview);
        } else if self.comparisons.selected.is_none()
            && self.pending_commands.contains("/comparisons/preflight")
        {
            panel = panel.child(hint(
                "Checking the current revision and supported evaluation inputs…",
            ));
        }
        if let Some(comparison) = self.comparisons.selected.clone() {
            let id = string(&comparison, "id");
            let state = string(&comparison, "state");
            let target = target_of(&comparison);
            let mut result=section(history_label(&comparison)).min_w_0()
                .child(hint(format!("ID {id}")))
                .child(hint(coverage(&comparison)))
                .child(hint(format!("Requested state: {} · Active time: {} ms",string(&comparison,"desiredState"),comparison["activeMs"])))
                .child(hint("Results preserve behavioral failures, provider errors and interrupted work separately. A quiet process does not establish completion."));
            if let Some(reason) = comparison["attentionReason"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                result = result.child(hint(format!("Attention required: {reason}")));
            }
            let cells = records(&comparison, "cells");
            let (total, unfinished) = (
                cells.len(),
                cells.iter().filter(|c| !c["result"].is_object()).count(),
            );
            let mut controls = div().flex().flex_wrap().gap_2();
            for (action, label, allowed) in [
                ("start", "Start saved comparison", state == "planned"),
                (
                    "pause",
                    "Pause",
                    !terminal(&state) && state != "paused" && state != "planned",
                ),
                (
                    "resume",
                    "Resume",
                    !terminal(&state) && comparison["desiredState"] == "paused",
                ),
                ("cancel", "Cancel remaining cells", !terminal(&state)),
            ] {
                if allowed {
                    let path = format!("/comparisons/{id}/control");
                    controls = controls.child(
                        Button::new(SharedString::from(format!("comparison-{action}")))
                            .label(label)
                            .disabled(self.blocked())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if action != "cancel" {
                                    return this.send(path.clone(), json!({"action":action}), true, cx);
                                }
                                // Cancelled is terminal; continuing means paying for a new comparison.
                                this.confirm_send(
                                    Confirmation {
                                        title: "Cancel remaining cells?",
                                        description: format!("{unfinished} of {total} cells can't be resumed. Finished results are kept."),
                                        keep: "Keep comparison",
                                        confirm: "Cancel remaining cells",
                                    },
                                    path.clone(),
                                    json!({"action":action}),
                                    true,
                                    window,
                                    cx,
                                )
                            })),
                    );
                }
            }
            result = result.child(controls);
            if comparison["adoption"].is_object() {
                result = result.child(hint(
                    comparison["adoption"]["adoptedRevision"]
                        .as_u64()
                        .map(|r| format!("Adopted as revision {r}."))
                        .unwrap_or_else(|| "Adopted · revision unavailable.".into()),
                ));
            } else {
                let catalog = match string(target, "kind").as_str() {
                    "role" => "roles",
                    "principle" => "principles",
                    _ => "playbooks",
                };
                let saved = records(&self.workspace, catalog)
                    .into_iter()
                    .find(|v| v["id"] == target["id"]);
                if let Some(body) = saved
                    .as_ref()
                    .and_then(|saved| adoption_request(&comparison, saved))
                {
                    let path = format!("/comparisons/{id}/adoption");
                    result=result.child(hint("This exact content matches the saved revision. Its comparison association is pending."))
                        .child(Button::new("associate-comparison-adoption").label("Associate saved revision").disabled(self.blocked()).on_click(cx.listener(move|this,_,_,cx|this.send(path.clone(),body.clone(),true,cx))));
                } else {
                    let exact =
                        self.current_comparison_draft(&comparison, cx)
                            .is_some_and(|draft| {
                                exact_draft(
                                    &comparison,
                                    &string(target, "kind"),
                                    &string(target, "id"),
                                    &draft,
                                )
                            });
                    result=result.child(hint(if exact {"The current draft matches this proposed snapshot. Use its normal Save or Approve control to adopt it."} else {"Historical comparison: the draft or saved base differs. These results do not describe your current edits."}));
                }
            }
            result = result
                .child(instruction_panel("Current snapshot", &target["current"]))
                .child(instruction_panel("Proposed snapshot", &target["proposed"]));
            let cells = records(&comparison, "cells");
            let cases = cells
                .iter()
                .map(|cell| string(cell, "caseId"))
                .collect::<std::collections::BTreeSet<_>>();
            for case in cases {
                let matching = cells
                    .iter()
                    .filter(|c| string(c, "caseId") == case)
                    .collect::<Vec<_>>();
                if !matching.is_empty() {
                    result = result.child(hint(format!("{case}: {}", case_counts(&matching))));
                }
            }
            for cell in cells {
                let cell_id = string(&cell, "id");
                let key = cell_id.clone();
                let expanded = self.comparisons.expanded.contains(&cell_id);
                let mut row = section(format!(
                    "{} · repetition {} · {} · {}",
                    string(&cell, "caseId"),
                    cell["repetition"],
                    string(&cell, "variant"),
                    string(&cell, "state")
                ))
                .min_w_0()
                .child(hint(cell["result"]["summary"].as_str().unwrap_or(
                    "Inspect receipts and outcome fields for this cell.",
                )))
                .child(
                    Button::new(SharedString::from(format!("comparison-cell-{cell_id}")))
                        .label(if expanded {
                            "Hide attempts and evidence"
                        } else {
                            "Inspect attempts and evidence"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.comparisons.expanded.remove(&key) {
                                this.comparisons.expanded.insert(key.clone());
                            }
                            cx.notify();
                        })),
                );
                if let Some(reason) = cell["attentionReason"].as_str().filter(|s| !s.is_empty()) {
                    row = row.child(hint(format!("Attention required: {reason}")));
                }
                if expanded {
                    row=row.child(hint(format!("Observed case: {} · Run state: {} · Active duration: {} ms",cell["result"]["observed"].as_bool().map(|v|v.to_string()).unwrap_or_else(||"Unavailable".into()),cell["runState"].as_str().unwrap_or("Not started"),cell["activeMs"].as_u64().map(|v|v.to_string()).unwrap_or_else(||"Unavailable".into()))))
                        .child(snapshot_panel("Measured outcomes",&cell["result"]["scores"]))
                        .child(snapshot_panel("Attempts, handoff and receipts",&cell))
                        .child(hint("Missing usage or cost is unavailable, not zero. Repair and intervention events remain part of the result."));
                    if let (Some(run), Some(client)) = (cell["runId"].as_str(), &self.client) {
                        let run = run.to_owned();
                        let descriptor = client.descriptor_path();
                        let subject = crate::wall::Subject {
                            key: string(&cell, "caseId"),
                            title: format!(
                                "repetition {} · {}",
                                cell["repetition"],
                                string(&cell, "variant")
                            ),
                        };
                        row = row.child(
                            Button::new(SharedString::from(format!(
                                "comparison-processes-{cell_id}"
                            )))
                            .label(crate::wall::OPEN_LABEL)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Err(error) = crate::wall::open(
                                        descriptor.clone(),
                                        run.clone(),
                                        subject.clone(),
                                        cx,
                                    ) {
                                        this.fail_at(error.to_string(), feedback::Place::Top);
                                    }
                                },
                            )),
                        );
                    }
                }
                result = result.child(row);
            }
            panel = panel.child(result);
        }
        let mut history = section("Previous comparisons");
        let matches = self
            .comparisons
            .history
            .iter()
            .filter(|v| {
                self.comparisons.target.as_ref().is_none_or(|(kind, id)| {
                    target_of(v)["kind"] == *kind && target_of(v)["id"] == *id
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if matches.is_empty() {
            history = history.child(hint(
                if self.comparisons.pending_reads.contains("/comparisons") {
                    "Loading comparison history…"
                } else {
                    "No recorded comparisons for this instruction."
                },
            ));
        }
        for value in matches {
            let id = string(&value, "id");
            let key = id.clone();
            let label = history_label(&value);
            history = history.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("open-comparison-{id}")))
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.comparisons.selected = Some(value.clone());
                                this.comparisons.preflight = None;
                                this.read_comparison(format!("/comparisons/{key}"));
                                cx.notify();
                            })),
                    )
                    .child(hint(format!("ID {id}"))),
            );
        }
        panel.child(history)
    }
}
fn execution_panel(execution: &Value) -> Div {
    let mut panel = section("Execution").min_w_0();
    for (label, key) in [
        ("Evaluated specialist", "model"),
        ("Supporting coordinator", "coordinatorModel"),
    ] {
        let model = &execution[key];
        panel = panel.child(hint(format!(
            "{label}: {} · effort {}",
            model["selector"].as_str().unwrap_or("Unavailable"),
            model["effort"].as_str().unwrap_or("Unavailable")
        )));
    }
    panel.child(hint(format!(
        "Harness: {} · Tool and check definitions are pinned before launch.",
        execution["runner"]["kind"]
            .as_str()
            .unwrap_or("Unavailable")
    )))
}
fn instruction_panel(title: &str, value: &Value) -> Div {
    let mut panel = section(title.to_owned()).min_w_0();
    if value.is_null() {
        return panel.child(hint("Unavailable"));
    }
    panel = panel.child(hint(format!(
        "{} · revision {}",
        string(value, "name"),
        value["revision"]
    )));
    for key in ["instructions", "text", "description"] {
        if let Some(text) = value[key].as_str() {
            panel = panel.child(hint(text.to_owned()));
        }
    }
    for (index, step) in records(value, "steps").iter().enumerate() {
        panel = panel.child(hint(format!("{}. {}", index + 1, string(step, "title"))));
    }
    panel
}
fn snapshot_panel(title: &str, value: &Value) -> Div {
    section(title.to_owned())
        .min_w_0()
        .child(hint(if value.is_null() {
            "Unavailable".into()
        } else {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }))
}
fn metric_counts(cells: &[&Value], variant: &str, metric: &str) -> (usize, usize) {
    let known = cells
        .iter()
        .filter(|cell| cell["variant"] == variant)
        .filter_map(|cell| cell["result"]["scores"][metric].as_bool())
        .collect::<Vec<_>>();
    (known.iter().filter(|&&value| value).count(), known.len())
}
fn case_counts(cells: &[&Value]) -> String {
    let mut lines = Vec::new();
    for variant in ["current", "proposed"] {
        let observed = cells
            .iter()
            .filter(|cell| cell["variant"] == variant && cell["result"]["observed"] == true)
            .count();
        let (correct, known) = metric_counts(cells, variant, "sourceCorrectness");
        let (false_completion, claims) = metric_counts(cells, variant, "falseCompletion");
        lines.push(format!("{variant}: {observed}/2 observed · correct {correct}/2 ({known}/2 classified) · false completion {false_completion}/2 ({claims}/2 classified)"));
    }
    let current = metric_counts(cells, "current", "sourceCorrectness");
    let proposed = metric_counts(cells, "proposed", "sourceCorrectness");
    lines.push(if current.1 == 2 && proposed.1 == 2 {
        format!("Proposed − current correctness count: {:+}. Two repetitions per variant; descriptive only.",proposed.0 as i64-current.0 as i64)
    } else { "Correctness difference unavailable until both variants have two classified results.".into() });
    lines.join("\n")
}

#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
