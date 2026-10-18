use super::*;

/// One rendered line of a learning decision. Unchanged runs collapse to a count.
#[derive(Debug, PartialEq)]
pub(super) enum LineChange {
    Unchanged(usize),
    Removed(String),
    Added(String),
}

/// Line diff between the current and proposed text. Blank lines only separate
/// paragraphs, so they never show as a change on their own.
pub(super) fn line_changes(before: &str, after: &str) -> Vec<LineChange> {
    let a = before
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>();
    let b = after
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>();
    // Longest-common-subsequence table; instructions are bounded to 12,000 characters.
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut changes) = (0, 0, Vec::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            match changes.last_mut() {
                Some(LineChange::Unchanged(n)) => *n += 1,
                _ => changes.push(LineChange::Unchanged(1)),
            }
            (i, j) = (i + 1, j + 1);
        } else if i < a.len() && (j == b.len() || table[i + 1][j] >= table[i][j + 1]) {
            changes.push(LineChange::Removed(a[i].to_owned()));
            i += 1;
        } else {
            changes.push(LineChange::Added(b[j].to_owned()));
            j += 1;
        }
    }
    changes
}

impl Workbench {
    pub(in crate::workbench) fn learning_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut proposals = records(&self.workspace, "learning");
        proposals.sort_by_key(|p| p["status"] != "pending");
        let mut panel=section("Learning decisions").child(hint("Approve a new role or project-guidance version for future assignments. Existing assignments keep their captured instructions, models and checks."));
        if proposals.is_empty() {
            return panel.child(hint("No learning proposals recorded."));
        }
        for proposal in proposals {
            let id = string(&proposal, "id");
            let path = format!("/learning/{id}/decision");
            let target = &proposal["target"];
            // The version the decision would replace; a pending proposal on another base can't apply.
            let (title, current, version) = if target["kind"] == "role" {
                let role = records(&self.workspace, "roles")
                    .into_iter()
                    .find(|r| r["id"] == target["roleId"])
                    .unwrap_or(Value::Null);
                (
                    format!(
                        "Role · {}",
                        role["name"]
                            .as_str()
                            .unwrap_or(target["roleId"].as_str().unwrap_or("unavailable"))
                    ),
                    string(&role, "instructions"),
                    role["revision"].as_u64().unwrap_or(1),
                )
            } else {
                let repo = records(&self.workspace, "repositories")
                    .into_iter()
                    .find(|r| r["id"] == target["repositoryId"])
                    .unwrap_or(Value::Null);
                (
                    format!(
                        "Project guidance · {}",
                        repo["name"].as_str().unwrap_or("unavailable")
                    ),
                    records(&repo, "guidance")
                        .last()
                        .map(|g| string(g, "text"))
                        .unwrap_or_default(),
                    records(&repo, "guidance")
                        .last()
                        .and_then(|g| g["version"].as_u64())
                        .unwrap_or(0),
                )
            };
            let pending = proposal["status"] == "pending";
            let stale = pending && proposal["baseVersion"].as_u64() != Some(version);
            let status = match proposal["status"].as_str().unwrap_or("") {
                "pending" => format!("Proposed against version {}", proposal["baseVersion"]),
                "approved" => {
                    if proposal["decision"]["appliedVersion"].is_number() {
                        format!(
                            "Approved · version {}",
                            proposal["decision"]["appliedVersion"]
                        )
                    } else {
                        "Approved · saving new version".into()
                    }
                }
                "superseded" => "Approved · not applied".into(),
                _ => "Rejected".into(),
            };
            let mut row = section(title)
                .id(SharedString::from(format!("learning-proposal-{id}")))
                .child(crate::accessibility::text(status))
                .child(hint(format!(
                    "{} · {}",
                    if proposal["origin"] == "operator" {
                        "Operator proposal".into()
                    } else {
                        format!("Coordinator · run {}", string(&proposal, "runId"))
                    },
                    string(&proposal, "createdAt")
                )))
                .child(crate::accessibility::text(string(&proposal, "rationale")))
                .child(hint(format!(
                    "Evidence: {}",
                    records(&proposal, "evidence")
                        .iter()
                        .map(|e| format!("{} {}", string(e, "kind"), string(e, "id")))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            if pending {
                row = row.child(crate::accessibility::text(
                    "Changes from the current version",
                ));
                let changes = line_changes(&current, &string(&proposal, "change"));
                if !changes
                    .iter()
                    .any(|c| !matches!(c, LineChange::Unchanged(_)))
                {
                    row = row.child(hint("No line changes."));
                }
                for change in changes {
                    row = row.child(match change {
                        LineChange::Unchanged(1) => hint("1 unchanged line"),
                        LineChange::Unchanged(n) => hint(format!("{n} unchanged lines")),
                        LineChange::Removed(line) => {
                            div().child(crate::accessibility::text(format!("Removed: {line}")))
                        }
                        LineChange::Added(line) => {
                            div().child(crate::accessibility::text(format!("Added: {line}")))
                        }
                    });
                }
                if stale {
                    let (noun, unit) = if target["kind"] == "role" {
                        ("role", "revision")
                    } else {
                        ("project guidance", "version")
                    };
                    row = row
                        .child(crate::accessibility::text(format!(
                            "Stale: {noun} is now {unit} {version}. This proposal can't be applied."
                        )))
                        .child(crate::accessibility::text(
                            if proposal["origin"] == "operator" {
                                "Submit a new proposal from Agent roles."
                            } else {
                                "Ask the coordinator for a new proposal."
                            },
                        ));
                }
            } else {
                row = row
                    .child(crate::accessibility::text("Proposed text"))
                    .child(crate::accessibility::text(string(&proposal, "change")));
            }
            if proposal["target"]["kind"] == "role"
                && let Some(mut draft) = records(&self.workspace, "roles")
                    .into_iter()
                    .find(|role| role["id"] == proposal["target"]["roleId"])
            {
                draft["instructions"] = proposal["change"].clone();
                draft["revision"] = proposal["baseVersion"].clone();
                row = row.child(self.comparison_entry(
                    &format!("proposal-{id}"),
                    "role",
                    &string(&proposal["target"], "roleId"),
                    &draft,
                    pending && !stale,
                    Some(&id),
                    cx,
                ));
            }
            if pending {
                self.collaboration
                    .form(&path, &json!({}), &[("reason", true)], window, cx);
                let form = &self.collaboration.forms[&path];
                let reason = form.fields.text("reason", cx);
                row = row.child(form.fields.render(
                    &format!("learning-{id}"),
                    "reason",
                    "Decision reason (required)",
                    self.busy,
                ));
                let mut controls = div().flex().flex_wrap().gap_2();
                for (decision, label, unavailable) in
                    [("approve", "Approve", stale), ("reject", "Reject", false)]
                {
                    let key = path.clone();
                    let reason = reason.clone();
                    controls = controls.child(
                        Button::new(SharedString::from(format!("{decision}-learning-{id}")))
                            .label(label)
                            .disabled(self.blocked() || reason.trim().is_empty() || unavailable)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(
                                    key.clone(),
                                    json!({"decision":decision,"reason":reason}),
                                    true,
                                    cx,
                                )
                            })),
                    );
                }
                row = row
                    .child(controls)
                    .children(crate::workbench::blockers::line(
                        format!("learning-blockers-{id}"),
                        "decide",
                        &crate::workbench::blockers::learning(reason.trim().is_empty()),
                    ));
            } else {
                row = row.child(self.decided_draft(&path, cx));
                row = row.child(hint(format!(
                    "Decision: {}",
                    string(&proposal["decision"], "reason")
                )));
                if let Some(reason) = proposal["decision"]["notApplied"].as_str() {
                    row = row.child(hint(reason.to_owned()));
                }
                if proposal["status"] == "approved"
                    && proposal["decision"]["pendingRoleSave"] == true
                {
                    let body = json!({"decision":"approve","reason":proposal["decision"]["reason"],"requestId":proposal["decision"]["requestId"]});
                    row = row.child(
                        Button::new(SharedString::from(format!("retry-learning-{id}")))
                            .label("Retry saving the approved version")
                            .disabled(self.blocked())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(path.clone(), body.clone(), false, cx)
                            })),
                    );
                }
            }
            panel = panel.child(row);
        }
        panel
    }
}

#[cfg(test)]
mod tests;
