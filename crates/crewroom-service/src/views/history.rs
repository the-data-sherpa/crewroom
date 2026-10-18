use crate::{array, present};
use crewroom_core::{Result, serialized::stringify, store::Store};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(super) fn model(runtime: &Value, attempt: &Value) -> Value {
    attempt.get("model").cloned().unwrap_or_else(|| {
        array(&runtime["assignments"])
            .iter()
            .find(|a| a["id"] == attempt["assignmentId"])
            .map(|a| a["model"].clone())
            .unwrap_or(Value::Null)
    })
}
fn add(value: &mut Value, key: &str, n: f64) {
    value[key] = json!(value[key].as_f64().unwrap_or(0.) + n);
}
fn tokens(sum: &mut Value, v: &Value) {
    if sum.is_null() {
        *sum = json!({});
    }
    for key in [
        "input",
        "output",
        "cacheRead",
        "cacheWrite",
        "reasoning",
        "total",
    ] {
        if let Some(n) = v[key].as_f64() {
            add(sum, key, n);
        }
    }
}
pub(super) fn usage(store: &Store, id: &str) -> Result<Value> {
    let runtime = store.runtime(id)?;
    let mut seen = HashSet::new();
    let mut receipts = HashMap::<String, u64>::new();
    let mut snapshots = HashMap::<String, Value>::new();
    let mut rows:Vec<Value>=array(&runtime["attempts"]).iter().map(|a|{
        let assignment=array(&runtime["assignments"]).iter().find(|r|r["id"]==a["assignmentId"]).unwrap_or(&Value::Null);
        json!({"attemptId":a["id"],"assignmentId":a["assignmentId"],"name":assignment["name"],"generation":a["generation"],"model":model(&runtime,a)["resolved"]["selector"],"awaitingStart":a["state"]=="admitted","state":"unavailable","providerRequests":0,"actualResponses":0,"requestCounts":if ["codex","claude-code"].iter().any(|k|model(&runtime,a)["resolved"]["adapter"]==*k){"unavailable"}else{"available"},"tokens":null,"unavailableReasons":[],"responses":0,"costResponses":0,"costUsd":null})
    }).collect();
    for item in
        store.agent_events_of_type(id, &["provider_request", "provider_usage", "message_end"])?
    {
        let Some(row) = rows
            .iter_mut()
            .find(|r| r["attemptId"] == item["attemptId"])
        else {
            continue;
        };
        let event = &item["event"];
        if event["type"] == "provider_request" {
            add(row, "providerRequests", 1.);
            continue;
        }
        if event["type"] == "provider_usage" {
            if event["state"] == "snapshot"
                && ["codex-native", "claude-native"]
                    .iter()
                    .any(|s| event["source"] == *s)
            {
                let Some(attempt) = array(&runtime["attempts"])
                    .iter()
                    .find(|a| a["id"] == item["attemptId"])
                else {
                    continue;
                };
                let adapter = model(&runtime, attempt)["resolved"]["adapter"].clone();
                if event["sessionId"] != attempt["sessionId"]
                    || (event["source"] == "claude-native" && adapter != "claude-code")
                    || (event["source"] == "codex-native" && adapter != "codex")
                {
                    continue;
                }
                let key = format!("{}:{}", row["assignmentId"], event["sessionId"]);
                let previous = snapshots.entry(key).or_insert(json!({}));
                let mut delta = json!({});
                for key in [
                    "input",
                    "output",
                    "cacheRead",
                    "cacheWrite",
                    "reasoning",
                    "total",
                ] {
                    let Some(current) = event["usage"][key].as_u64() else {
                        continue;
                    };
                    let last = previous[key].as_u64().unwrap_or(0);
                    delta[key] = json!(current.saturating_sub(last));
                    previous[key] = json!(current.max(last));
                }
                tokens(&mut row["tokens"], &delta);
                if let Some(cost) = event["cost"].as_f64().filter(|n| n.is_finite() && *n >= 0.) {
                    let last = previous["cost"].as_f64().unwrap_or(0.);
                    add(row, "costUsd", (cost - last).max(0.));
                    previous["cost"] = json!(cost.max(last));
                    row["costBasis"] = json!("cumulative-list-price-estimate");
                }
                row["nativeSnapshots"] = json!(true);
                continue;
            }
            *receipts
                .entry(row["attemptId"].as_str().unwrap_or("").into())
                .or_default() += 1;
            if event["state"] == "actual" {
                add(row, "actualResponses", 1.);
                tokens(&mut row["tokens"], &event["usage"]);
            } else {
                let reasons = row["unavailableReasons"].as_array_mut().unwrap();
                if reasons.len() < 3 && !reasons.contains(&event["reason"]) {
                    reasons.push(event["reason"].clone());
                }
            }
            continue;
        }
        let message = &event["message"];
        if message["role"] != "assistant" {
            continue;
        }
        let Some(attempt) = array(&runtime["attempts"])
            .iter()
            .find(|a| a["id"] == item["attemptId"])
        else {
            continue;
        };
        let identity = if message.get("timestamp").is_none() {
            format!("{}:{}", attempt["id"], item["sequence"])
        } else {
            format!("{}:{}", attempt["sessionId"], stringify(message))
        };
        if !seen.insert(identity) {
            continue;
        }
        add(row, "responses", 1.);
        let selected = model(&runtime, attempt);
        if message["provider"] == selected["resolved"]["provider"]
            && message["model"] == selected["resolved"]["id"]
            && let Some(cost) = message["usage"]["cost"]["total"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.)
        {
            add(row, "costUsd", cost);
            add(row, "costResponses", 1.);
        }
    }
    for row in &mut rows {
        if row["requestCounts"] == "unavailable" {
            row["state"] = json!(if row["nativeSnapshots"] == true {
                "partial"
            } else {
                "unavailable"
            });
            row["unavailableReasons"] = json!([
                "Native harness reports cumulative tokens. Request counts and unreported in-flight usage are unavailable. Any cost shown is a list-price estimate, not a subscription charge or billing. Recovered totals are attributed when first observed."
            ]);
            continue;
        }
        let requests = row["providerRequests"].as_f64().unwrap_or(0.) as u64;
        let received = *receipts
            .get(row["attemptId"].as_str().unwrap_or(""))
            .unwrap_or(&0);
        if requests > received {
            let stopped = array(&runtime["attempts"])
                .iter()
                .find(|a| a["id"] == row["attemptId"])
                .is_some_and(|a| a["ownership"] == "stopped");
            row["unavailableReasons"]
                .as_array_mut()
                .unwrap()
                .push(json!(format!(
                    "{} provider request(s) {}",
                    requests - received,
                    if stopped {
                        "ended without a usage receipt."
                    } else {
                        "have no usage receipt yet."
                    }
                )));
        }
        if requests == 0 {
            row["unavailableReasons"]
                .as_array_mut()
                .unwrap()
                .push(json!("No provider request was recorded for this attempt."));
        }
        let actual = row["actualResponses"].as_f64().unwrap_or(0.) as u64;
        row["state"] = json!(if requests > 0 && actual == requests {
            "actual"
        } else if actual > 0 {
            "partial"
        } else {
            "unavailable"
        });
    }
    let started: Vec<_> = rows.iter().filter(|r| r["awaitingStart"] != true).collect();
    let mut total = json!({"providerRequests":0,"actualResponses":0,"tokens":null,"costUsd":null,"responses":0,"costResponses":0});
    for row in &started {
        for key in [
            "providerRequests",
            "actualResponses",
            "responses",
            "costResponses",
        ] {
            add(&mut total, key, row[key].as_f64().unwrap_or(0.));
        }
        if !row["tokens"].is_null() {
            tokens(&mut total["tokens"], &row["tokens"]);
        }
        if let Some(cost) = row["costUsd"].as_f64() {
            add(&mut total, "costUsd", cost);
        }
        if row["costBasis"] == "cumulative-list-price-estimate" {
            total["costBasis"] = row["costBasis"].clone();
        }
    }
    let unknown = started
        .iter()
        .filter(|r| r["requestCounts"] == "unavailable")
        .count();
    total["requestCounts"] = json!(if unknown == 0 {
        "available"
    } else if unknown == started.len() {
        "unavailable"
    } else {
        "partial"
    });
    total["state"] = json!(
        if !started.is_empty() && started.iter().all(|r| r["state"] == "actual") {
            "actual"
        } else if total["actualResponses"].as_f64().unwrap_or(0.) > 0. || !total["tokens"].is_null()
        {
            "partial"
        } else {
            "unavailable"
        }
    );
    total["incompleteAttempts"]=json!(started.iter().filter(|r|r["state"]!="actual").map(|r|json!({"attemptId":r["attemptId"],"name":r["name"],"generation":r["generation"]})).collect::<Vec<_>>());
    Ok(
        json!({"total":total,"source":"Host gateway reports and native harness cumulative snapshots","billedUsd":null,"rows":rows,"note":"Missing usage is unavailable, never zero. Native snapshots are deduplicated across attempts; recovered increments are attributed when first observed. Runner cost estimates are not provider billing or subscription charges; native Codex cost is unavailable."}),
    )
}
fn string(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}

