use super::*;
/// Pure policy over already verified observations. Unknown evidence holds
/// readiness; only confirmed regression latches a review invalidation.
pub fn assess(record: &Value, local: Option<&Value>, source_conflict: bool) -> Value {
    if present(&record["merged"]) {
        return json!({"ready":false,"blockers":[],"reviewInvalidated":false});
    }
    let mut blockers: Vec<String> = vec![];
    match local {
        None => blockers.push("Local evidence is unavailable.".into()),
        Some(l)
            if l["ready"] != true
                || l["candidate"]["id"] != record["candidateId"]
                || l["candidate"]["revision"] != record["revision"] =>
        {
            blockers.push("Local evidence does not cover the current publication revision.".into())
        }
        _ => {}
    }
    if source_conflict {
        blockers.push("Jira requirements changed.".into());
    }
    if let Some(error) = record["error"].as_str().filter(|s| !s.is_empty()) {
        blockers.push(error.into());
    }
    if record["push"]["state"] != "confirmed" || record["pr"]["state"] != "confirmed" {
        blockers.push("Push and PR creation both require confirmed receipts.".into());
    } else if record["pr"]["describedSha256"]
        != digest(record["description"].as_str().unwrap_or(""))
    {
        blockers.push("The PR description update has not been confirmed.".into());
    }
    if record["remoteSourceHead"] != record["revision"]["head"] {
        blockers.push("The remote source branch no longer matches the published candidate.".into());
    }
    if record["remoteTargetHead"] != record["revision"]["base"] {
        blockers.push("The remote target does not match the validated base.".into());
    }
    let observed = &record["observation"];
    let pr = &observed["pr"];
    let policy = &record["policy"];
    let revision_changed = present(observed)
        && (pr["source"]["commit"]["hash"] != record["revision"]["head"]
            || pr["destination"]["commit"]["hash"] != record["revision"]["base"]
            || pr["source"]["branch"]["name"] != record["branch"]
            || pr["destination"]["branch"]["name"] != record["target"]);
    if !present(observed) {
        blockers.push("Hosted validation has not been observed.".into());
    } else {
        if pr["state"] != "OPEN"
            || pr["draft"] == true
            || (present(&record["pr"]["providerState"]) && record["pr"]["providerState"] != "OPEN")
        {
            blockers.push("The pull request is not open for human review.".into());
        }
        if revision_changed {
            blockers.push("The PR head, base or branch no longer matches local evidence.".into());
        }
        for key in array(&policy["statusKeys"]) {
            let status = array(&observed["statuses"])
                .iter()
                .find(|s| s["key"] == *key);
            if status.is_none_or(|s| {
                s["state"] != "SUCCESSFUL"
                    || (present(&s["refname"])
                        && s["refname"] != ""
                        && s["refname"] != record["branch"])
            }) {
                blockers.push(format!(
                    "Hosted status {} does not confirm success for this branch and revision.",
                    key.as_str().unwrap_or("")
                ));
            }
        }
        let pipeline = &observed["pipelines"][0];
        if !pipeline_matches(record, pipeline) {
            blockers.push("The required PR pipeline has not validated this head and base.".into());
        } else {
            if !successful(pipeline) {
                blockers.push("The required PR pipeline is incomplete or unsuccessful.".into());
            }
            for name in array(&policy["pipelineSteps"]) {
                let matching = array(&pipeline["steps"])
                    .iter()
                    .filter(|s| s["name"] == *name)
                    .collect::<Vec<_>>();
                if matching.len() != 1 || !successful(matching[0]) {
                    blockers.push(format!(
                        "Pipeline step {} is missing, ambiguous, skipped or unsuccessful.",
                        name.as_str().unwrap_or("")
                    ));
                }
            }
            if array(&pipeline["steps"]).iter().any(|s| {
                !array(&policy["pipelineSteps"]).contains(&s["name"])
                    && !array(&policy["optionalSteps"]).contains(&s["name"])
            }) {
                blockers
                    .push("The pipeline contains steps outside the accepted check policy.".into());
            }
        }
    }
    // GitHub reviewer decisions (FOR-55). A changes-requested review stands on
    // GitHub until that reviewer approves or it is dismissed, whichever revision
    // it was submitted at, so it holds readiness and names that revision. A
    // GitHub observation without decisions is unknown evidence, not approval.
    if present(observed)
        && (crate::providers::github_pulls::is_github(&record["repository"])
            || observed.get("reviews").is_some())
    {
        match observed["reviews"].as_array() {
            None => blockers.push("GitHub review decisions have not been observed.".into()),
            Some(reviews) => {
                for review in reviews.iter().filter(|r| r["state"] == "CHANGES_REQUESTED") {
                    let who = review["user"]["display_name"]
                        .as_str()
                        .map_or_else(|| "A reviewer".to_owned(), |n| format!("Reviewer {n}"));
                    let at = match review["commit"].as_str() {
                        Some(c) if record["revision"]["head"] == c => {
                            format!("at this revision {}", &c[..c.len().min(12)])
                        }
                        Some(c) => format!("at earlier revision {}", &c[..c.len().min(12)]),
                        None => "at an unrecorded revision".into(),
                    };
                    blockers.push(format!(
                        "{who} requested changes on GitHub {at}. It stands until they approve or the review is dismissed."
                    ));
                }
            }
        }
    }
    let failed_hosted = present(observed)
        && pr["source"]["commit"]["hash"] == record["revision"]["head"]
        && (array(&policy["statusKeys"]).iter().any(|key| {
            array(&observed["statuses"])
                .iter()
                .find(|s| s["key"] == *key)
                .is_some_and(|s| s["state"] == "FAILED")
        }) || array(&observed["pipelines"]).iter().any(|p| {
            pipeline_matches(record, p)
                && (p["state"]["result"]["name"] == "FAILED"
                    || array(&p["steps"]).iter().any(|s| {
                        array(&policy["pipelineSteps"]).contains(&s["name"])
                            && s["state"]["result"]["name"] == "FAILED"
                    }))
        }));
    let regression = source_conflict
        || local.is_some_and(|l| {
            l["confirmedInvalidation"] == true
                || (present(&l["candidate"]) && l["candidate"]["id"] != record["candidateId"])
        })
        || (present(&record["remoteSourceHead"])
            && record["remoteSourceHead"] != record["revision"]["head"])
        || (present(&record["remoteTargetHead"])
            && record["remoteTargetHead"] != record["revision"]["base"])
        || revision_changed
        || failed_hosted;
    json!({"ready":blockers.is_empty(),"reviewInvalidated":!blockers.is_empty() && (record["reviewInvalidated"]==true || regression),"blockers":blockers})
}
fn successful(v: &Value) -> bool {
    v["state"]["name"] == "COMPLETED" && v["state"]["result"]["name"] == "SUCCESSFUL"
}
fn pipeline_matches(record: &Value, p: &Value) -> bool {
    present(p)
        && p["target"]["commit"]["hash"] == record["revision"]["head"]
        && p["target"]["destination_commit"]["hash"] == record["revision"]["base"]
        && p["target"]["pullrequest"]["id"] == record["pr"]["id"]
}
pub(super) fn regressed(now: &Value, previous: &Value) -> bool {
    fn older(a: &Value, b: &Value) -> bool {
        a.as_str().and_then(crewroom_core::source_time::parse)
            < b.as_str().and_then(crewroom_core::source_time::parse)
    }
    let n = &now["pipelines"][0];
    let p = &previous["pipelines"][0];
    older(&now["pr"]["updated_on"], &previous["pr"]["updated_on"])
        || array(&now["statuses"]).iter().any(|s| {
            array(&previous["statuses"])
                .iter()
                .any(|old| old["key"] == s["key"] && older(&s["updated_on"], &old["updated_on"]))
        })
        || (present(n)
            && present(p)
            && now["pr"]["source"]["commit"]["hash"] == previous["pr"]["source"]["commit"]["hash"]
            && (
                n["build_number"].as_i64().unwrap_or(0),
                n["run_number"].as_i64().unwrap_or(0),
            ) < (
                p["build_number"].as_i64().unwrap_or(0),
                p["run_number"].as_i64().unwrap_or(0),
            ))
}
