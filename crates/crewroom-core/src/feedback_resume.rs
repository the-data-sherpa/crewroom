//! Explicit Resume pins feedback while retaining uncertain and terminal holds.
use crate::{
    Error, Result,
    dependencies::coding,
    lifecycle::Transition,
    store::{Store, conflict, id, invalid, remove},
};
use serde_json::{Value, json};

fn feedback_conflict(message: &str, terminal: bool) -> Error {
    Error::FeedbackResume {
        message: message.into(),
        terminal,
    }
}
fn open(state: &Value) -> bool {
    ["queued", "starting", "handed_off", "held"].contains(&state.as_str().unwrap_or(""))
}
fn versions(feedback: &Value) -> Value {
    let mut comments = feedback["comments"].as_array().cloned().unwrap_or_default();
    for comment in &mut comments {
        remove(comment, "currentness");
    }
    comments.sort_by_key(|c| {
        (
            c["id"].as_i64().unwrap_or(0),
            c["kind"].as_str().map(str::to_owned),
        )
    });
    json!(comments)
}
/// Read-only mirror of `queue_feedback_resume`'s gate: captured PR feedback that only an
/// explicit operator Resume can act on. In-flight, already-addressed or terminal feedback is excluded.
pub fn feedback_awaits_resume(run: &Value, publication: &Value) -> bool {
    let previous = run["feedbackContinuations"]
        .as_array()
        .and_then(|a| a.last())
        .cloned()
        .unwrap_or(Value::Null);
    if run["feedbackResumeHold"]["terminal"] == true
        || previous["state"] == "blocked"
        || run.get("resumeRequest").is_some()
        || (open(&previous["state"]) && previous["state"] != "held")
        || publication["feedback"]["comments"]
            .as_array()
            .is_none_or(Vec::is_empty)
        || publication["pr"]["state"] != "confirmed"
        || publication["pr"]["id"].as_i64().is_none_or(|n| n == 0)
        || publication.get("merged").is_some()
        || publication["pr"]["providerState"] != "OPEN"
        || !coding(run)
        || !["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
        || run["desiredState"] == "cancelled"
    {
        return false;
    }
    previous["state"] == "held"
        || previous["publicationId"] != publication["id"]
        || !crate::serialized::same(
            &versions(&previous["feedback"]),
            &versions(&publication["feedback"]),
        )
}
impl Store {
    pub fn queue_feedback_resume(&self, run_id: &str) -> Result<bool> {
        self.transaction(|s|{
            let mut run=s.get_run(run_id)?;let publication=s.record("publications","run_id",run_id)?.unwrap_or(Value::Null);
            let previous=run["feedbackContinuations"].as_array().and_then(|a|a.last()).cloned().unwrap_or(Value::Null);
            if run["feedbackResumeHold"]["terminal"]==true {return Err(feedback_conflict(run["feedbackResumeHold"]["reason"].as_str().unwrap_or("Feedback continuation is blocked."),true));}
            if previous["state"]=="blocked" {return Err(feedback_conflict(previous["holdReason"].as_str().unwrap_or("This feedback continuation is terminal."),true));}
            if publication["feedback"]["comments"].as_array().is_none_or(Vec::is_empty) || publication["pr"]["state"]!="confirmed" || publication["pr"]["id"].as_i64().is_none_or(|n|n==0) {
                if run.get("resumeRequest").is_some() && previous.is_object() {return Err(feedback_conflict("The captured PR feedback or delivery identity is unavailable. Retain it for inspection.",false));}return Ok(false);
            }
            if publication.get("merged").is_some() || publication["pr"]["providerState"]!="OPEN" {
                return Err(feedback_conflict("This PR is closed, merged or its open state is unconfirmed; feedback continuation is blocked.",publication.get("merged").is_some() || ["MERGED","DECLINED","SUPERSEDED"].contains(&publication["pr"]["providerState"].as_str().unwrap_or(""))));
            }
            if !coding(&run) || !["paused","attention_required"].contains(&run["status"].as_str().unwrap_or("")) || run["desiredState"]=="cancelled" {return Err(conflict("Feedback continuation requires a paused coding run."));}
            let work=s.record("repository_work","run_id",run_id)?.unwrap_or(Value::Null);
            if !work.is_object() || work["branch"]!=publication["branch"] || work["repository"]["path"]!=publication["repositoryPath"] {return Err(feedback_conflict("The owned branch or repository no longer matches the published PR.",true));}
            remove(&mut run,"feedbackResumeHold");
            if run.get("resumeRequest").is_some() {return Ok(s.coding_slot()?.is_some_and(|h|h["id"]!=run_id) || s.unstopped_coding_run(run_id)?.is_some());}
            let coding=s.record("coding","run_id",run_id)?.unwrap_or(Value::Null);
            let latest=coding["candidates"].as_array().and_then(|a|a.last());
            if let Some(latest)=latest && latest["id"]!=publication["candidateId"] {
                let same=previous.is_object() && latest["feedbackFrom"]["continuationId"]==previous["id"] && previous["publicationId"]==publication["id"]
                    && ((previous["state"]=="candidate" && previous["candidateId"]==latest["id"]) || open(&previous["state"]));
                if !same {return Err(feedback_conflict("A newer local candidate is not the published PR source. Validate or reconcile it before selecting new feedback.",false));}
                if previous["state"]=="held" {let selected=run.get_mut("feedbackContinuations").and_then(Value::as_array_mut).unwrap().last_mut().unwrap();selected["state"]=json!("handed_off");remove(selected,"holdReason");}
                run["resumeRequest"]=json!({"requestedAt":s.time(),"state":"queued"});s.save_run(&run)?;return Ok(true);
            }
            if previous.is_object() && open(&previous["state"]) {
                if previous["publicationId"]!=publication["id"] || previous["prId"]!=publication["pr"]["id"] || previous["branch"]!=publication["branch"] {return Err(feedback_conflict("The captured PR identity changed; this continuation cannot be reused.",true));}
                if previous["state"]=="held" {let selected=run.get_mut("feedbackContinuations").and_then(Value::as_array_mut).unwrap().last_mut().unwrap();selected["state"]=json!("queued");remove(selected,"holdReason");}
                run["resumeRequest"]=json!({"requestedAt":s.time(),"state":"queued"});s.save_run(&run)?;return Ok(true);
            }
            let same=previous["publicationId"]==publication["id"] && previous["prId"]==publication["pr"]["id"] && previous["sourceCandidateId"]==publication["candidateId"]
                && previous["sourceHead"]==publication["revision"]["head"] && crate::serialized::same(&versions(&previous["feedback"]),&versions(&publication["feedback"]));
            let at=s.time();
            if !same {
                if !run["feedbackContinuations"].is_array() {run["feedbackContinuations"]=json!([]);}
                run.get_mut("feedbackContinuations").and_then(Value::as_array_mut).unwrap().push(json!({"id":id(),"requestedAt":at,"publicationId":publication["id"],"repository":publication["repository"],"prId":publication["pr"]["id"],"branch":publication["branch"],"sourceCandidateId":publication["candidateId"],"sourceHead":publication["revision"]["head"],"feedback":publication["feedback"],"state":"queued"}));
            }
            run["resumeRequest"]=json!({"requestedAt":at,"state":"queued"});s.save_run(&run)?;Ok(true)
        })
    }
    pub fn hold_queued_feedback_resume(
        &self,
        run_id: &str,
        reason: &str,
        terminal: bool,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run.get("resumeRequest").is_none() {
                return Ok(run);
            }
            if let Some(selected) = run
                .get_mut("feedbackContinuations")
                .and_then(Value::as_array_mut)
                .and_then(|a| a.last_mut())
                && open(&selected["state"])
            {
                selected["state"] = json!(if terminal { "blocked" } else { "held" });
                selected["holdReason"] = json!(reason);
            }
            run["feedbackResumeHold"] = json!({"reason":reason,"terminal":terminal});
            remove(&mut run, "resumeRequest");
            s.save_run(&run)?;
            let held = s.change_run(
                run_id,
                Transition::Attention {
                    reason: reason.into(),
                },
            )?;
            if terminal
                && s.coding_slot()?.is_some_and(|h| h["id"] == run_id)
                && s.record("repository_work", "run_id", run_id)?
                    .is_none_or(|w| w.get("writer").is_none())
                && s.runtime(run_id)?["attempts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|a| a["state"] == "admitted" || a["ownership"] == "stopped")
            {
                s.db.execute("UPDATE coding_slot SET run_id=NULL WHERE id=1", [])?;
            }
            Ok(held)
        })
    }
    pub fn queue_review_resume(&self, run_id: &str) -> Result<bool> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            let holder = s.coding_slot()?;
            if holder.as_ref().is_none_or(|h| h["id"] == run_id)
                && s.unstopped_coding_run(run_id)?.is_none()
            {
                return Ok(false);
            }
            if !coding(&run) || run["slotRelease"]["reason"] != "review" {
                return Ok(false);
            }
            if !["paused", "attention_required"].contains(&run["status"].as_str().unwrap_or(""))
                || run["desiredState"] == "cancelled"
            {
                return Err(conflict("This review continuation is no longer available."));
            }
            if run.get("resumeRequest").is_none() {
                run["resumeRequest"] = json!({"requestedAt":s.time(),"state":"queued"});
                s.save_run(&run)?;
            }
            Ok(true)
        })
    }
    pub fn set_resume_request_state(&self, run_id: &str, state: &str) -> Result<Value> {
        if !["queued", "starting"].contains(&state) {
            return Err(invalid("Invalid Resume request state."));
        }
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run.get("resumeRequest").is_none() || run["resumeRequest"]["state"] == state {
                return Ok(run);
            }
            run["resumeRequest"]["state"] = json!(state);
            if state == "starting"
                && let Some(selected) = run
                    .get_mut("feedbackContinuations")
                    .and_then(Value::as_array_mut)
                    .and_then(|a| a.last_mut())
                && selected["state"] == "queued"
            {
                selected["state"] = json!("starting");
            }
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn clear_resume_request(&self, run_id: &str, disposition: &str) -> Result<Value> {
        if !["held", "cancelled"].contains(&disposition) {
            return Err(invalid("Invalid Resume disposition."));
        }
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run.get("resumeRequest").is_none() {
                return Ok(run);
            }
            if let Some(selected) = run
                .get_mut("feedbackContinuations")
                .and_then(Value::as_array_mut)
                .and_then(|a| a.last_mut())
                && ["queued", "starting"].contains(&selected["state"].as_str().unwrap_or(""))
            {
                selected["state"] = json!(disposition);
            }
            remove(&mut run, "resumeRequest");
            s.save_run(&run)?;
            s.get_run(run_id)
        })
    }
    pub fn mark_feedback_handoff(&self, run_id: &str, id: &str) -> Result<Option<Value>> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            let Some(selected) = run
                .get_mut("feedbackContinuations")
                .and_then(Value::as_array_mut)
                .and_then(|a| a.iter_mut().find(|c| c["id"] == id))
            else {
                return Ok(None);
            };
            let changed = selected["state"] == "starting";
            if changed {
                selected["state"] = json!("handed_off");
            }
            let saved = selected.clone();
            if changed {
                s.save_run(&run)?;
            }
            Ok(Some(saved))
        })
    }
    pub fn finish_feedback_continuation(
        &self,
        run_id: &str,
        id: &str,
        outcome: &Value,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            let Some(selected) = run
                .get_mut("feedbackContinuations")
                .and_then(Value::as_array_mut)
                .and_then(|a| a.iter_mut().find(|c| c["id"] == id))
            else {
                return Err(conflict("The feedback continuation is not active."));
            };
            let candidate = outcome.get("candidateId");
            if candidate.map_or(selected["state"] == "no_change", |c| {
                selected["state"] == "candidate" && selected["candidateId"] == *c
            }) {
                return Ok(selected.clone());
            }
            if selected["state"] == "candidate"
                && let Some(candidate) = candidate
            {
                let coding = s.record("coding", "run_id", run_id)?.unwrap_or(Value::Null);
                if let Some(successor) = coding["candidates"].as_array().and_then(|a| a.last())
                    && successor["id"] == *candidate
                    && successor["feedbackFrom"]["continuationId"] == id
                    && successor["feedbackFrom"]["sourceCandidateId"] == selected["candidateId"]
                {
                    selected["candidateId"] = successor["id"].clone();
                    let saved = selected.clone();
                    s.save_run(&run)?;
                    return Ok(saved);
                }
            }
            if !["starting", "handed_off"].contains(&selected["state"].as_str().unwrap_or("")) {
                return Err(conflict("The feedback continuation is not active."));
            }
            selected["state"] = json!(if candidate.is_some() {
                "candidate"
            } else {
                "no_change"
            });
            if let Some(candidate) = candidate {
                selected["candidateId"] = candidate.clone();
            }
            let saved = selected.clone();
            s.save_run(&run)?;
            Ok(saved)
        })
    }
}
