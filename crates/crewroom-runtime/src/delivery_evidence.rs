//! Provider-neutral parts of ticket delivery shared by Jira and GitHub issue
//! delivery: validating a reporter's request, and composing verified evidence
//! and research summaries bound to the durable facts that authorized them.
//! Provider names only appear in messages; no credentials are captured.
use crate::{
    conflict,
    evidence::{Evidence, Research},
    messaging::StoreAccess,
    providers::github_pulls,
    repository::{array, text},
};
use crewroom_core::{Error, Result, store::Store};
use serde_json::{Value, json};

pub(crate) fn present(value: &Value) -> bool {
    !value.is_null() && value != false
}

/// Validates a reporter request: the allowed fields per kind, a stable event
/// key and bounded body. `provider` names the target in refusals.
pub(crate) fn report(input: &Value, provider: &str) -> Result<Value> {
    let kind = input["kind"].as_str().unwrap_or("");
    let allowed: &[&str] = match kind {
        "milestone" | "blocker" => &["kind", "eventKey", "body"],
        "evidence" | "pr_link" => &["kind", "eventKey", "candidateId"],
        "in_progress" | "research_outcome" => &["kind", "eventKey"],
        _ => return Err(Error::Invalid(format!("Invalid {provider} report kind."))),
    };
    if input
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(Error::Invalid(format!(
            "Unrecognized {provider} report fields."
        )));
    }
    let event = input["eventKey"].as_str().unwrap_or("");
    if event.is_empty()
        || event.len() > 200
        || !event.as_bytes()[0].is_ascii_alphanumeric()
        || !event
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
        || kind == "in_progress" && event != "execution-started"
    {
        return Err(Error::Invalid(format!("Invalid {provider} event key.")));
    }
    let mut result = json!({"kind":kind,"eventKey":event});
    if allowed.contains(&"body") {
        let body = input["body"]
            .as_str()
            .ok_or_else(|| Error::Invalid(format!("Invalid {provider} report body.")))?;
        let body = crewroom_core::store::trim_text(body);
        if !(1..=12000).contains(&body.chars().count()) {
            return Err(Error::Invalid(format!(
                "Invalid {provider} report body length."
            )));
        }
        result["body"] = json!(body);
    }
    if allowed.contains(&"candidateId") {
        let id = input["candidateId"]
            .as_str()
            .filter(|id| crewroom_core::store::valid_uuid(id))
            .ok_or_else(|| Error::Invalid(format!("Invalid {provider} candidate UUID.")))?;
        result["candidateId"] = json!(id);
    }
    Ok(result)
}

/// The canonical web URL of a publication's confirmed PR on its own host.
pub(crate) fn pr_url(publication: &Value) -> String {
    let repository = &publication["repository"];
    if github_pulls::is_github(repository) {
        format!(
            "https://github.com/{}/{}/pull/{}",
            repository["owner"].as_str().unwrap_or(""),
            repository["name"].as_str().unwrap_or(""),
            publication["pr"]["id"]
        )
    } else {
        format!(
            "https://bitbucket.org/{}/{}/pull-requests/{}",
            repository["workspace"].as_str().unwrap_or(""),
            repository["slug"].as_str().unwrap_or(""),
            publication["pr"]["id"]
        )
    }
}

/// Bind an async evidence assessment to the durable facts used to authorize it.
/// Provider credentials and unrelated messages/observations are never captured.
pub(crate) struct Proof {
    run_id: String,
    publication: bool,
    provider: &'static str,
    value: Value,
}
impl Proof {
    fn read(s: &Store, id: &str, publication: bool) -> Result<Value> {
        let run = s.get_run(id)?;
        let runtime = s.runtime(id)?;
        let mut result = json!({
            "requirements": {"revision":run["taskRevision"],"snapshot":run["taskSnapshot"],"sourceConflict":run["sourceConflict"],"researchSource":run["researchSource"],"researchCompletion":run["researchCompletion"],"researchOutcome":run["researchOutcome"]},
            "coding":s.coding_record(id)?,"work":s.repository_work(id)?,"artifacts":s.research_artifacts(id)?,"insertion":s.insertion_blockers(id)?,
            "assignments":array(&runtime["assignments"]).iter().map(|a|json!({"id":a["id"],"currentAttemptId":a["currentAttemptId"],"retiredAt":a["retiredAt"],"rebriefRequired":a["rebriefRequired"]})).collect::<Vec<_>>()
        });
        if publication {
            let p = s.publication(id)?.unwrap_or(Value::Null);
            result["publication"] = json!({"id":p["id"],"candidateId":p["candidateId"],"repository":p["repository"],"revision":p["revision"],"pr":p["pr"],"remoteSourceHead":p["remoteSourceHead"],"observedPr":p["observation"]["pr"]});
        }
        Ok(result)
    }
    pub(crate) fn capture<S: StoreAccess>(
        store: &S,
        id: &str,
        publication: bool,
        provider: &'static str,
    ) -> Result<Self> {
        Ok(Self {
            run_id: id.into(),
            publication,
            provider,
            value: store.with_store(|s| Self::read(s, id, publication))?,
        })
    }
    pub(crate) fn check(&self, s: &Store) -> Result<()> {
        if Self::read(s, &self.run_id, self.publication)? != self.value {
            return Err(conflict(&format!(
                "Evidence changed before {} authorization.",
                self.provider
            )));
        }
        Ok(())
    }
}
pub(crate) struct VerifiedText {
    pub body: String,
    pub proof: Proof,
}

