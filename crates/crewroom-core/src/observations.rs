//! Append-only host receipts, with bounded streaming progress and durable cursors.
use crate::{
    Result,
    lifecycle::Transition,
    store::{Store, conflict, invalid},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

// Match the host's UTF-16 size budget while retaining only complete Unicode scalars.
// A truncation boundary never creates the lone surrogate JavaScript slice can emit.
fn bounded_progress(value: &str, limit: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= limit
        })
        .collect()
}

pub fn compact_observation(event: &Value) -> Value {
    if event["type"] != "message_update" {
        return event.clone();
    }
    let delta = &event["assistantMessageEvent"];
    let phase = event
        .get("phase")
        .filter(|v| !v.is_null())
        .unwrap_or(&delta["type"]);
    let index = event
        .get("contentIndex")
        .filter(|v| !v.is_null())
        .unwrap_or(&delta["contentIndex"]);
    let mut compact = json!({"type":"message_update"});
    if let Some(id) = event["messageId"].as_str() {
        compact["messageId"] = json!(bounded_progress(id, 200));
    }
    compact["phase"] = json!(
        phase
            .as_str()
            .map(|p| bounded_progress(p, 80))
            .unwrap_or_else(|| "streaming".into())
    );
    if let Some(index) = index
        .as_f64()
        .filter(|n| n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0)
    {
        compact["contentIndex"] = json!(index as i64);
    }
    compact
}
impl Store {
    pub fn agent_events(&self, run_id: &str) -> Result<Vec<Value>> {
        let mut query = self.db.prepare("SELECT e.attempt_id,e.sequence,e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? ORDER BY e.rowid")?;
        query
            .query_map([run_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .map(|r| {
                let (id, sequence, data) = r?;
                Ok(json!({"attemptId":id,"sequence":sequence,"event":crate::store::parse(data)?}))
            })
            .collect()
    }
    pub fn agent_events_of_type(&self, run_id: &str, types: &[&str]) -> Result<Vec<Value>> {
        let placeholders = vec!["?"; types.len()].join(",");
        let sql = format!(
            "SELECT e.attempt_id,e.sequence,e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? AND json_extract(e.data,'$.type') IN ({placeholders}) ORDER BY e.rowid"
        );
        let mut query = self.db.prepare(&sql)?;
        let values = std::iter::once(run_id).chain(types.iter().copied());
        query
            .query_map(rusqlite::params_from_iter(values), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .map(|r| {
                let (id, sequence, data) = r?;
                Ok(json!({"attemptId":id,"sequence":sequence,"event":crate::store::parse(data)?}))
            })
            .collect()
    }
    pub fn command_events(&self, run_id: &str) -> Result<Vec<Value>> {
        Ok(self
            .agent_events(run_id)?
            .into_iter()
            .filter(|e| {
                [
                    "tool_started",
                    "tool_receipt_reused",
                    "tool_exited",
                    "tool_cancelled",
                ]
                .contains(&e["event"]["type"].as_str().unwrap_or(""))
            })
            .collect())
    }
    pub fn has_agent_event(&self, attempt_id: &str, kind: &str) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM agent_events WHERE attempt_id=? AND json_extract(data,'$.type')=?)",params![attempt_id,kind],|r| r.get(0))?)
    }
    pub fn attempt_for_pane(&self, pane: &str) -> Result<Option<String>> {
        Ok(self.db.query_row("SELECT id FROM attempts WHERE json_extract(data,'$.pane')=? ORDER BY rowid DESC LIMIT 1",[pane],|r|r.get(0)).optional()?)
    }
    pub fn agent_event_page(&self, run_id: &str, after: u64, limit: usize) -> Result<Value> {
        if !(1..=1000).contains(&limit) || after > 9_007_199_254_740_991 {
            return Err(invalid("Invalid event page bounds."));
        }
        let mut query = self.db.prepare("SELECT e.rowid,e.attempt_id,e.sequence,e.data FROM agent_events e JOIN attempts a ON e.attempt_id=a.id JOIN assignments s ON a.assignment_id=s.id WHERE s.run_id=? AND e.rowid>? ORDER BY e.rowid LIMIT ?")?;
        let rows = query
            .query_map(params![run_id, after as i64, (limit + 1) as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = rows.len() > limit;
        let events = rows.into_iter().take(limit).map(|(cursor,id,sequence,data)| Ok(json!({"cursor":cursor,"attemptId":id,"sequence":sequence,"event":crate::store::parse(data)?}))).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"after":events.last().map(|e|e["cursor"].clone()).unwrap_or(json!(after)),"events":events,"hasMore":more}),
        )
    }
    pub fn record_agent_event(&self, attempt_id: &str, sequence: u64, event: &Value) -> Result<()> {
        if sequence == 0 || sequence > 9_007_199_254_740_991 || !event.is_object() {
            return Err(invalid(
                "An event needs a positive safe sequence and an object payload.",
            ));
        }
        let event = compact_observation(event);
        self.transaction(|s| {
            let mut attempt = s.get_attempt(attempt_id)?; let run_id = attempt["runId"].as_str().unwrap().to_owned();
            if s.db.execute("INSERT OR IGNORE INTO agent_events(attempt_id,sequence,data) VALUES(?,?,?)",params![attempt_id,sequence as i64,event.to_string()])? == 0 { return Ok(()); }
            if attempt["ownership"] != "stopped" {
                if attempt["state"] != "failed" && attempt["state"] != "attention_required" {
                    match event["type"].as_str() {
                        Some("agent_start") => attempt["state"] = json!("executing"),
                        Some("session_settled") => attempt["state"] = json!("idle"),
                        Some("process_exit") => attempt["state"] = json!("attention_required"),
                        _ => (),
                    }
                }
                if event["type"] == "model_policy_rejected" {
                    attempt["state"] = json!("failed");
                    let reason = if event["code"] == "input_budget" {"Provider input budget is unavailable or exceeded. Inspect the retained budget observation before continuing."}
                        else if ["codex_contract", "claude_contract"].contains(&event["code"].as_str().unwrap_or("")) {event["reason"].as_str().unwrap_or("Native runner receipt or profile requires reconciliation.")}
                        else {"The provider attempted an unapproved model or reasoning setting."};
                    s.change_run(&run_id,Transition::Attention{reason:reason.into()})?;
                }
                if event["type"] == "prompt_result" && event["status"] == "error" { attempt["state"] = json!("failed"); }
            }
            s.save_attempt(&attempt)?;
            if event["type"] == "agent_start" { s.change_run(&run_id,Transition::AgentStart)?; }
            if ["agent_start","session_settled","tool_started","tool_receipt_reused","tool_exited","process_exit","model_policy_rejected","prompt_result"].contains(&event["type"].as_str().unwrap_or("")) { s.account_time_in_transaction(&run_id)?; }
            s.compact_completed_message(attempt_id, sequence, &event)?;
            Ok(())
        })
    }
    pub(crate) fn compact_completed_message(
        &self,
        attempt_id: &str,
        sequence: u64,
        event: &Value,
    ) -> Result<()> {
        if event["type"] == "message_end" && event["message"].is_object() {
            let message_id = event
                .get("messageId")
                .filter(|v| !v.is_null())
                .unwrap_or(&event["message"]["id"]);
            if let Some(id) = message_id
                .as_str()
                .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 200)
                && !(event.get("messageId").is_some()
                    && event["message"].get("id").is_some()
                    && event["messageId"] != event["message"]["id"])
            {
                self.db.execute("DELETE FROM agent_events WHERE attempt_id=? AND sequence<? AND json_extract(data,'$.type')='message_update' AND json_extract(data,'$.messageId')=?",params![attempt_id,sequence as i64,id])?;
            }
        }
        Ok(())
    }
    pub fn observer_cursor(&self, assignment_id: &str) -> Result<i64> {
        Ok(self
            .db
            .query_row(
                "SELECT cursor FROM observer_cursors WHERE assignment_id=?",
                [assignment_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }
    pub fn record_observer_page(
        &self,
        assignment_id: &str,
        events: &[Value],
        cursor: i64,
    ) -> Result<()> {
        self.transaction(|s| {
            if cursor.unsigned_abs() > 9_007_199_254_740_991 || cursor < s.observer_cursor(assignment_id)? || events.iter().any(|e| e["cursor"].as_i64().is_none_or(|c| c > cursor || c.unsigned_abs()>9_007_199_254_740_991)) { return Err(conflict("Invalid observer cursor.")); }
            for event in events { s.db.execute("INSERT OR IGNORE INTO observations VALUES(?,?,?)",params![assignment_id,event["cursor"].as_i64(),event.to_string()])?; }
            s.db.execute("INSERT INTO observer_cursors VALUES(?,?) ON CONFLICT(assignment_id) DO UPDATE SET cursor=excluded.cursor",params![assignment_id,cursor])?;
            Ok(())
        })
    }
}
