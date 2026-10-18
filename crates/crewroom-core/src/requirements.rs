//! Explicit adoption preserves historical briefs and invalidates revision evidence.
use crate::{
    Result,
    lifecycle::{Transition, transition},
    store::{Store, conflict, id, integer, remove, request_id, strict, trim_text, trimmed_text},
};
use rusqlite::params;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn requirements_hash(task: &Value, revision: Option<u64>) -> String {
    // Match the ordered JSON used by shared/requirements.ts, including omission
    // of an absent repository. Existing hashes remain untouched until adoption.
    let mut value = json!({"title":task["title"],"description":task["description"],"brief":task["brief"],"criteria":task["criteria"]});
    if let Some(repository) = task.get("repositoryId") {
        value["repositoryId"] = repository.clone();
    }
    if let Some(revision) = revision {
        value["adoptedRevision"] = json!(revision);
    }
    format!(
        "{:x}",
        Sha256::digest(crate::serialized::stringify(&value).as_bytes())
    )
}

impl Store {
    pub fn adopt_requirements(&self, run_id: &str, body: &Value) -> Result<Value> {
        strict(body, &["requestId", "revision", "reason"])?;
        let key = request_id(body)?;
        let revision = integer(body, "revision", 1, u64::MAX)?;
        let reason = body
            .get("reason")
            .map(|_| trimmed_text(body, "reason", 1, 4000))
            .transpose()?;
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if let Some(prior) = run["adoptions"].as_array().and_then(|a| a.iter().find(|a| a["requestId"] == key)) {
                if prior["toRevision"] != revision || prior["reason"].as_str() != reason { return Err(conflict("This adoption decision already has different content.")); }
                return Ok(run);
            }
            if run["desiredState"] == "cancelled" && run["status"] != "completed" { return Err(conflict("A cancelled run cannot adopt new requirements.")); }
            if s.record("publications","run_id",run_id)?.is_some_and(|p| p.get("merged").is_some_and(|v| !v.is_null())) { return Err(conflict("A merged run cannot adopt new requirements.")); }
            let task_id = run["taskId"].as_str().unwrap();
            let task = s.get_task(task_id)?;
            let revisions = s.requirement_revisions(task_id)?;
            if revisions.last().is_none_or(|r| r["revision"] != revision || r["sourceRevision"] != task["sourceRevision"])
                || task["revision"] != revision || task["needsReacceptance"] == true {
                return Err(conflict("Accept the current source and latest brief before adopting it."));
            }
            if trim_text(task["brief"].as_str().unwrap_or("")).is_empty() || task["criteria"].as_array().is_none_or(Vec::is_empty) { return Err(conflict("A non-empty brief and acceptance criteria are required before adoption.")); }
            if run["taskRevision"].as_u64().unwrap_or(0) >= revision { return Err(conflict("The run already uses this revision.")); }
            let runtime = s.runtime(run_id)?;
            let assignments = runtime["assignments"].as_array().unwrap();
            let active = runtime["attempts"].as_array().unwrap().iter().any(|a| a["state"] != "admitted") || run["status"] == "running"
                || (run["desiredState"] == "running" && !assignments.is_empty());
            if active && reason.is_none() { return Err(conflict("Active adoption requires an operator reason.")); }
            let at = s.time();
            if run["status"] == "completed" {
                if s.open_run(task_id)?.is_some() { return Err(conflict("Another run is open for this ticket. Resolve it before reopening the research outcome.")); }
                run["status"] = json!("paused"); run["desiredState"] = json!("paused"); run["controlRevision"] = json!(run["controlRevision"].as_u64().unwrap_or(0)+1);
            }
            let from = run["taskRevision"].clone();
            let mut adoption = json!({"requestId":key,"fromRevision":from,"toRevision":revision,"at":at});
            if let Some(reason) = reason { adoption["reason"] = json!(reason); }
            if !run["adoptions"].is_array() { run["adoptions"] = json!([]); }
            run["adoptions"].as_array_mut().unwrap().push(adoption);
            let changed: Vec<_> = run["taskSnapshot"]["criteria"].as_array().unwrap().iter().enumerate().filter_map(|(i,c)| (task["criteria"][i] != *c).then_some(i)).collect();
            for mut assignment in assignments.iter().filter(|a| a["coordinator"] != true && a.get("retiredAt").is_none()).cloned() {
                let indices: Vec<_> = assignment["brief"]["criteria"].as_array().into_iter().flatten().filter_map(|item| {
                    let index = item["index"].as_u64()?;
                    (task["criteria"][index as usize] != item["criterion"]).then_some(index)
                }).collect();
                remove(&mut assignment,"rebriefRequired");
                if !indices.is_empty() { assignment["rebriefRequired"] = json!({"adoptedRevision":revision,"criterionIndices":indices}); }
                s.db.execute("UPDATE assignments SET data=? WHERE id=?", params![assignment.to_string(),assignment["id"].as_str()])?;
            }
            run["taskRevision"] = json!(revision); run["taskSnapshot"] = task.clone();
            if run["purpose"] == "research" { remove(&mut run,"researchCompletion"); }
            if run.get("sourceConflict").is_some() {
                remove(&mut run,"sourceConflict");
                if run["pauseReason"] == "source" && !active {
                    run["status"] = json!("awaiting_runner"); run["desiredState"] = json!("running");
                    remove(&mut run,"pauseReason"); remove(&mut run,"attentionReason");
                }
            }
            if active && ["running","awaiting_runner"].contains(&run["status"].as_str().unwrap_or("")) {
                run = transition(&run,&Transition::Desire{desired_state:"paused".into()},s.clock_ms())?;
                run = transition(&run,&Transition::BeginPause{reason:Some("requirements".into())},s.clock_ms())?;
            }
            s.save_run(&run)?;
            s.reject_outdated_insertions(run_id,revision)?;
            if let Some(mut work) = s.record("repository_work","run_id",run_id)? {
                work["requirementsHash"] = json!(requirements_hash(&task, Some(revision)));
                s.db.execute("UPDATE repository_work SET data=? WHERE run_id=?",params![work.to_string(),run_id])?;
            }
            if active {
                let message = json!({"id":id(),"runId":run_id,"createdAt":at,"delivery":"pending","transitions":[],"kind":"requirements_adoption",
                    "body":format!("Operator adopted accepted requirements revision {revision} from {from}. Reason: {}. Accepted task: {}. Changed criterion indices: {}. Existing assignment briefs remain historical; re-brief flagged assignments via the team proposal/retirement path at a safe point. Earlier coding evidence does not cover this revision.",reason.unwrap(),crate::serialized::stringify(&task),crate::serialized::stringify(&json!(changed)))});
                s.db.execute("INSERT INTO steering VALUES(?,?,?,?)",params![message["id"].as_str(),run_id,format!("adopt:{key}"),message.to_string()])?;
            }
            s.get_run(run_id)
        })
    }

    fn reject_outdated_insertions(&self, run_id: &str, revision: u64) -> Result<()> {
        for mut proposal in self.all("team_proposals")?.into_iter().filter(|p| {
            p["runId"] == run_id
                && p["origin"] == "operator"
                && p["status"] == "pending"
                && p["requirementRevision"] != revision
        }) {
            let reason = "Accepted requirements changed before insertion. Submit a new specialist proposal for the current requirements.";
            proposal["status"] = json!("rejected");
            proposal["reason"] = json!(reason);
            self.db.execute(
                "UPDATE team_proposals SET data=? WHERE id=?",
                params![proposal.to_string(), proposal["id"].as_str()],
            )?;
            for mut notice in self.all("delivery_notices")?.into_iter().filter(|n| {
                n["runId"] == run_id
                    && n["payload"]["subjectId"] == proposal["id"]
                    && n["payload"]["kind"] == "operator_specialist_proposed"
                    && n["disposition"] == "pending"
            }) {
                notice["payload"] = json!({"kind":"operator_specialist_proposed","subjectId":proposal["id"],"status":"rejected","reason":reason,
                    "instruction":"This operator proposal was rejected before admission. Retain its history; wait for a new operator request rather than recreating it."});
                self.db.execute(
                    "UPDATE delivery_notices SET data=? WHERE id=?",
                    params![notice.to_string(), notice["id"].as_str()],
                )?;
            }
            let notice = json!({"id":id(),"runId":run_id,"payload":{"kind":"team_proposal_rejected","subjectId":proposal["id"],"reason":reason,"origin":"operator"},"createdAt":self.time(),"disposition":"pending"});
            self.db.execute(
                "INSERT INTO delivery_notices VALUES(?,?,?)",
                params![notice["id"].as_str(), run_id, notice.to_string()],
            )?;
        }
        Ok(())
    }
}