/// Verified coding evidence for the current candidate; with `publication`, the
/// confirmed open PR on its host is required and linked.
pub(crate) async fn evidence_text<S: StoreAccess>(
    store: &S,
    evidence: Option<&Evidence<S>>,
    provider: &'static str,
    run_id: &str,
    candidate_id: &str,
    publication: bool,
) -> Result<VerifiedText> {
    let evidence = evidence.ok_or_else(|| conflict("Verified evidence is unavailable."))?;
    let proof = Proof::capture(store, run_id, publication, provider)?;
    let assessment = evidence.assess(run_id).await?;
    if assessment["ready"] != true || assessment["candidate"]["id"] != candidate_id {
        return Err(conflict(
            "Evidence must cover the current verified candidate.",
        ));
    }
    store.with_store(|s|{
        proof.check(s)?;
        let record=s.coding_record(run_id)?.unwrap();
        let checks=array(&record["requiredChecks"]).iter().filter(|c|c["required"]==true).map(|c|c["id"].as_str().unwrap()).collect::<Vec<_>>().join(", ");
        let mut body=format!("Revision {}\nCandidate {candidate_id}\nRequired checks passed: {checks}\nIndependent review passed for this revision.\nMerge remains an operator decision.",text(&assessment["candidate"]["revision"],"head")?);
        if publication {
            let p=s.publication(run_id)?.unwrap_or(Value::Null);
            let expected=pr_url(&p);
            if p["candidateId"]!=candidate_id || p["pr"]["state"]!="confirmed" || p["remoteSourceHead"]!=p["revision"]["head"]
                || p["observation"]["pr"]["source"]["commit"]["hash"]!=p["revision"]["head"] || p["observation"]["pr"]["state"]!="OPEN"
                || !p["pr"]["url"].as_str().is_some_and(|u| u.eq_ignore_ascii_case(&expected)) { return Err(conflict("A confirmed PR receipt for this candidate is required.")); }
            body.push_str(&format!("\n\nPull request: {expected}\nHosted validation must be checked against the current PR revision."));
        }
        Ok(VerifiedText{body,proof})
    })
}

/// The verified research outcome summary for the accepted revision.
pub(crate) fn research_text<S: StoreAccess>(
    store: &S,
    research: Option<&Research<S>>,
    provider: &'static str,
    run_id: &str,
) -> Result<VerifiedText> {
    let research = research.ok_or_else(|| conflict("Research evidence is unavailable."))?;
    let proof = Proof::capture(store, run_id, false, provider)?;
    let result = research.read(run_id)?;
    store.with_store(|s|{
        proof.check(s)?;let run=s.get_run(run_id)?;
        if run["purpose"]!="research" || result["assessment"]["ready"]!=true || !present(&run["researchCompletion"]) || run["researchCompletion"]["revision"]!=run["taskRevision"] { return Err(conflict(&format!("A requested, fully verified research outcome is required before reporting to {provider}."))); }
        let artifacts=array(&result["artifacts"]).iter().filter(|a|a["state"]=="verified" && !present(&a["supersededBy"]) && a["requirementRevision"]==run["taskRevision"])
            .map(|a|format!("- {} ({}), SHA-256 {}\n  Limitations: {}",a["title"].as_str().unwrap(),a["kind"].as_str().unwrap(),a["body"]["sha256"].as_str().unwrap(),a["limitations"].as_str().unwrap())).collect::<Vec<_>>().join("\n");
        Ok(VerifiedText{body:format!("Research outcome completed for accepted revision {}.\n{artifacts}\nCrewroom reference: run {run_id}, /api/runs/{run_id}/result\nNo code change or PR is claimed.",run["taskRevision"]),proof})
    })
}
