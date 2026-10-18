//! Read projection of pending operator decisions ("Needs you").
//! Every item is derived from recorded workflow, provider or proposal state, never from
//! terminal output, and disappears once that state is resolved by any screen.
use crate::{array, present};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Per-run inputs the workspace snapshot does not carry.
pub(super) struct RunState {
    pub runtime: Value,
    pub publication: Value,
    pub publication_available: bool,
}

fn ended(run: &Value) -> bool {
    run["desiredState"] == "cancelled"
        || ["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""))
}

fn item(kind: &str, id: String, run: Option<&Value>, task_id: &Value, title: String) -> Value {
    let mut v = json!({"kind":kind,"id":id,"taskId":task_id,"title":title});
    if let Some(run) = run {
        v["runId"] = run["id"].clone();
    }
    v
}

pub(super) fn needs_you(v: &Value, runs: &HashMap<String, RunState>) -> Vec<Value> {
    let mut items = Vec::new();
    let tasks = array(&v["tasks"]);
    let task = |id: &Value| tasks.iter().find(|t| t["id"] == *id);
    for t in tasks.iter().filter(|t| t["needsReacceptance"] == true) {
        let mut i = item(
            "requirements",
            format!("requirements-brief-{}", t["id"].as_str().unwrap_or("")),
            None,
            &t["id"],
            format!(
                "Source changed for {}",
                t["key"].as_str().unwrap_or("ticket")
            ),
        );
        i["stage"] = json!("brief");
        i["detail"] = json!(
            "The imported source changed after the brief was accepted. Review and save the brief before any run can adopt it."
        );
        items.push(i);
    }
    for run in array(&v["runs"]) {
        let run_id = run["id"].as_str().unwrap_or("");
        let task_id = &run["taskId"];
        let Some(state) = runs.get(run_id) else {
            continue;
        };
        if !ended(run) {
            for assignment in array(&state.runtime["assignments"]) {
                let limit = 2 + array(&assignment["repairAllowances"]).len() as u64;
                let used = assignment["automaticCycles"].as_u64().unwrap_or(0);
                if used >= limit && !present(&assignment["retiredAt"]) {
                    let mut i = item(
                        "repair",
                        format!("repair-{}", assignment["id"].as_str().unwrap_or("")),
                        Some(run),
                        task_id,
                        format!(
                            "{} used {used} of {limit} automatic repair cycles",
                            assignment["name"].as_str().unwrap_or("Assignment")
                        ),
                    );
                    i["assignmentId"] = assignment["id"].clone();
                    i["detail"] = json!(
                        "Further automatic repair is blocked until you allow one more cycle."
                    );
                    items.push(i);
                }
                let current = array(&state.runtime["attempts"])
                    .iter()
                    .find(|a| a["id"] == assignment["currentAttemptId"]);
                if let Some(attempt) = current.filter(|a| a["ownership"] == "uncertain") {
                    let mut i = item(
                        "ownership",
                        format!("ownership-{}", attempt["id"].as_str().unwrap_or("")),
                        Some(run),
                        task_id,
                        format!(
                            "Process ownership is uncertain for {}",
                            assignment["name"].as_str().unwrap_or("an assignment")
                        ),
                    );
                    i["attemptId"] = attempt["id"].clone();
                    i["detail"] = json!(attempt["ownershipReason"].as_str().unwrap_or(
                        "Crewroom could not confirm whether this agent process stopped. Inspect it, then Pause, Resume or Cancel."
                    ));
                    items.push(i);
                }
            }
        }
        let snapshot = &run["taskSnapshot"];
        if let Some(t) = task(task_id).filter(|t| {
            t["needsReacceptance"] != true
                && (!ended(run) || run["status"] == "completed")
                && (snapshot["brief"] != t["brief"]
                    || snapshot["criteria"] != t["criteria"]
                    || snapshot["sourceRevision"] != t["sourceRevision"])
        }) {
            let mut i = item(
                "requirements",
                format!("requirements-adopt-{run_id}"),
                Some(run),
                task_id,
                format!("Accepted brief revision {} awaits adoption", t["revision"]),
            );
            i["stage"] = json!("adopt");
            i["revision"] = t["revision"].clone();
            i["retainedRevision"] = run["taskRevision"].clone();
            i["reasonRequired"] = json!(
                array(&state.runtime["attempts"])
                    .iter()
                    .any(|a| a["state"] != "admitted")
            );
            items.push(i);
        }
        let publication = &state.publication;
        if crewroom_core::feedback_awaits_resume(run, publication) {
            let comments = array(&publication["feedback"]["comments"]);
            let mut i = item(
                "feedback",
                format!("feedback-{run_id}"),
                Some(run),
                task_id,
                format!(
                    "{} review comment{} on PR #{}",
                    comments.len(),
                    if comments.len() == 1 { "" } else { "s" },
                    publication["pr"]["id"]
                ),
            );
            i["detail"] = json!(run["feedbackResumeHold"]["reason"].as_str().unwrap_or(
                "Resume addresses them on the same branch and PR, with fresh checks and review."
            ));
            items.push(i);
        }
        for step in ["push", "pr"] {
            let outcome = publication[step]["state"].as_str().unwrap_or("");
            if ["uncertain", "blocked"].contains(&outcome) && !present(&publication["merged"]) {
                let mut i = item(
                    "publication",
                    format!("publication-{step}-{run_id}"),
                    Some(run),
                    task_id,
                    format!(
                        "{} outcome is {outcome}",
                        if step == "pr" { "PR" } else { "Push" }
                    ),
                );
                i["step"] = json!(step);
                i["available"] = json!(state.publication_available);
                i["detail"] = json!(publication[step]["reason"].as_str().unwrap_or(
                    "The provider result was not confirmed. Reconcile against the provider before retrying."
                ));
                items.push(i);
            }
        }
    }
    let jira = v["jira"].is_object();
    for update in array(&v["jiraUpdates"]) {
        let outcome = update["state"].as_str().unwrap_or("");
        if !["uncertain", "blocked"].contains(&outcome) {
            continue;
        }
        let run = array(&v["runs"])
            .iter()
            .find(|r| r["id"] == update["runId"]);
        let mut i = item(
            "jira",
            format!("jira-{}", update["id"].as_str().unwrap_or("")),
            run,
            run.map_or(&Value::Null, |r| &r["taskId"]),
            format!(
                "Jira {} update is {outcome}",
                update["kind"].as_str().unwrap_or("")
            ),
        );
        i["available"] = json!(jira);
        i["detail"] = json!(update["reason"].as_str().unwrap_or(
            "Crewroom could not confirm whether Jira applied this update. Reconcile before it is retried."
        ));
        items.push(i);
    }
    for update in array(&v["githubUpdates"]) {
        let outcome = update["state"].as_str().unwrap_or("");
        if !["uncertain", "blocked"].contains(&outcome)
            || update["reason"]
                .as_str()
                .is_some_and(|r| r.starts_with("Superseded by the merge"))
        {
            continue;
        }
        let run = array(&v["runs"])
            .iter()
            .find(|r| r["id"] == update["runId"]);
        let mut i = item(
            "github",
            format!("github-{}", update["id"].as_str().unwrap_or("")),
            run,
            run.map_or(&Value::Null, |r| &r["taskId"]),
            format!(
                "GitHub {} update for {} is {outcome}",
                update["kind"].as_str().unwrap_or(""),
                update["issueKey"].as_str().unwrap_or("the issue")
            ),
        );
        i["available"] = json!(true);
        i["updateId"] = update["id"].clone();
        i["detail"] = json!(update["reason"].as_str().unwrap_or(
            "Crewroom could not confirm whether GitHub applied this update. Check the issue, then retry or reconcile."
        ));
        items.push(i);
    }
    for proposal in array(&v["learning"]) {
        let retry =
            proposal["status"] == "approved" && proposal["decision"]["pendingRoleSave"] == true;
        if proposal["status"] != "pending" && !retry {
            continue;
        }
        let run = array(&v["runs"])
            .iter()
            .find(|r| r["id"] == proposal["runId"]);
        let target = &proposal["target"];
        let subject = if target["kind"] == "role" {
            let roles = array(&v["roles"]);
            let role = roles.iter().find(|r| r["id"] == target["roleId"]);
            format!(
                "role {}",
                role.and_then(|r| r["name"].as_str())
                    .or(target["roleId"].as_str())
                    .unwrap_or("unavailable")
            )
        } else {
            "project guidance".into()
        };
        let mut i = item(
            "learning",
            format!("learning-{}", proposal["id"].as_str().unwrap_or("")),
            run,
            run.map_or(&Value::Null, |r| &r["taskId"]),
            if retry {
                format!("Approved change to {subject} was not saved")
            } else {
                format!(
                    "{} proposes a change to {subject}",
                    if proposal["origin"] == "operator" {
                        "Operator"
                    } else {
                        "Coordinator"
                    }
                )
            },
        );
        i["learningId"] = proposal["id"].clone();
        i["retry"] = json!(retry);
        i["detail"] = proposal["rationale"].clone();
        i["change"] = proposal["change"].clone();
        if retry {
            i["decision"] = json!({"reason":proposal["decision"]["reason"],"requestId":proposal["decision"]["requestId"]});
        }
        items.push(i);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(runtime: Value, publication: Value) -> RunState {
        RunState {
            runtime,
            publication,
            publication_available: true,
        }
    }

    fn kinds(items: &[Value]) -> Vec<&str> {
        items.iter().map(|i| i["kind"].as_str().unwrap()).collect()
    }

    #[test]
    fn projects_each_pending_decision_and_drops_it_once_resolved() {
        let brief = json!({"brief":"b","criteria":["c"],"sourceRevision":1});
        let mut v = json!({
            "tasks":[{"id":"t","key":"K-1","revision":2,"brief":"b2","criteria":["c"],"sourceRevision":1}],
            "runs":[{"id":"r","taskId":"t","purpose":"coding","status":"paused","desiredState":"paused","taskRevision":1,"taskSnapshot":brief}],
            "learning":[{"id":"l","runId":"r","status":"pending","target":{"kind":"role","roleId":"reviewer"},"rationale":"why","change":"text"}],
            "roles":[{"id":"reviewer","name":"Independent reviewer"}],
            "jiraUpdates":[{"id":"j","runId":"r","kind":"review","state":"uncertain"}],
        });
        let runtime = json!({
            "assignments":[{"id":"a","name":"Implementer","automaticCycles":2,"currentAttemptId":"p"}],
            "attempts":[{"id":"p","state":"attention_required","ownership":"uncertain"}],
        });
        let publication = json!({"id":"pub","pr":{"state":"confirmed","id":7,"providerState":"OPEN"},"push":{"state":"uncertain"},"feedback":{"comments":[{"id":1}]}});
        let runs = HashMap::from([("r".to_owned(), state(runtime.clone(), publication.clone()))]);
        let items = needs_you(&v, &runs);
        assert_eq!(
            kinds(&items),
            [
                "repair",
                "ownership",
                "requirements",
                "feedback",
                "publication",
                "jira",
                "learning"
            ]
        );
        assert_eq!(items[2]["reasonRequired"], true);
        assert_eq!(items[5]["available"], false);
        assert_eq!(
            items[6]["title"],
            "Coordinator proposes a change to role Independent reviewer"
        );

        // Resolution anywhere removes the item: a granted cycle, stopped ownership, adoption,
        // a requested resume, reconciled delivery and a decided proposal.
        let runtime = json!({
            "assignments":[{"id":"a","name":"Implementer","automaticCycles":2,"repairAllowances":[{"id":"k"}],"currentAttemptId":"p"}],
            "attempts":[{"id":"p","state":"exited","ownership":"stopped"}],
        });
        let publication = json!({"id":"pub","pr":{"state":"confirmed","id":7,"providerState":"OPEN"},"push":{"state":"confirmed"},"feedback":{"comments":[{"id":1}]}});
        v["runs"][0]["taskSnapshot"] = json!({"brief":"b2","criteria":["c"],"sourceRevision":1});
        v["runs"][0]["resumeRequest"] = json!({"state":"queued"});
        v["jiraUpdates"][0]["state"] = json!("confirmed");
        v["learning"][0]["status"] = json!("rejected");
        let runs = HashMap::from([("r".to_owned(), state(runtime, publication))]);
        assert!(needs_you(&v, &runs).is_empty());
    }

    #[test]
    fn undelivered_github_issue_updates_ask_for_a_retry_with_their_reason() {
        let mut v = json!({
            "tasks":[{"id":"t","key":"#4"}],
            "runs":[{"id":"r","taskId":"t","purpose":"coding","status":"running","desiredState":"running"}],
            "githubUpdates":[
                {"id":"g1","runId":"r","kind":"milestone","issueKey":"#4","state":"blocked","reason":"GitHub denied this request (HTTP 403)."},
                {"id":"g2","runId":"r","kind":"in_review","issueKey":"#4","state":"blocked","reason":"Superseded by the merge."},
                {"id":"g3","runId":"r","kind":"evidence","issueKey":"#4","state":"pending","reason":"GitHub is not connected."},
            ],
        });
        let items = needs_you(&v, &HashMap::new());
        assert_eq!(kinds(&items), ["github"]);
        assert_eq!(items[0]["updateId"], "g1");
        assert_eq!(
            items[0]["title"],
            "GitHub milestone update for #4 is blocked"
        );
        assert_eq!(items[0]["detail"], "GitHub denied this request (HTTP 403).");
        assert_eq!(items[0]["runId"], "r");
        v["githubUpdates"][0]["state"] = json!("confirmed");
        assert!(needs_you(&v, &HashMap::new()).is_empty());
    }

    #[test]
    fn source_change_asks_for_the_brief_first_and_ended_runs_ask_nothing() {
        let v = json!({
            "tasks":[{"id":"t","key":"K-1","needsReacceptance":true,"revision":2,"brief":"b2"}],
            "runs":[{"id":"r","taskId":"t","purpose":"coding","status":"cancelled","desiredState":"cancelled","taskSnapshot":{"brief":"b"}}],
        });
        let runtime = json!({"assignments":[{"id":"a","name":"Implementer","automaticCycles":5,"currentAttemptId":"p"}],"attempts":[{"id":"p","ownership":"uncertain"}]});
        let publication = json!({"pr":{"state":"confirmed","id":7,"providerState":"OPEN"},"feedback":{"comments":[{"id":1}]}});
        let runs = HashMap::from([("r".to_owned(), state(runtime, publication))]);
        let items = needs_you(&v, &runs);
        assert_eq!(kinds(&items), ["requirements"]);
        assert_eq!(items[0]["stage"], "brief");
    }

    #[test]
    fn feedback_already_addressed_by_a_continuation_is_not_asked_again() {
        let feedback = json!({"comments":[{"id":1,"updated_on":"2026-10-01T00:00:00Z"}]});
        let v = json!({
            "tasks":[{"id":"t"}],
            "runs":[{"id":"r","taskId":"t","purpose":"coding","status":"paused","feedbackContinuations":[{"state":"candidate","publicationId":"pub","feedback":feedback}]}],
        });
        let publication = json!({"id":"pub","pr":{"state":"confirmed","id":7,"providerState":"OPEN"},"feedback":feedback});
        let runs = HashMap::from([("r".to_owned(), state(json!({}), publication.clone()))]);
        assert!(needs_you(&v, &runs).is_empty());
        let mut newer = publication;
        newer["feedback"]["comments"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":2}));
        let runs = HashMap::from([("r".to_owned(), state(json!({}), newer))]);
        assert_eq!(kinds(&needs_you(&v, &runs)), ["feedback"]);
    }
}
