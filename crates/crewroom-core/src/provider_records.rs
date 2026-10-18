//! Durable provider intents and observations. This module performs no network I/O.
use crate::{
    Result,
    store::{Store, conflict},
};
use rusqlite::params;
use serde_json::{Value, json};

use crate::source_time::parse as source_timestamp;
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
/// Feedback identity: GitHub reviews (`kind: "review"`) have their own id space,
/// so an item matches only by kind and id. Comments carry no kind.
fn same_feedback(a: &Value, b: &Value) -> bool {
    a["id"] == b["id"] && a["kind"] == b["kind"]
}
impl Store {
    pub fn coding_record(&self, run_id: &str) -> Result<Option<Value>> {
        self.get_run(run_id)?;
        self.record("coding", "run_id", run_id)
    }
    pub fn update_coding_record(
        &self,
        run_id: &str,
        update: impl FnOnce(Option<Value>) -> Result<Value>,
    ) -> Result<Value> {
        self.transaction(|s| {
            let record=update(s.coding_record(run_id)?)?;
            if record["runId"]!=run_id {return Err(conflict("Coding record belongs to another run."));}
            s.db.execute("INSERT INTO coding VALUES(?,?) ON CONFLICT(run_id) DO UPDATE SET data=excluded.data",params![run_id,record.to_string()])?;Ok(record)
        })
    }
    pub fn publications(&self) -> Result<Vec<Value>> {
        self.all("publications")
    }
    pub fn publication(&self, run_id: &str) -> Result<Option<Value>> {
        self.record("publications", "run_id", run_id)
    }
    pub fn save_publication(&self, value: &Value) -> Result<Value> {
        self.transaction(|s| {
            let mut record=value.clone();let run_id=value["runId"].as_str().unwrap();
            if let Some(current)=s.publication(run_id)?
                && current["id"]==record["id"]
                && let Some(feedback)=current.get("feedback")
                && (record.get("feedback").is_none() || feedback["observedAt"].as_str()>=record["feedback"]["observedAt"].as_str()) {record["feedback"]=feedback.clone();}
            s.db.execute("INSERT INTO publications VALUES(?,?) ON CONFLICT(run_id) DO UPDATE SET data=excluded.data",params![run_id,record.to_string()])?;
            if record.get("merged").is_some_and(|v|!v.is_null() && v!=false) {s.release_coding_slot_in_transaction(run_id,"merged",None)?;}
            s.account_time_in_transaction(run_id)?;Ok(record)
        })
    }
    pub fn save_publication_feedback(
        &self,
        run_id: &str,
        pr_id: u64,
        read: &Value,
        observed_at: &str,
    ) -> Result<Value> {
        self.transaction(|s| {
            let mut record=s.publication(run_id)?.ok_or_else(||conflict("The observed PR no longer matches this publication."))?;
            if record["pr"]["id"]!=pr_id || record["pr"]["state"]!="confirmed" {return Err(conflict("The observed PR no longer matches this publication."));}
            let previous=&record["feedback"];
            if previous["observedAt"].as_str().is_some_and(|at|at>observed_at) {return Ok(previous.clone());}
            let mut known=array(&previous["comments"]).to_vec();let mut inconsistent=false;
            for comment in array(&read["comments"]) {
                let index=known.iter().position(|c|same_feedback(c,comment));
                if let Some(index)=index {
                    let prior=&known[index];let old=prior["updated_on"].as_str().and_then(source_timestamp);let new=comment["updated_on"].as_str().and_then(source_timestamp);
                    // A review has no edit time, so its version cannot be compared: the newer observation wins.
                    let versioned=comment["kind"]!="review";
                    let earlier=versioned && matches!((new,old),(Some(new),Some(old)) if new<old);
                    let same=matches!((new,old),(Some(new),Some(old)) if new==old);
                    let conflict=versioned && same && (prior["content"]["raw"]!=comment["content"]["raw"] || prior["deleted"]!=comment["deleted"]);
                    if earlier || conflict {known[index]["currentness"]=json!(if conflict {"conflict"} else {"unknown"});inconsistent=true;}
                    else {known[index]=comment.clone();known[index]["currentness"]=json!("known");}
                } else {let mut comment=comment.clone();comment["currentness"]=json!("known");known.push(comment);}
            }
            for comment in &mut known {if !array(&read["comments"]).iter().any(|c|same_feedback(c,comment)) {comment["currentness"]=json!("unknown");}}
            known.sort_by(|a,b| {
                let created=match (a["created_on"].as_str().and_then(source_timestamp),b["created_on"].as_str().and_then(source_timestamp)) {(Some(a),Some(b))=>a.cmp(&b),_=>std::cmp::Ordering::Equal};
                created.then_with(||a["id"].as_u64().cmp(&b["id"].as_u64())).then_with(||a["kind"].as_str().cmp(&b["kind"].as_str()))
            });
            let mut feedback=json!({"observedAt":observed_at,"state":if inconsistent {json!("partial")} else {read["state"].clone()}});
            if inconsistent {feedback["reason"]=json!("The PR host returned an older or conflicting comment version; prior content was retained.");} else if let Some(reason)=read.get("reason") {feedback["reason"]=reason.clone();}
            feedback["comments"]=json!(known);record["feedback"]=feedback.clone();s.db.execute("UPDATE publications SET data=? WHERE run_id=?",params![record.to_string(),run_id])?;Ok(feedback)
        })
    }
    pub fn jira_updates(&self, run_id: Option<&str>) -> Result<Vec<Value>> {
        if let Some(run_id) = run_id.filter(|s| !s.is_empty()) {
            self.run_records("jira_updates", run_id)
        } else {
            self.all("jira_updates")
        }
    }
    pub fn save_jira_update(&self, update: &Value) -> Result<Value> {
        // Preserve the existing two durability boundaries: the provider receipt
        // survives a failed notification; retrying it deduplicates the notice.
        let run_id = update["runId"].as_str().unwrap();
        self.db.execute("INSERT INTO jira_updates VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![update["id"].as_str(),run_id,update["eventKey"].as_str(),update.to_string()])?;
        if ["confirmed", "uncertain", "blocked"].contains(&update["state"].as_str().unwrap_or("")) {
            let mut payload = json!({"kind":"jira","subjectId":update["id"],"eventKey":update["eventKey"],"state":update["state"]});
            if let Some(external) = update.get("externalId") {
                payload["externalId"] = external.clone();
            }
            self.notice_delivery(run_id, &payload)?;
        }
        Ok(update.clone())
    }
    /// GitHub issue delivery intents and receipts (FOR-48). Kept apart from
    /// Jira's so each provider's outbox is reconciled only against itself.
    pub fn github_updates(&self, run_id: Option<&str>) -> Result<Vec<Value>> {
        if let Some(run_id) = run_id.filter(|s| !s.is_empty()) {
            self.run_records("github_updates", run_id)
        } else {
            self.all("github_updates")
        }
    }
    pub fn save_github_update(&self, update: &Value) -> Result<Value> {
        let run_id = update["runId"].as_str().unwrap();
        self.db.execute("INSERT INTO github_updates VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![update["id"].as_str(),run_id,update["eventKey"].as_str(),update.to_string()])?;
        if ["confirmed", "uncertain", "blocked"].contains(&update["state"].as_str().unwrap_or("")) {
            let mut payload = json!({"kind":"github","subjectId":update["id"],"eventKey":update["eventKey"],"state":update["state"]});
            if let Some(external) = update.get("externalId") {
                payload["externalId"] = external.clone();
            }
            self.notice_delivery(run_id, &payload)?;
        }
        Ok(update.clone())
    }
}