fn fallback(v: &Value, default: &str) -> String {
    if v.is_null() {
        default.into()
    } else {
        string(v)
    }
}
fn cause(event: &Value) -> Option<String> {
    Some(match event["type"].as_str()? {
        "process_exit" => format!(
            "Runner exited: {}{}.",
            if event["code"].is_null() {
                "exit code unknown".into()
            } else {
                format!("code {}", string(&event["code"]))
            },
            if present(&event["signal"]) {
                format!(" · signal {}", string(&event["signal"]))
            } else {
                String::new()
            }
        ),
        "prompt_result" if event["status"] == "error" => format!(
            "Turn failed: {}",
            if event["error"].is_string() {
                string(&event["error"])
            } else {
                fallback(
                    &event["error"]["message"],
                    "cause unknown; no error detail recorded",
                )
            }
        ),
        "model_policy_rejected" if event["code"] == "input_budget" => format!(
            "Provider input budget rejected: {}. {}",
            fallback(&event["cause"], "cause unavailable"),
            if event["recovery"] == "fresh_session" {
                "Explicit Resume starts a fresh model session."
            } else {
                "Correct the configuration before resuming."
            }
        ),
        "model_policy_rejected" => format!(
            "Model policy rejected a provider request. Approved: {} · {}.",
            fallback(&event["requestedModel"], "unknown model"),
            fallback(&event["requestedEffort"], "unknown effort")
        ),
        "ownership_unresolved" => format!(
            "Ownership unresolved: {}",
            fallback(&event["reason"], "cause unknown; no reason recorded")
        ),
        _ => return None,
    })
}
fn next(run: &Value) -> &'static str {
    if ["closed", "completed", "cancelled"].contains(&run["status"].as_str().unwrap_or("")) {
        return "Inspect retained history and evidence.";
    }
    if run["desiredState"] == "cancelled" {
        return "Wait for confirmed stop or resolve the recorded ownership hold. This run cannot resume.";
    }
    if present(&run["launchHold"]) {
        return if run["launchHold"]["source"] == "preflight" {
            "Correct the launch prerequisites; the queue rechecks them automatically."
        } else {
            "Correct the preparation hold, then use Save launch details to retry."
        };
    }
    if run["feedbackResumeHold"]["terminal"] == true {
        return "Inspect the PR status and recorded feedback hold.";
    }
    if run["pauseReason"] == "budget"
        || run["activeMs"].as_f64().unwrap_or(0.)
            >= run["activeLimitMs"].as_f64().unwrap_or(3600000.)
    {
        return "Set a higher total active-time limit before requesting Resume.";
    }
    if present(&run["resumeRequest"]) {
        return "Wait for the queued Resume; inspect the coding slot and process ownership if it remains held.";
    }
    match run["status"].as_str() {
        Some("attention_required") => {
            "Inspect process and recovery details. Resolve ownership or delivery holds before requesting Resume."
        }
        Some("paused") => {
            "Inspect the retained result; use Resume when ready and the runner is available."
        }
        Some("pausing") => {
            "Wait for stop confirmation; inspect process ownership if it remains unresolved."
        }
        _ if run.get("position").is_some() => {
            "Inspect the queue hold and launch details; eligible coding plans start automatically."
        }
        _ => "Inspect the coordinator conversation and saved evidence.",
    }
}
pub(super) fn read(store: &Store, id: &str) -> Result<Value> {
    let run = store.get_run(id)?;
    let runtime = store.runtime(id)?;
    let coding = store.coding_record(id)?.unwrap_or(Value::Null);
    let research = store.research_artifacts(id)?;
    let proposals = store.refinement_proposals(Some(id))?;
    let events = store.agent_events_of_type(
        id,
        &[
            "process_exit",
            "prompt_result",
            "model_policy_rejected",
            "ownership_unresolved",
        ],
    )?;
    let revision = |candidate: &Value| {
        array(&coding["candidates"])
            .iter()
            .find(|c| c["id"] == *candidate)
            .map(|c| {
                format!(
                    "Head {} · base {} · tree {}",
                    string(&c["revision"]["head"]),
                    string(&c["revision"]["base"]),
                    string(&c["revision"]["tree"])
                )
            })
            .unwrap_or_else(|| "Revision unknown: candidate unavailable".into())
    };
    let mut attempts = Vec::new();
    for attempt in array(&runtime["attempts"]) {
        let assignment = array(&runtime["assignments"])
            .iter()
            .find(|a| a["id"] == attempt["assignmentId"])
            .unwrap_or(&Value::Null);
        let current =
            assignment["currentAttemptId"] == attempt["id"] && !present(&assignment["retiredAt"]);
        let mut causes = Vec::new();
        let mut add = |reason: String| {
            if !causes.contains(&reason) {
                causes.push(reason);
            }
        };
        for item in events.iter().filter(|e| e["attemptId"] == attempt["id"]) {
            if let Some(reason) = cause(&item["event"]) {
                add(reason);
            }
        }
        for note in array(&attempt["ownershipNotes"]) {
            add(string(&note["reason"]));
        }
        if let Some(reason) = attempt["ownershipReason"].as_str() {
            add(reason.into());
        }
        if present(&assignment["retiredAt"]) && assignment["currentAttemptId"] == attempt["id"] {
            add(format!(
                "Assignment retired: {}",
                fallback(
                    &assignment["retirementReason"],
                    "reason unknown; not recorded"
                )
            ));
        }
        if present(&attempt["fallback"]) {
            add(format!(
                "Configured fallback from {}: {}",
                string(&attempt["fallback"]["fromModel"]),
                string(&attempt["fallback"]["reason"])
            ));
        }
        if causes.is_empty()
            && ["failed", "exited", "attention_required"]
                .contains(&attempt["state"].as_str().unwrap_or(""))
        {
            causes.push("Cause unknown: no reason was recorded for this attempt.".into());
        }
        let mut evidence = Vec::new();
        for c in array(&coding["candidates"])
            .iter()
            .filter(|c| array(&c["implementationAttempts"]).contains(&attempt["id"]))
        {
            evidence.push(json!({"id":c["id"],"label":"Implementation candidate","at":c["createdAt"],"revision":revision(&c["id"])}));
        }
        for c in array(&coding["checks"])
            .iter()
            .filter(|c| c["attemptId"] == attempt["id"])
        {
            let state = if c["started"] != true || c["completed"] != true {
                "incomplete".into()
            } else if c["timedOut"] == true {
                "timed out".into()
            } else if c["sourceIntact"] != true {
                "source changed".into()
            } else {
                format!("exit {}", string(&c["exitCode"]))
            };
            let mut v = json!({"id":c["id"],"label":format!("Check {} · {state}",string(&c["checkId"])),"revision":revision(&c["candidateId"]),"artifact":c["output"]});
            if let Some(at) = c.get("completedAt") {
                v["at"] = at.clone();
            }
            evidence.push(v);
        }
        for r in array(&coding["reviews"])
            .iter()
            .filter(|r| r["attemptId"] == attempt["id"])
        {
            evidence.push(json!({"id":r["id"],"label":format!("Review · {}",string(&r["verdict"])),"at":r["createdAt"],"revision":revision(&r["candidateId"]),"artifact":r["artifact"],"detail":r["summary"]}));
        }
        for r in research.iter().filter(|r| r["attemptId"] == attempt["id"]) {
            evidence.push(json!({"id":r["id"],"label":format!("{} · {}{}",string(&r["title"]),string(&r["state"]),if present(&r["supersededBy"]){" · superseded"}else{""}),"at":r["createdAt"],"revision":format!("Requirements revision {} · source {}",string(&r["requirementRevision"]),fallback(&r["pinnedCommit"],"not assigned")),"artifact":r["body"]}));
        }
        for p in proposals.iter().filter(|p| p["attemptId"] == attempt["id"]) {
            let joined = |key| {
                array(&p[key])
                    .iter()
                    .map(string)
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            evidence.push(json!({"id":p["id"],"label":format!("Refinement proposal · {}",string(&p["status"])),"at":p["createdAt"],"revision":format!("Requirements revision {} · source revision {}",string(&p["baseRevision"]),string(&p["sourceRevision"])),"detail":format!("{}\nCriteria: {}\nDecisions: {}\nUnresolved questions: {}",string(&p["brief"]),joined("criteria"),joined("decisions"),joined("unresolvedQuestions"))}));
        }
        let mut v = json!({"model":model(&runtime,attempt),"state":attempt["state"],"attemptId":attempt["id"],"name":assignment["name"],"generation":attempt["generation"],"current":current,"causes":causes,"evidence":evidence,"nextAction":if present(&assignment["retiredAt"]){"Inspect this retired assignment and its retained evidence. It cannot resume."}else if current {next(&run)}else{"Inspect this retained attempt and its evidence. Use the current attempt for further work."},"createdAt":attempt["createdAt"],"startedAt":attempt["startedAt"],"stoppedAt":attempt["stoppedAt"]});
        for key in ["retiredAt", "retirementReason"] {
            if let Some(value) = assignment.get(key) {
                v[key] = value.clone();
            }
        }
        for key in [
            "ownership",
            "ownershipReason",
            "observedModel",
            "runnerKind",
            "runnerVersion",
            "runnerProfile",
            "sessionId",
        ] {
            if let Some(value) = attempt.get(key) {
                v[key] = value.clone();
            }
        }
        attempts.push(v);
    }
    Ok(json!({"usage":usage(store,id)?,"attempts":attempts}))
}

#[cfg(test)]
mod native_usage_tests {
    use super::*;
    #[test]
    fn cumulative_replay_across_replacement_counts_each_token_once_and_stays_partial() {
        for adapter in ["codex", "claude-code"] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
            let task = store
                .create_task(&json!({"title":"Usage recovery","description":""}))
                .unwrap();
            let run = store
                .queue_refinement(
                    task["id"].as_str().unwrap(),
                    &json!({"revision":1,"purpose":"explore"}),
                )
                .unwrap();
            let run = run["id"].as_str().unwrap();
            let role = crewroom_core::roles::Roles::new(root.path().join("roles.json"))
                .read()
                .unwrap()
                .remove(0);
            let model = json!({"resolved":{"adapter":adapter,"provider":adapter,"api":if adapter=="codex"{"codex-app-server"}else{"claude-code-stream"},"id":"fixture","resolvedModel":"fixture-resolved","selector":format!("{adapter}/fixture")}});
            let admitted=store.admit_assignment(run,&json!({"name":"Coordinator","coordinator":true,"role":role,"model":model,"input":"Read usage"})).unwrap();
            let first = admitted["attempt"]["id"].as_str().unwrap();
            let claim = |id: &str| {
                store.claim_attempt(id).unwrap();
                // Stored identity fixture; this test starts no OS processes.
                store.record_attempt_identity(id,&json!({"hostPid":101,"runnerKind":adapter,"runnerPid":102,"sessionId":"same-thread","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
            };
            claim(first);
            store.set_desired_state(run, "running").unwrap();
            store.start_execution(run).unwrap();
            let snapshot = |n: u64| json!({"type":"provider_usage","state":"snapshot","source":if adapter=="codex"{"codex-native"}else{"claude-native"},"accounting":"cumulative","sessionId":"same-thread","usage":{"input":n,"total":n},"cost":if adapter=="codex"{Value::Null}else{json!(n as f64/1000.)}});
            store.record_agent_event(first, 1, &snapshot(100)).unwrap();
            store.record_agent_event(first, 2, &snapshot(100)).unwrap();
            store.begin_pause(run, None).unwrap();
            store.record_ownership(first, true, None).unwrap();
            store.finish_pause(run).unwrap();
            let next = store
                .replace_attempt(
                    admitted["assignment"]["id"].as_str().unwrap(),
                    &json!({"automatic":false}),
                )
                .unwrap();
            store.set_desired_state(run, "running").unwrap();
            let next = next["id"].as_str().unwrap();
            claim(next);
            // The first 75 tokens were persisted by the adapter just before a crash
            // but never reached the old host journal. Resume replays the whole snapshot.
            store.record_agent_event(next, 1, &snapshot(175)).unwrap();
            store.record_agent_event(next, 2, &snapshot(175)).unwrap();
            store.record_agent_event(next, 3, &snapshot(200)).unwrap();
            store.record_agent_event(next, 4, &snapshot(50)).unwrap();
            let mut wrong = snapshot(999);
            wrong["sessionId"] = json!("another-session");
            store.record_agent_event(next, 5, &wrong).unwrap();
            let result = usage(&store, run).unwrap();
            assert!(result["total"]["tokens"].get("reasoning").is_none());
            assert_eq!(result["total"]["tokens"]["total"], 200.0);
            assert_eq!(result["total"]["state"], "partial");
            assert_eq!(result["total"]["requestCounts"], "unavailable");
            assert!(
                result["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["requestCounts"] == "unavailable")
            );
            if adapter == "codex" {
                assert!(result["total"]["costUsd"].is_null());
            } else {
                assert!((result["total"]["costUsd"].as_f64().unwrap() - 0.2).abs() < 1e-12);
                assert_eq!(
                    result["total"]["costBasis"],
                    "cumulative-list-price-estimate"
                );
            }
            assert_eq!(result["total"]["providerRequests"], 0.0);
            assert_eq!(result["rows"][0]["tokens"]["total"], 100.0);
            assert_eq!(result["rows"][1]["tokens"]["total"], 100.0);
        }
    }
}
