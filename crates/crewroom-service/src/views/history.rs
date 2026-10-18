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
                "Native harness reports cumulative tokens only. Request counts and unreported in-flight usage are unavailable, so these figures are partial: not zero and not complete. Recovered totals are attributed when first observed."
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
    for row in &mut rows {
        let attempt = array(&runtime["attempts"])
            .iter()
            .find(|a| a["id"] == row["attemptId"])
            .cloned()
            .unwrap_or(Value::Null);
        let (route, recorded) = billing_route(&model(&runtime, &attempt));
        row["billing"] = json!(route);
        row["billingRecorded"] = json!(recorded);
        if route == "api"
            && row["costUsd"].is_null()
            && row["awaitingStart"] != true
            && let Some(reasons) = row["unavailableReasons"].as_array_mut()
        {
            reasons.push(json!(
                "No list-price estimate was reported; the cost estimate is unavailable, not zero."
            ));
        }
        if route != "api" {
            // Dollar figures belong to API use only. Subscription and unknown
            // routes never show a list-price estimate as a cost.
            if row["costUsd"].is_number() {
                row["estimateWithheld"] = json!(true);
            }
            row["costUsd"] = Value::Null;
            row["costResponses"] = json!(0);
            if let Some(object) = row.as_object_mut() {
                object.remove("costBasis");
            }
        }
    }
    let routes: Vec<Value> = ["api", "subscription", "unknown"]
        .iter()
        .filter_map(|route| {
            let members: Vec<&Value> = rows.iter().filter(|r| r["billing"] == *route).collect();
            (!members.is_empty()).then(|| route_total(route, &members))
        })
        .collect();
    Ok(
        json!({"routes":routes,"source":"Host gateway reports and native harness cumulative snapshots","billedUsd":null,"rows":rows,"note":"Usage is grouped by billing route and never totalled across routes. API use shows a runner list-price estimate, not provider billing. Subscription use shows runs and tokens only, with no dollar figure. Unknown-route use shows no cost estimate. Missing usage is unavailable, never zero. Native snapshots are deduplicated across attempts; recovered increments are attributed when first observed."}),
    )
}
/// The billing route frozen with the attempt's model selection. Assignments
/// admitted before billing routes were recorded read as unknown.
fn billing_route(model: &Value) -> (&'static str, bool) {
    let offering = &model["selection"]["offering"]["billing"];
    let recorded = if offering.is_string() {
        offering
    } else {
        &model["selection"]["billingRoute"]
    };
    match recorded.as_str() {
        Some("api") => ("api", true),
        Some("subscription") => ("subscription", true),
        Some(_) => ("unknown", true),
        None => ("unknown", false),
    }
}
/// One billing route's usage. Totals never mix routes.
fn route_total(route: &str, members: &[&Value]) -> Value {
    let started: Vec<_> = members
        .iter()
        .filter(|r| r["awaitingStart"] != true)
        .collect();
    let mut total = json!({"billing":route,"providerRequests":0,"actualResponses":0,"tokens":null,"costUsd":null,"responses":0,"costResponses":0});
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
        if route == "api"
            && let Some(cost) = row["costUsd"].as_f64()
        {
            add(&mut total, "costUsd", cost);
        }
        if route == "api" && row["costBasis"] == "cumulative-list-price-estimate" {
            total["costBasis"] = row["costBasis"].clone();
        }
        if row["estimateWithheld"] == true {
            total["estimateWithheld"] = json!(true);
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
    total["awaitingStart"] = json!(started.is_empty());
    total["attempts"] = json!(started.len());
    total["assignments"] = json!(
        started
            .iter()
            .map(|r| r["assignmentId"].to_string())
            .collect::<HashSet<_>>()
            .len()
    );
    total["tokenAttempts"] = json!(started.iter().filter(|r| !r["tokens"].is_null()).count());
    total["unrecordedAttempts"] = json!(
        members
            .iter()
            .filter(|r| r["billingRecorded"] != true)
            .count()
    );
    total["incompleteAttempts"]=json!(started.iter().filter(|r|r["state"]!="actual").map(|r|json!({"attemptId":r["attemptId"],"name":r["name"],"generation":r["generation"]})).collect::<Vec<_>>());
    total
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
            let model = json!({"resolved":{"adapter":adapter,"provider":adapter,"api":if adapter=="codex"{"codex-app-server"}else{"claude-code-stream"},"id":"fixture","resolvedModel":"fixture-resolved","selector":format!("{adapter}/fixture")},"selection":{"offering":{"billing":"api"}}});
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
            assert!(result["routes"][0]["tokens"].get("reasoning").is_none());
            assert_eq!(result["routes"][0]["tokens"]["total"], 200.0);
            assert_eq!(result["routes"][0]["state"], "partial");
            assert_eq!(result["routes"][0]["requestCounts"], "unavailable");
            assert!(
                result["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["requestCounts"] == "unavailable")
            );
            if adapter == "codex" {
                assert!(result["routes"][0]["costUsd"].is_null());
            } else {
                assert!((result["routes"][0]["costUsd"].as_f64().unwrap() - 0.2).abs() < 1e-12);
                assert_eq!(
                    result["routes"][0]["costBasis"],
                    "cumulative-list-price-estimate"
                );
            }
            assert_eq!(result["routes"][0]["providerRequests"], 0.0);
            assert_eq!(result["rows"][0]["tokens"]["total"], 100.0);
            assert_eq!(result["rows"][1]["tokens"]["total"], 100.0);
            assert_eq!(result["routes"].as_array().unwrap().len(), 1);
            assert_eq!(result["routes"][0]["billing"], "api");
            assert!(result.get("total").is_none());
        }
    }

    /// One run with an assignment per billing shape. Each assignment reports a
    /// Claude-native cumulative snapshot of `tokens` and a list-price cost.
    fn mixed_run(assignments: &[(&str, Value, u64)]) -> (tempfile::TempDir, Value) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
        let task = store
            .create_task(&json!({"title":"Usage by route","description":""}))
            .unwrap();
        let task = task["id"].as_str().unwrap();
        store.refine(task,&json!({"revision":1,"sourceRevision":1,"brief":"Inspect usage","criteria":["Explain usage"]})).unwrap();
        // Research runs admit specialists beside the coordinator.
        let run = store
            .queue_research(
                task,
                &json!({"revision":2,"requestId":uuid::Uuid::new_v4().to_string()}),
            )
            .unwrap();
        let run = run["id"].as_str().unwrap().to_owned();
        let role = crewroom_core::roles::Roles::new(root.path().join("roles.json"))
            .read()
            .unwrap()
            .remove(0);
        let mut attempts = Vec::new();
        for (index, (name, selection, _)) in assignments.iter().enumerate() {
            let mut model = json!({"resolved":{"adapter":"claude-code","provider":"claude-code","api":"claude-code-stream","id":"fixture","resolvedModel":"fixture-resolved","selector":"claude-code/fixture"}});
            if !selection.is_null() {
                model["selection"] = selection.clone();
            }
            let admitted = store
                .admit_assignment(
                    &run,
                    &json!({"name":name,"coordinator":index==0,"role":role,"model":model,"input":"Read usage"}),
                )
                .unwrap();
            let id = admitted["attempt"]["id"].as_str().unwrap().to_owned();
            store.claim_attempt(&id).unwrap();
            // Stored identity fixture; this test starts no OS processes.
            store.record_attempt_identity(&id,&json!({"hostPid":101,"runnerKind":"claude-code","runnerPid":102,"sessionId":format!("session-{index}"),"hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
            attempts.push(id);
        }
        store.set_desired_state(&run, "running").unwrap();
        store.start_execution(&run).unwrap();
        for (index, (_, _, tokens)) in assignments.iter().enumerate() {
            store
                .record_agent_event(
                    &attempts[index],
                    1,
                    &json!({"type":"provider_usage","state":"snapshot","source":"claude-native","accounting":"cumulative","sessionId":format!("session-{index}"),"usage":{"input":tokens,"total":tokens},"cost":*tokens as f64/1000.}),
                )
                .unwrap();
        }
        let result = usage(&store, &run).unwrap();
        (root, result)
    }
    fn route<'a>(result: &'a Value, billing: &str) -> &'a Value {
        array(&result["routes"])
            .iter()
            .find(|r| r["billing"] == billing)
            .unwrap_or_else(|| panic!("no {billing} route in {result}"))
    }

    #[test]
    fn usage_groups_by_billing_route_without_cross_route_totals() {
        let (_root, result) = mixed_run(&[
            ("Coordinator", json!({"offering":{"billing":"api"}}), 100),
            (
                "Implementer",
                json!({"offering":{"billing":"subscription"}}),
                300,
            ),
            ("Reviewer", json!({"offering":{"billing":"unknown"}}), 50),
        ]);
        assert!(result.get("total").is_none(), "no cross-route total");
        let order: Vec<_> = array(&result["routes"])
            .iter()
            .map(|r| r["billing"].clone())
            .collect();
        assert_eq!(order, vec!["api", "subscription", "unknown"]);
        let api = route(&result, "api");
        assert!((api["costUsd"].as_f64().unwrap() - 0.1).abs() < 1e-12);
        assert_eq!(api["costBasis"], "cumulative-list-price-estimate");
        assert_eq!(api["tokens"]["total"], 100.0);
        assert_eq!(api["attempts"], 1);
        // Subscription: runs and tokens, never a dollar figure.
        let subscription = route(&result, "subscription");
        assert!(subscription["costUsd"].is_null());
        assert!(subscription.get("costBasis").is_none());
        assert_eq!(subscription["estimateWithheld"], true);
        assert_eq!(subscription["tokens"]["total"], 300.0);
        assert_eq!(subscription["attempts"], 1);
        assert_eq!(subscription["assignments"], 1);
        // Native accounting is partial: request counts are unavailable, not zero.
        assert_eq!(subscription["state"], "partial");
        assert_eq!(subscription["requestCounts"], "unavailable");
        let unknown = route(&result, "unknown");
        assert!(unknown["costUsd"].is_null());
        assert_eq!(unknown["tokens"]["total"], 50.0);
        assert_eq!(unknown["unrecordedAttempts"], 0);
        for row in array(&result["rows"]) {
            assert_eq!(row["billingRecorded"], true);
            if row["billing"] != "api" {
                assert!(row["costUsd"].is_null(), "{row}");
                assert!(row.get("costBasis").is_none(), "{row}");
            }
            assert!(
                array(&row["unavailableReasons"])
                    .iter()
                    .any(|r| r.as_str().unwrap().contains("not zero")),
                "{row}"
            );
        }
    }

    #[test]
    fn historical_assignment_without_a_recorded_route_is_unknown() {
        let (_root, result) = mixed_run(&[
            ("Coordinator", Value::Null, 120),
            (
                "Implementer",
                json!({"billingRoute":"subscription","offering":{}}),
                80,
            ),
        ]);
        let unknown = route(&result, "unknown");
        assert_eq!(unknown["unrecordedAttempts"], 1);
        assert!(unknown["costUsd"].is_null());
        assert_eq!(unknown["estimateWithheld"], true);
        assert_eq!(unknown["tokens"]["total"], 120.0);
        let legacy = array(&result["rows"])
            .iter()
            .find(|r| r["name"] == "Coordinator")
            .unwrap();
        assert_eq!(legacy["billing"], "unknown");
        assert_eq!(legacy["billingRecorded"], false);
        // The selection-level route is read when the offering omits it.
        assert_eq!(route(&result, "subscription")["tokens"]["total"], 80.0);
        assert!(
            array(&result["routes"])
                .iter()
                .all(|r| r["billing"] != "api")
        );
    }
}
