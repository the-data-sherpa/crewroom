use super::*;
/// The PR delivery headline. Ready and merged are stated only from a current
/// observation; delivery failures are named from the push and PR receipts.
pub(super) fn delivery_headline(record: &Value, fresh: bool) -> String {
    const STALE: &str = "observation not current. Reconnect before relying on this.";
    if record["merged"].is_object() {
        return if fresh {
            "Merged".into()
        } else {
            format!("Last observed merged · {STALE}")
        };
    }
    if record["ready"] == true {
        return if fresh {
            "Ready for human review".into()
        } else {
            format!("Last observed ready for human review · {STALE}")
        };
    }
    let push = record["push"]["state"].as_str().unwrap_or("");
    let pr = record["pr"]["state"].as_str().unwrap_or("");
    let headline = if push == "blocked" {
        "Push blocked · the remote branch or its PR changed"
    } else if push == "uncertain" {
        "Push outcome unknown · reconcile to confirm"
    } else if pr == "blocked" {
        "PR blocked · an open PR exists or the provider refused it"
    } else if pr == "uncertain" {
        "PR outcome unknown · reconcile to confirm"
    } else if matches!(push, "pending" | "sending") {
        "Publishing · push receipt pending"
    } else if matches!(pr, "pending" | "sending") {
        "Publishing · PR receipt pending"
    } else if pr == "confirmed" {
        "PR created · not yet ready for review"
    } else {
        "Delivery state unavailable"
    };
    if fresh {
        headline.into()
    } else {
        format!("Last observed: {headline} · observation not current")
    }
}
/// Label names, or "none".
fn label_list(labels: &Value) -> String {
    let names: Vec<_> = labels
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if names.is_empty() {
        "none".into()
    } else {
        format!("\"{}\"", names.join("\", \""))
    }
}
fn short(hash: &Value) -> String {
    hash.as_str()
        .map(|h| h.chars().take(12).collect())
        .unwrap_or_else(|| "unavailable".into())
}
/// The PR host and repository a publication record names. GitHub identities
/// carry their provider; every earlier record is Bitbucket.
pub(super) fn repository_label(repository: &Value) -> String {
    if repository["provider"] == "github" {
        format!(
            "GitHub {}/{} · repository {}",
            repository["owner"].as_str().unwrap_or(""),
            repository["name"].as_str().unwrap_or(""),
            repository["uuid"].as_str().unwrap_or("")
        )
    } else {
        format!(
            "Bitbucket {}/{} · repository {}",
            repository["workspace"].as_str().unwrap_or(""),
            repository["slug"].as_str().unwrap_or(""),
            repository["uuid"].as_str().unwrap_or("")
        )
    }
}
/// The provider's own PR state and the head it last reported, beside the
/// published revision. Provider state is a separate fact from Crewroom's
/// push/PR receipts and from readiness.
pub(super) fn pr_facts(record: &Value) -> Option<String> {
    if record["pr"]["state"] != "confirmed" {
        return None;
    }
    let github = record["repository"]["provider"] == "github";
    let observed = &record["observation"]["pr"];
    let state = match record["pr"]["providerState"].as_str() {
        Some("OPEN") if observed["draft"] == true => "open (draft)",
        Some("OPEN") => "open",
        Some("MERGED") => "merged",
        Some("DECLINED") if github => "closed without merging",
        Some("DECLINED") => "declined",
        Some("SUPERSEDED") => "superseded",
        _ => "state not observed",
    };
    let mut facts = format!(
        "{} PR #{} · {state}",
        if github { "GitHub" } else { "Bitbucket" },
        record["pr"]["id"]
    );
    let published = &record["revision"]["head"];
    if observed.is_object() {
        let head = &observed["source"]["commit"]["hash"];
        facts.push_str(&format!(
            " · observed head {} on base {}",
            short(head),
            short(&observed["destination"]["commit"]["hash"])
        ));
        if head != published {
            facts.push_str(&format!(
                " · differs from the published revision {}",
                short(published)
            ));
        }
    } else {
        facts.push_str(&format!(
            " · head not yet observed (published {})",
            short(published)
        ));
    }
    Some(facts)
}
impl Workbench {
    pub(super) fn read_feedback(&mut self, run: &str, cx: &mut Context<Self>) {
        let path = format!("/runs/{run}/feedback");
        if !self.connected || self.observation.feedback_pending {
            return;
        }
        match self
            .client
            .as_ref()
            .ok_or_else(|| "Service connection unavailable".into())
            .and_then(|c| c.read(path))
        {
            Ok(()) => {
                self.observation.feedback_pending = true;
                self.observation.feedback_error.clear();
            }
            Err(error) => self.observation.feedback_error = error,
        }
        cx.notify();
    }
    pub(super) fn publication_panel(
        &mut self,
        run: &Value,
        result: &Value,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = string(run, "id");
        let record = &result["publication"]["record"];
        let mut panel = div().flex().flex_col().gap_4();
        if record.is_object() {
            let candidates = records(result, "candidates");
            let published = &record["revision"]["head"];
            let mut row = section("PR delivery")
                .child(crate::accessibility::text(delivery_headline(
                    record,
                    self.observation.result_fresh,
                )))
                .child(hint(format!(
                    "Push: {} · PR: {}",
                    value(&record["push"]["state"]),
                    value(&record["pr"]["state"]),
                )))
                .child(hint(format!(
                    "Published: {}",
                    revision_label(&candidates, published)
                )))
                .children(
                    (candidates
                        .last()
                        .is_some_and(|c| c["revision"]["head"] != *published))
                    .then(|| hint("Not the latest revision")),
                )
                .child(hint(text_lines(&record["blockers"])))
                .child(hint(record["error"].as_str().unwrap_or("")));
            if let Some(facts) = pr_facts(record) {
                row = row.child(hint(facts).id("pr-provider-facts"));
            }
            if record["merged"].is_object() {
                row = row.child(hint(format!(
                    "Merge commit: {} · observed {}",
                    value(&record["merged"]["commit"]),
                    value(&record["merged"]["observedAt"])
                )));
            }
            if let Some(url) = record["pr"]["url"]
                .as_str()
                .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            {
                let url = url.to_owned();
                row = row.child(
                    Button::new("open-pull-request")
                        .label("Open pull request")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            let observed = &record["observation"];
            if observed.is_object() {
                row = row
                    .child(hint(format!(
                        "Provider observation: {}. Merge remains your decision.",
                        value(&observed["observedAt"])
                    )))
                    .child(crate::accessibility::text("Hosted statuses"));
                for key in records(&record["policy"], "statusKeys") {
                    let statuses = records(observed, "statuses");
                    let state = statuses
                        .iter()
                        .find(|s| s["key"] == key)
                        .map(|s| value(&s["state"]))
                        .unwrap_or_else(|| "Unavailable".into());
                    row = row.child(
                        hint(format!("{} · {state}", value(&key)))
                            .id(SharedString::from(format!("hosted-status-{}", value(&key)))),
                    );
                }
                row = row.child(crate::accessibility::text("PR pipeline"));
                let pipelines = records(observed, "pipelines");
                if pipelines.is_empty() {
                    row = row.child(hint("No matching pipeline observed for this PR revision."));
                }
                for (pipeline_index, p) in pipelines.iter().enumerate() {
                    row = row.child(
                        hint(format!(
                            "Build {} · {}\nHead: {}\nBase: {}",
                            value(&p["build_number"]),
                            p["state"]["result"]["name"]
                                .as_str()
                                .or(p["state"]["name"].as_str())
                                .unwrap_or("Unavailable"),
                            value(&p["target"]["commit"]["hash"]),
                            value(&p["target"]["destination_commit"]["hash"])
                        ))
                        .id(SharedString::from(format!("pipeline-{pipeline_index}"))),
                    );
                    for (index, step) in records(p, "steps").iter().enumerate() {
                        row = row.child(
                            hint(format!(
                                "{} · {}",
                                value(&step["name"]),
                                step["state"]["result"]["name"]
                                    .as_str()
                                    .or(step["state"]["name"].as_str())
                                    .unwrap_or("Unavailable")
                            ))
                            .id(SharedString::from(format!(
                                "pipeline-{pipeline_index}-step-{index}"
                            ))),
                        );
                    }
                }
            } else {
                row=row.child(hint("Hosted observation unavailable. Local evidence alone does not establish hosted readiness."));
            }
            if record["pr"]["id"].is_number() {
                let identity = format!("{}:{}", value(&record["id"]), value(&record["pr"]["id"]));
                if self.observation.feedback_for.as_deref() != Some(&identity)
                    && self.connected
                    && self.observation.result_fresh
                {
                    self.observation.feedback_for = Some(identity);
                    self.read_feedback(&id, cx);
                }
                let mut feedback =
                    section(format!("PR feedback · #{}", value(&record["pr"]["id"]))).child(hint(
                        format!("Source: {}", repository_label(&record["repository"])),
                    ));
                let observed = &record["feedback"];
                if observed.is_null() {
                    feedback = feedback.child(hint(
                        "Feedback has not been observed. This is not a confirmed empty review.",
                    ));
                } else {
                    feedback = feedback.child(hint(format!(
                        "Feedback read: {} · {}\n{}",
                        value(&observed["state"]),
                        value(&observed["observedAt"]),
                        observed["reason"].as_str().unwrap_or("")
                    )));
                    let comments = records(observed, "comments");
                    if comments.is_empty() {
                        feedback = feedback.child(hint(if observed["state"] == "complete" {
                            "No PR comments observed."
                        } else {
                            "No feedback is known; the read is incomplete."
                        }));
                    }
                    for comment in comments {
                        let mut line = section(format!(
                            "Comment #{} · {}",
                            value(&comment["id"]),
                            comment["user"]["display_name"]
                                .as_str()
                                .or(comment["user"]["uuid"].as_str())
                                .unwrap_or("Author unavailable")
                        ))
                        .id(SharedString::from(format!(
                            "feedback-comment-{}",
                            value(&comment["id"])
                        )))
                        .child(hint(format!(
                            "Updated {} · currentness {}",
                            value(&comment["updated_on"]),
                            value(&comment["currentness"])
                        )))
                        .child(crate::accessibility::text(
                            comment["content"]["raw"]
                                .as_str()
                                .unwrap_or("No comment text recorded")
                                .to_owned(),
                        ));
                        if comment["inline"].is_object() {
                            line = line.child(hint(format!(
                                "{}:{}{}",
                                value(&comment["inline"]["path"]),
                                value(
                                    comment["inline"]
                                        .get("to")
                                        .or(comment["inline"].get("from"))
                                        .unwrap_or(&Value::Null)
                                ),
                                if comment["inline"]["outdated"] == true {
                                    " · outdated line"
                                } else {
                                    ""
                                }
                            )));
                        }
                        if comment["parent"].is_object() {
                            line = line.child(hint(format!(
                                "Reply to #{}",
                                value(&comment["parent"]["id"])
                            )));
                        }
                        for (flag_index, (yes, label)) in [
                            (comment["deleted"] == true, "Deleted at source"),
                            (!comment["resolution"].is_null(), "Resolved"),
                            (
                                comment["currentness"] == "conflict",
                                "Conflicting version; prior text retained",
                            ),
                            (comment["currentness"] == "unknown", "Currentness unknown"),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if yes {
                                line =
                                    line.child(hint(label).id(SharedString::from(format!(
                                        "comment-flag-{flag_index}"
                                    ))));
                            }
                        }
                        for continuation in
                            records(result, "feedbackContinuations").iter().filter(|c| {
                                c["publicationId"] == record["id"]
                                    && c["prId"] == record["pr"]["id"]
                            })
                        {
                            if let Some(saved) = records(&continuation["feedback"], "comments")
                                .iter()
                                .find(|c| c["id"] == comment["id"])
                            {
                                line=line.child(hint(format!("Captured by continuation {} · comment version {} · {} read · {}",value(&continuation["id"]),value(&saved["updated_on"]),value(&continuation["feedback"]["state"]),value(&continuation["state"]))).id(SharedString::from(format!("continuation-{}", value(&continuation["id"])))));
                                if saved["updated_on"] != comment["updated_on"] {
                                    line = line.child(
                                        hint(format!(
                                            "Earlier captured text: {}",
                                            value(&saved["content"]["raw"])
                                        ))
                                        .id(
                                            SharedString::from(format!(
                                                "continuation-text-{}",
                                                value(&continuation["id"])
                                            )),
                                        ),
                                    );
                                }
                            }
                        }
                        feedback = feedback.child(line);
                    }
                }
                let run = id.clone();
                feedback = feedback
                    .child(hint(self.observation.feedback_error.clone()))
                    .child(
                        Button::new("refresh-pr-feedback")
                            .label(if self.observation.feedback_pending {
                                "Reading feedback…"
                            } else {
                                "Refresh PR feedback"
                            })
                            .disabled(!self.connected || self.observation.feedback_pending)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.read_feedback(&run, cx)),
                            ),
                    );
                row = row.child(feedback);
            }
            let path = format!("/runs/{id}/publication/reconcile");
            row = row.child(
                Button::new("reconcile-publication")
                    .label("Reconcile publication")
                    .disabled(
                        self.blocked()
                            || !self.observation.result_fresh
                            || result["publication"]["available"] != true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(path.clone(), json!({}), false, cx)
                    })),
            );
            let blockers = crate::workbench::blockers::reconcile_publication(
                self.observation.result_fresh,
                result["publication"]["available"] == true,
                result["publication"]["reason"].as_str().unwrap_or(""),
            );
            row = row.children(crate::workbench::blockers::line(
                "publication-blockers",
                "reconcile",
                &blockers,
            ));
            panel = panel.child(row);
        }
        let updates: Vec<_> = records(&self.workspace, "jiraUpdates")
            .into_iter()
            .filter(|u| u["runId"] == id)
            .collect();
        if self.workspace["jira"].is_object() || !updates.is_empty() {
            let reconcile = updates.iter().any(|u| {
                ["pending", "uncertain", "sending"].contains(&u["state"].as_str().unwrap_or(""))
            });
            let mut jira=section("Jira delivery").child(hint("Only confirmed updates have provider receipts. Jira status is refreshed separately."));
            if updates.is_empty() {
                jira = jira.child(hint("No Jira updates requested."));
            }
            for u in updates {
                jira = jira.child(
                    hint(format!(
                        "{} · {}\n{}\nEvent {} · receipt {}\n{}",
                        value(&u["kind"]),
                        if u["state"] == "sending" {
                            "Sending · confirmation pending".into()
                        } else {
                            value(&u["state"])
                        },
                        u["body"].as_str().unwrap_or("Configured transition"),
                        value(&u["eventKey"]),
                        value(&u["externalId"]),
                        u["reason"].as_str().unwrap_or("")
                    ))
                    .id(SharedString::from(format!(
                        "jira-delivery-{}",
                        value(&u["eventKey"])
                    ))),
                );
            }
            panel = panel.child(
                jira.child(
                    Button::new("reconcile-jira")
                        .label("Reconcile Jira updates")
                        .disabled(
                            self.blocked() || !self.workspace["jira"].is_object() || !reconcile,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send("/jira/reconcile".into(), json!({}), false, cx)
                        })),
                )
                .children(crate::workbench::blockers::line(
                    "jira-reconcile-blockers",
                    "reconcile Jira",
                    &crate::workbench::blockers::reconcile_jira(
                        self.workspace["jira"].is_object(),
                        reconcile,
                    ),
                )),
            );
        }
        if let Some(github) = self.github_delivery_section(run, &id, cx) {
            panel = panel.child(github);
        }
        panel
    }

    /// GitHub issue delivery (FOR-48) for a GitHub-sourced ticket: each update
    /// with its state, reason and receipt, a retry for blocked or uncertain
    /// updates, and the issue's GitHub state named as a separate fact.
    fn github_delivery_section(
        &self,
        run: &Value,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let updates: Vec<_> = records(&self.workspace, "githubUpdates")
            .into_iter()
            .filter(|u| u["runId"] == id)
            .collect();
        if run["taskSnapshot"]["source"] != "github" && updates.is_empty() {
            return None;
        }
        let mut github = section("GitHub issue delivery").child(hint(
            "Only confirmed updates reached the issue. Pending, sending, uncertain and blocked updates are not delivered. The issue's GitHub open/closed state is a source fact, separate from this run's workflow state.",
        ));
        if updates.is_empty() {
            github = github.child(hint("No GitHub issue updates requested."));
        }
        for u in &updates {
            let state = match u["state"].as_str().unwrap_or("") {
                "sending" => "Sending · confirmation pending".to_owned(),
                "confirmed" => "Delivered".to_owned(),
                other => format!("Not delivered · {other}"),
            };
            let what = match u["kind"].as_str().unwrap_or("") {
                "in_progress" | "in_review" => format!(
                    "Workflow label change: add {} · remove {}",
                    label_list(&u["labels"]["add"]),
                    label_list(&u["labels"]["remove"])
                ),
                "done" => format!(
                    "Merged: workflow labels removed; GitHub issue state {}",
                    u["issueState"].as_str().unwrap_or("not yet read")
                ),
                _ => u["body"].as_str().unwrap_or("").to_owned(),
            };
            let key = value(&u["eventKey"]);
            github = github.child(
                hint(format!(
                    "{} {} · {state}\n{what}\nEvent {key} · receipt {}\n{}",
                    u["issueKey"].as_str().unwrap_or(""),
                    value(&u["kind"]),
                    value(&u["externalId"]),
                    u["reason"].as_str().unwrap_or("")
                ))
                .id(SharedString::from(format!("github-delivery-{key}"))),
            );
            if matches!(u["state"].as_str(), Some("blocked" | "uncertain"))
                && !u["reason"]
                    .as_str()
                    .is_some_and(|r| r.starts_with("Superseded by the merge"))
            {
                let path = format!("/github/delivery/{}/retry", value(&u["id"]));
                github = github.child(
                    Button::new(SharedString::from(format!("retry-github-{key}")))
                        .label(if u["state"] == "uncertain" {
                            "Retry after checking the issue"
                        } else {
                            "Retry"
                        })
                        .disabled(self.blocked())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(path.clone(), json!({}), false, cx)
                        })),
                );
            }
        }
        let pending = updates.iter().any(|u| {
            ["pending", "uncertain", "sending"].contains(&u["state"].as_str().unwrap_or(""))
        });
        Some(
            github.child(
                Button::new("reconcile-github")
                    .label("Reconcile GitHub updates")
                    .disabled(self.blocked() || !pending)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.send("/github/delivery/reconcile".into(), json!({}), false, cx)
                    })),
            ),
        )
    }
}
