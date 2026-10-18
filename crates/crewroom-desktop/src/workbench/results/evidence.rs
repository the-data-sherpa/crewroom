use super::*;

/// Match the receipt contract: a missing or interrupted process is never a failed check.
pub(in crate::workbench) fn check_label(check: Option<&Value>, definition: &Value) -> &'static str {
    let Some(c) = check else {
        return "Not run";
    };
    if c["definition"] != *definition {
        return "Out of date";
    }
    if c["started"] == true
        && c["completed"] == true
        && c["sourceIntact"] == true
        && c["timedOut"] == false
        && c["exitCode"] == 0
    {
        return "Passed";
    }
    if c["timedOut"] == true {
        return "Timed out";
    }
    if c["started"] != true || c["completed"] != true {
        return "Interrupted · incomplete";
    }
    if c["sourceIntact"] != true {
        return "Source changed during check";
    }
    "Failed"
}
fn producer(result: &Value, attempt: &Value) -> String {
    let attempts = records(result, "attempts");
    let a = attempts
        .iter()
        .find(|a| a["id"] == *attempt)
        .cloned()
        .unwrap_or_default();
    let assignments = records(result, "assignments");
    let name = assignments
        .iter()
        .find(|s| s["id"] == a["assignmentId"])
        .map(|s| value(&s["name"]))
        .unwrap_or_else(|| "Unknown assignment".into());
    format!(
        "{name} · attempt {} · model {} · session {}",
        value(attempt),
        value(&a["model"]["resolved"]["selector"]),
        value(&a["sessionId"])
    )
}
impl Workbench {
    pub(super) fn candidate_evidence(
        &self,
        run: &str,
        candidate: &Value,
        result: &Value,
        cx: &Context<Self>,
    ) -> Div {
        let id = string(candidate, "id");
        let path = format!("/runs/{run}/result/candidates/{id}/diff");
        let mut panel = section("Candidate evidence")
            .child(hint(format!(
                "Base: {}\nCandidate commit: {}\nTree: {}",
                value(&candidate["revision"]["base"]),
                value(&candidate["revision"]["head"]),
                value(&candidate["revision"]["tree"])
            )))
            .child(
                Button::new("view-candidate-diff")
                    .label("View change")
                    .disabled(!self.connected)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_detail(path.clone(), "Candidate diff".into(), false, window, cx)
                    })),
            );
        if candidate["refreshedFrom"].is_string() {
            panel = panel.child(hint(format!(
                "Rebased from candidate {}. Earlier checks and review cover that prior revision.",
                value(&candidate["refreshedFrom"])
            )));
        }
        let checks = records(result, "checks");
        let definitions = records(result, "requiredChecks");
        let mut check_panel = section("Repository checks");
        if definitions.is_empty() {
            check_panel = check_panel.child(hint("No required checks configured."));
        }
        for definition in definitions {
            let check = checks
                .iter()
                .rev()
                .find(|c| c["candidateId"] == candidate["id"] && c["checkId"] == definition["id"]);
            let mut row = section(format!(
                "{} · {} · {}",
                value(&definition["id"]),
                if definition["required"] == true {
                    "Required"
                } else {
                    "Optional"
                },
                check_label(check, &definition)
            ))
            .id(SharedString::from(format!(
                "check-definition-{}",
                value(&definition["id"])
            )))
            .child(hint(format!(
                "Command: {} {}",
                value(&definition["command"]),
                value(&definition["args"])
            )));
            if let Some(c) = check {
                row=row.child(hint(format!("Exit: {} · platform {} · network {} · memory {} MiB",value(&c["exitCode"]),value(&c["environment"]["platform"]),value(&c["environment"]["network"]),value(&c["environment"]["memoryMiB"]))))
                    .child(hint(producer(result,&c["attemptId"]))).child(hint(format!("Image: {}\nTemporary file execution: {}\nOutput SHA-256: {}\nSource SHA-256: {}",value(&c["environment"]["image"]),value(&c["environment"]["tmpExecutable"]),value(&c["output"]["sha256"]),value(&c["source"]["sha256"]))))
                    .child(self.artifact_button(run,&c["output"],"View check output",format!("check-output-{}",value(&c["id"])),cx));
            }
            check_panel = check_panel.child(row);
        }
        for operation in records(result, "checkOperations")
            .iter()
            .filter(|o| o["candidateId"] == candidate["id"] && o["state"] == "rejected")
        {
            check_panel = check_panel.child(
                hint(format!(
                    "Rejected check {}: {}",
                    value(&operation["definition"]["id"]),
                    value(&operation["reason"])
                ))
                .id(SharedString::from(format!(
                    "check-operation-{}",
                    value(&operation["id"])
                ))),
            );
        }
        panel = panel.child(check_panel);
        let mut reviews = Vec::<Value>::new();
        for review in records(result, "reviews")
            .into_iter()
            .filter(|r| r["candidateId"] == candidate["id"])
        {
            reviews.retain(|r| r["assignmentId"] != review["assignmentId"]);
            reviews.push(review);
        }
        let mut review_panel = section("Independent review");
        if reviews.is_empty() {
            review_panel =
                review_panel.child(hint("No independent review recorded for this revision."));
        }
        for review in reviews {
            let mut row = section(if review["verdict"] == "pass" {
                "Independent review passed"
            } else {
                "Independent review requested changes"
            })
            .id(SharedString::from(format!(
                "review-{}",
                value(&review["id"])
            )))
            .child(crate::accessibility::text(value(&review["summary"])))
            .child(hint(producer(result, &review["attemptId"])));
            if review["checkPolicyRevision"].as_u64().unwrap_or(0)
                != records(result, "checkPolicyChanges").len() as u64
            {
                row = row.child(hint("Review predates the current check environment."));
            }
            for (index, finding) in records(&review, "findings").iter().enumerate() {
                row = row.child(
                    hint(format!(
                        "{} · {}:{}\n{}",
                        value(&finding["severity"]),
                        value(&finding["path"]),
                        value(&finding["line"]),
                        value(&finding["body"])
                    ))
                    .id(SharedString::from(format!("finding-{index}"))),
                );
            }
            row = row.child(crate::accessibility::text("Criteria coverage"));
            let coverage = records(&review, "coverage");
            for (i, criterion) in records(result, "criteria").iter().enumerate() {
                let entry = coverage.iter().find(|c| c["criterion"] == i);
                row = row.child(
                    hint(format!(
                        "Criterion {i}: {}\n{}",
                        value(criterion),
                        entry
                            .map(|c| format!(
                                "{} · Checks: {}",
                                value(&c["assessment"]),
                                value(&c["checkIds"])
                            ))
                            .unwrap_or_else(|| "No coverage recorded.".into())
                    ))
                    .id(SharedString::from(format!("review-criterion-{i}"))),
                );
            }
            review_panel = review_panel.child(row.child(self.artifact_button(
                run,
                &review["artifact"],
                "View review record",
                format!("review-artifact-{}", value(&review["id"])),
                cx,
            )));
        }
        panel.child(review_panel)
    }
    pub(super) fn research_evidence(&self, run: &Value, result: &Value, cx: &Context<Self>) -> Div {
        let research = &result["research"];
        let assessment = &research["assessment"];
        let id = string(run, "id");
        let mut panel=section("Research outcome and evidence").child(crate::accessibility::text(if assessment["completed"]==true{"Completed research outcome"}else{"Research incomplete"}))
            .child(hint(format!("Accepted revision {} · pinned source {}. Research has no coding gates, commit or PR.",value(&assessment["revision"]),research["source"]["commit"].as_str().unwrap_or("not assigned"))))
            .child(hint(text_lines(&assessment["blockers"])));
        if research["outcome"].is_object() && assessment["completed"] != true {
            panel=panel.child(hint(format!("Previous completion for revision {} is stale after adoption; current criteria need new verified artifacts.",value(&research["outcome"]["revision"]))));
        }
        for (i, criterion) in records(research, "criteria").iter().enumerate() {
            panel = panel.child(
                hint(format!(
                    "Criterion {i}: {} · {}",
                    value(criterion),
                    if records(assessment, "covered").contains(&json!(i)) {
                        "Verified"
                    } else {
                        "Uncovered"
                    }
                ))
                .id(SharedString::from(format!("research-criterion-{i}"))),
            );
        }
        let artifacts = records(research, "artifacts");
        if artifacts.is_empty() {
            panel = panel.child(hint("No artifacts submitted."));
        }
        for item in artifacts {
            let label = if item["state"] == "rejected" {
                "Rejected"
            } else if item["supersededBy"].is_string() {
                "Superseded"
            } else if item["requirementRevision"] != assessment["revision"]
                || item["pinnedCommit"] != research["source"]["commit"]
            {
                "Stale · accepted revision changed"
            } else {
                "Verified"
            };
            let mut row=section(format!("{} · {} · {label}",value(&item["title"]),value(&item["kind"]))).id(SharedString::from(format!("research-artifact-{}", value(&item["id"])))).child(hint(text_lines(&item["reasons"]))).child(hint(format!("Submitted {} · assignment {} · attempt {}\nModel {} ({}) · requested {} ({}) · fallback {}\nRequirement revision {} · pinned commit {}\nArtifact SHA-256 {} · {} bytes",value(&item["createdAt"]),value(&item["assignmentId"]),value(&item["attemptId"]),value(&item["model"]["selector"]),value(&item["model"]["effort"]),value(&item["model"]["requested"]["model"]),value(&item["model"]["requested"]["effort"]),value(&item["model"]["fallbackFrom"]),value(&item["requirementRevision"]),value(&item["pinnedCommit"]),value(&item["body"]["sha256"]),value(&item["body"]["bytes"]))))
                .child(self.artifact_button(&id,&item["body"],"View research artifact",format!("research-artifact-{}",value(&item["id"])),cx));
            if item["supersededBy"].is_string() {
                row = row.child(hint(format!(
                    "Superseded by artifact {}",
                    value(&item["supersededBy"])
                )));
            }
            row = row.child(crate::accessibility::text("Citations"));
            let citations = records(&item, "citations");
            if citations.is_empty() {
                row = row.child(hint("No repository citations."));
            }
            for (index, citation) in citations.iter().enumerate() {
                row = row.child(
                    hint(format!(
                        "{} · {}:{}–{} @ {}\n{}\n{}",
                        if citation["verified"] == true {
                            "Verified citation"
                        } else {
                            "Rejected citation"
                        },
                        value(&citation["path"]),
                        value(&citation["startLine"]),
                        value(&citation["endLine"]),
                        value(&citation["commit"]),
                        value(&citation["excerpt"]),
                        citation["reason"].as_str().unwrap_or("")
                    ))
                    .id(SharedString::from(format!("citation-{index}"))),
                );
            }
            row = row
                .child(crate::accessibility::text("Limitations"))
                .child(crate::accessibility::text(value(&item["limitations"])))
                .child(crate::accessibility::text("Criterion coverage"));
            for coverage in records(&item, "coverage") {
                row = row.child(
                    hint(format!(
                        "Criterion {} → {}",
                        value(&coverage["criterionId"]),
                        value(&coverage["section"])
                    ))
                    .id(SharedString::from(format!(
                        "coverage-{}",
                        value(&coverage["criterionId"])
                    ))),
                );
            }
            panel = panel.child(row);
        }
        panel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_stale_and_changed_source_receipts_never_claim_pass_or_failed_execution() {
        let definition = json!({"id":"test","image":"immutable"});
        let mut c = json!({"definition":definition,"started":true,"completed":true,"sourceIntact":true,"timedOut":false,"exitCode":0});
        assert_eq!(check_label(None, &definition), "Not run");
        assert_eq!(check_label(Some(&c), &definition), "Passed");
        c["definition"]["image"] = json!("older");
        assert_eq!(check_label(Some(&c), &definition), "Out of date");
        c["definition"] = definition.clone();
        c["timedOut"] = json!(true);
        assert_eq!(check_label(Some(&c), &definition), "Timed out");
        c["timedOut"] = json!(false);
        c["completed"] = json!(false);
        assert_eq!(
            check_label(Some(&c), &definition),
            "Interrupted · incomplete"
        );
        c["completed"] = json!(true);
        c["sourceIntact"] = json!(false);
        assert_eq!(
            check_label(Some(&c), &definition),
            "Source changed during check"
        );
        c["sourceIntact"] = json!(true);
        c["exitCode"] = json!(1);
        assert_eq!(check_label(Some(&c), &definition), "Failed");
    }
}
