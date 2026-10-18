//! Durable message decisions. Storage, dispatch, receipt and application are distinct.
//! These methods never contact Mail or a host; the controller supplies proven facts.
use crate::{
    Error, Result,
    store::{Store, conflict, id, integer, invalid, remove, strict},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

fn string<'a>(value: &'a Value, key: &str, min: usize, max: usize, trim: bool) -> Result<&'a str> {
    let value = value[key]
        .as_str()
        .ok_or_else(|| invalid(&format!("{key} must be text.")))?;
    let value = if trim {
        crate::store::trim_text(value)
    } else {
        value
    };
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid(&format!("Invalid {key} length.")));
    }
    Ok(value)
}
pub(crate) fn closed_reason(run: &Value) -> Option<&'static str> {
    match run["status"].as_str() {
        Some("closed") => Some("This run is closed."),
        Some("completed") => Some("This research run is completed."),
        Some("cancelled") => Some("This run is cancelled."),
        _ if run["desiredState"] == "cancelled" => {
            Some("Cancellation has been requested for this run.")
        }
        _ => None,
    }
}
pub(crate) fn undelivered_reason(run: &Value) -> Option<&'static str> {
    match run["status"].as_str() {
        Some("completed") => Some(
            "Research completed before delivery. This message will not be sent if the run reopens.",
        ),
        Some("closed") => Some("The run closed before delivery."),
        Some("cancelled") => Some("Cancellation was requested before delivery."),
        _ if run["desiredState"] == "cancelled" => {
            Some("Cancellation was requested before delivery.")
        }
        _ => None,
    }
}
fn transitions(message: &Value) -> Vec<Value> {
    if let Some(saved) = message["transitions"].as_array() {
        return saved.clone();
    }
    let state = message["delivery"].as_str().unwrap_or("pending");
    let terminal = || {
        let mut value = json!({"state":state});
        if let Some(reason) = message.get("dispositionReason") {
            value["reason"] = reason.clone();
        }
        value
    };
    if state == "pending" {
        return vec![];
    }
    if state == "not_delivered" {
        return vec![terminal()];
    }
    let mut values = vec![json!({"state":"dispatched"})];
    if ["received", "applied", "rejected"].contains(&state) {
        values.push(json!({"state":"received"}));
    }
    if ["applied", "rejected"].contains(&state) {
        values.push(terminal());
    }
    values
}
impl Store {
    pub fn steering(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run_records("steering", run_id)
    }
    pub fn delivery_notices(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run_records("delivery_notices", run_id)
    }
    fn save_message(&self, table: &str, message: &Value) -> Result<()> {
        self.db.execute(
            &format!("UPDATE {table} SET data=? WHERE id=?"),
            params![message.to_string(), message["id"].as_str()],
        )?;
        Ok(())
    }
    pub(crate) fn transition_steering(
        &self,
        message: &mut Value,
        state: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let mut history = transitions(message);
        let mut event = json!({"state":state,"at":self.time()});
        if let Some(reason) = reason {
            event["reason"] = json!(reason);
        }
        history.push(event);
        message["transitions"] = json!(history);
        message["delivery"] = json!(state);
        if state == "not_delivered" {
            if let Some(reason) = reason {
                message["dispositionReason"] = json!(reason);
            } else {
                remove(message, "dispositionReason");
            }
        }
        self.save_message("steering", message)
    }
    pub fn steer(&self, run_id: &str, input: &Value) -> Result<Value> {
        let key = string(input, "requestId", 1, 200, false)?;
        let body = string(input, "body", 1, 40000, true)?;
        self.transaction(|s| {
            let previous:Option<String>=s.db.query_row("SELECT data FROM steering WHERE run_id=? AND request_id=?",params![run_id,key],|r|r.get(0)).optional()?;
            if let Some(previous)=previous {let previous=crate::store::parse(previous)?;if previous["body"]!=body{return Err(conflict("This steering request already has different content."));}return Ok(previous);}
            if let Some(reason)=closed_reason(&s.get_run(run_id)?){return Err(conflict(&format!("{reason} Steering cannot schedule more work.")));}
            if s.steering(run_id)?.iter().filter(|m| ["pending","dispatched"].contains(&m["delivery"].as_str().unwrap_or(""))).count()>=100{return Err(conflict("Coordinator backlog reached 100 pending messages. Wait for receipt before sending more; this instruction was not saved."));}
            let message=json!({"id":id(),"runId":run_id,"body":body,"createdAt":s.time(),"delivery":"pending","transitions":[]});
            s.db.execute("INSERT INTO steering VALUES(?,?,?,?)",params![message["id"].as_str(),run_id,key,message.to_string()])?;Ok(message)
        })
    }
    pub fn prepare_message(
        &self,
        attempt_id: &str,
        tool_call_id: &str,
        input: &Value,
    ) -> Result<Value> {
        strict(input, &["to", "body", "replyTo"])?;
        let to = string(input, "to", 0, usize::MAX, false)?;
        let body = string(input, "body", 1, 40000, false)?;
        if input.get("replyTo").is_some() {
            string(input, "replyTo", 0, usize::MAX, false)?;
        }
        string(&json!({"key":tool_call_id}), "key", 1, 200, false)?;
        self.transaction(|s| {
            let attempt=s.get_attempt(attempt_id)?;let run=attempt["runId"].as_str().unwrap();let assignments=s.run_records("assignments",run)?;
            let sender=assignments.iter().find(|a|a["currentAttemptId"]==attempt_id).filter(|a|a.get("retiredAt").is_none()).ok_or_else(||conflict("Sender attempt no longer owns this assignment."))?;
            if ["failed","exited","attention_required"].contains(&attempt["state"].as_str().unwrap_or("")){return Err(conflict("Sender attempt no longer owns this assignment."));}
            let recipient=assignments.iter().find(|a|a["id"]==to || a["name"]==to).filter(|a|a.get("retiredAt").is_none()).ok_or_else(||conflict("Unknown or retired recipient in this run."))?;
            let messages=s.run_records("peer_messages",run)?;
            if let Some(reply)=input["replyTo"].as_str().filter(|v|!v.is_empty()) && !messages.iter().any(|m|m["id"]==reply && m["to"]==sender["id"] && ["accepted","dispatched","received"].contains(&m["disposition"].as_str().unwrap_or(""))){return Err(conflict("Reply does not refer to an accepted message for this sender."));}
            if let Some(prior)=messages.iter().find(|m|m["attemptId"]==attempt_id && m["toolCallId"]==tool_call_id){if prior["body"]!=body || prior["to"]!=recipient["id"] || prior.get("replyTo")!=input.get("replyTo"){return Err(conflict("Message key has different content."));}return Ok(prior.clone());}
            s.assert_evaluation_dispatch(run)?;
            if messages.iter().filter(|m|m["to"]==recipient["id"] && m["disposition"]!="received").count()>=100{return Err(conflict("Recipient backlog reached 100 pending messages. Retained messages must be received before sending more."));}
            let mut message=json!({"id":id(),"runId":run,"attemptId":attempt_id,"toolCallId":tool_call_id,"from":sender["id"],"to":recipient["id"],"body":body,"createdAt":s.time(),"disposition":"pending"});
            if let Some(reply)=input.get("replyTo"){message["replyTo"]=reply.clone();}
            s.db.execute("INSERT INTO peer_messages VALUES(?,?,?,?,?)",params![message["id"].as_str(),run,attempt_id,tool_call_id,message.to_string()])?;Ok(message)
        })
    }
    pub fn bind_mailbox(&self, assignment_id: &str, name: &str) -> Result<()> {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(invalid("Mailbox names must contain ASCII letters only."));
        }
        self.transaction(|s| {
            let mut assignment = s.get_assignment(assignment_id)?;
            if assignment.get("mailboxName").is_some() && assignment["mailboxName"] != name {
                return Err(conflict("Mailbox identity already bound."));
            }
            assignment["mailboxName"] = json!(name);
            s.save_assignment(&assignment)
        })
    }
    pub fn get_peer_message(&self, message_id: &str) -> Result<Value> {
        self.record("peer_messages", "id", message_id)?
            .ok_or_else(|| Error::NotFound("Message not found.".into()))
    }
    pub fn record_message_stored(&self, message_id: &str, transport_id: u64) -> Result<Value> {
        integer(
            &json!({"transportId":transport_id}),
            "transportId",
            1,
            9_007_199_254_740_991,
        )?;
        self.transaction(|s| {
            let mut message = s.get_peer_message(message_id)?;
            if message.get("transportId").is_some() {
                if message["transportId"] != transport_id {
                    return Err(conflict(
                        "Transport identity changed. Reconciliation required.",
                    ));
                }
            } else {
                message["transportId"] = json!(transport_id);
                message["disposition"] = json!("stored");
                s.save_message("peer_messages", &message)?;
            }
            Ok(message)
        })
    }
    pub fn accept_message(&self, message_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut message = s.get_peer_message(message_id)?;
            if message.get("transportId").is_none() || message["disposition"] == "uncertain" {
                return Err(conflict("Message storage is not confirmed."));
            }
            if message["disposition"] == "stored" {
                message["disposition"] = json!("accepted");
                s.save_message("peer_messages", &message)?;
            }
            Ok(message)
        })
    }
    pub fn mark_message_uncertain(&self, message_id: &str) -> Result<()> {
        self.transaction(|s| {
            let mut message = s.get_peer_message(message_id)?;
            message["disposition"] = json!("uncertain");
            s.save_message("peer_messages", &message)
        })
    }
    pub fn record_dispatch(&self, message_id: &str, received: bool) -> Result<()> {
        self.transaction(|s| {
            let mut message = s.get_peer_message(message_id)?;
            if !["accepted", "dispatched", "received"]
                .contains(&message["disposition"].as_str().unwrap_or(""))
            {
                return Err(conflict("Message has not been accepted."));
            }
            if message["disposition"] == "received" {
                return Ok(());
            }
            if !received {
                s.assert_evaluation_dispatch(message["runId"].as_str().unwrap())?;
            }
            let assignment = s.get_assignment(message["to"].as_str().unwrap())?;
            if message.get("recipientAttemptId").is_none() {
                message["recipientAttemptId"] = assignment["currentAttemptId"].clone();
            }
            message["disposition"] = json!(if received { "received" } else { "dispatched" });
            s.save_message("peer_messages", &message)?;
            let mut attempt = s.get_attempt(assignment["currentAttemptId"].as_str().unwrap())?;
            attempt["activeInputMessageId"] = json!(message_id);
            s.save_attempt(&attempt)
        })
    }
    // Serialized key order participates in legacy notice identity; Value equality ignores it.
    #[allow(clippy::cmp_owned)]
    pub fn notice_delivery(&self, run_id: &str, payload: &Value) -> Result<Value> {
        self.transaction(|s|{s.get_run(run_id)?;if let Some(previous)=s.delivery_notices(run_id)?.into_iter().rev().find(|n|n["payload"]["kind"]==payload["kind"] && n["payload"]["subjectId"]==payload["subjectId"]) && crate::serialized::same(&previous["payload"],payload){return Ok(previous);}
            let notice=json!({"id":id(),"runId":run_id,"payload":payload,"createdAt":s.time(),"disposition":"pending"});s.db.execute("INSERT INTO delivery_notices VALUES(?,?,?)",params![notice["id"].as_str(),run_id,notice.to_string()])?;Ok(notice)})
    }
    pub fn dispatch_delivery_notice(
        &self,
        attempt_id: &str,
        notice_id: &str,
        received: bool,
    ) -> Result<Value> {
        self.transaction(|s| {
            let attempt = s.get_attempt(attempt_id)?;
            let run_id = attempt["runId"].as_str().unwrap();
            let run = s.get_run(run_id)?;
            if !s.run_records("assignments", run_id)?.iter().any(|a| {
                a["coordinator"] == true
                    && a.get("retiredAt").is_none()
                    && a["currentAttemptId"] == attempt_id
            }) {
                return Err(conflict(
                    "Delivery notices target only the current coordinator.",
                ));
            }
            let mut notice = s
                .delivery_notices(run_id)?
                .into_iter()
                .find(|n| n["id"] == notice_id)
                .ok_or_else(|| conflict("Delivery notice is not available for this attempt."))?;
            if if received {
                notice["disposition"] != "dispatched" || notice["attemptId"] != attempt_id
            } else {
                notice["disposition"] != "pending"
                    || attempt["ownership"] != "held"
                    || run["status"] != "running"
                    || run["desiredState"] != "running"
            } {
                return Err(conflict(
                    "Delivery notice is not available for this attempt.",
                ));
            }
            notice["disposition"] = json!(if received { "received" } else { "dispatched" });
            notice["attemptId"] = json!(attempt_id);
            s.save_message("delivery_notices", &notice)?;
            Ok(notice)
        })
    }
    pub fn restore_undispatched_input(&self, attempt_id: &str, input_id: &str) -> Result<()> {
        self.transaction(|s| {
            let attempt = s.get_attempt(attempt_id)?;
            if attempt["ownership"] != "stopped" {
                return Err(conflict(
                    "Confirm old ownership stopped before restoring undispatched input.",
                ));
            }
            let run_id = attempt["runId"].as_str().unwrap();
            for mut message in s
                .run_records("peer_messages", run_id)?
                .into_iter()
                .filter(|m| {
                    m["id"] == input_id
                        && m["disposition"] == "dispatched"
                        && m["recipientAttemptId"] == attempt_id
                })
            {
                message["disposition"] = json!("accepted");
                remove(&mut message, "recipientAttemptId");
                s.save_message("peer_messages", &message)?;
            }
            for mut message in s.steering(run_id)?.into_iter().filter(|m| {
                m["inputId"] == input_id
                    && m["delivery"] == "dispatched"
                    && m["attemptId"] == attempt_id
            }) {
                let reason = undelivered_reason(&s.get_run(run_id)?);
                remove(&mut message, "inputId");
                remove(&mut message, "attemptId");
                s.transition_steering(
                    &mut message,
                    if reason.is_some() {
                        "not_delivered"
                    } else {
                        "pending"
                    },
                    Some(
                        reason.unwrap_or("Stopped host has no saved input; waiting for delivery."),
                    ),
                )?;
            }
            if let Some(mut notice) = s.delivery_notices(run_id)?.into_iter().find(|n| {
                n["id"] == input_id
                    && n["disposition"] == "dispatched"
                    && n["attemptId"] == attempt_id
            }) {
                notice["disposition"] = json!("pending");
                remove(&mut notice, "attemptId");
                s.save_message("delivery_notices", &notice)?;
            }
            Ok(())
        })
    }
    pub fn dispatch_steering(
        &self,
        attempt_id: &str,
        ids: &[String],
        input_id: &str,
    ) -> Result<()> {
        self.transaction(|s| {
            let attempt = s.get_attempt(attempt_id)?;
            let run_id = attempt["runId"].as_str().unwrap();
            if !s
                .run_records("assignments", run_id)?
                .iter()
                .any(|a| a["coordinator"] == true && a["currentAttemptId"] == attempt_id)
            {
                return Err(conflict(
                    "Steering is delivered only to the current coordinator.",
                ));
            }
            if let Some(reason) = closed_reason(&s.get_run(run_id)?) {
                return Err(conflict(&format!(
                    "{reason} Steering cannot be dispatched."
                )));
            }
            for id in ids {
                let mut message = s
                    .steering(run_id)?
                    .into_iter()
                    .find(|m| m["id"] == *id && m["delivery"] == "pending")
                    .ok_or_else(|| conflict("Steering is not pending."))?;
                message["attemptId"] = json!(attempt_id);
                message["inputId"] = json!(input_id);
                s.transition_steering(&mut message, "dispatched", None)?;
            }
            Ok(())
        })
    }
    pub fn receive_steering(&self, attempt_id: &str, input_id: &str) -> Result<()> {
        self.transaction(|s| {
            let attempt = s.get_attempt(attempt_id)?;
            for mut message in s
                .steering(attempt["runId"].as_str().unwrap())?
                .into_iter()
                .filter(|m| {
                    m["delivery"] == "dispatched"
                        && m["attemptId"] == attempt_id
                        && m["inputId"] == input_id
                })
            {
                s.transition_steering(&mut message, "received", None)?;
            }
            Ok(())
        })
    }
    pub fn apply_steering(
        &self,
        attempt_id: &str,
        tool_call_id: &str,
        input: &Value,
    ) -> Result<Value> {
        strict(input, &["messageId", "disposition", "reason"])?;
        let message_id = string(input, "messageId", 0, usize::MAX, false)?;
        let disposition = string(input, "disposition", 1, 20, false)?;
        if !["applied", "rejected"].contains(&disposition) {
            return Err(invalid("Invalid steering disposition."));
        }
        let reason = string(input, "reason", 1, 4000, true)?;
        self.transaction(|s| {
            let attempt = s.get_attempt(attempt_id)?;
            let run_id = attempt["runId"].as_str().unwrap();
            if attempt["ownership"] != "held"
                || !s
                    .run_records("assignments", run_id)?
                    .iter()
                    .any(|a| a["coordinator"] == true && a["currentAttemptId"] == attempt_id)
            {
                return Err(conflict(
                    "Only the active coordinator can reconcile steering.",
                ));
            }
            let mut message = s
                .steering(run_id)?
                .into_iter()
                .find(|m| {
                    m["id"] == message_id
                        && !["pending", "not_delivered"]
                            .contains(&m["delivery"].as_str().unwrap_or(""))
                })
                .ok_or_else(|| conflict("Steering has not been delivered to this run."))?;
            if ["applied", "rejected"].contains(&message["delivery"].as_str().unwrap_or("")) {
                if message["delivery"] != disposition || message["dispositionReason"] != reason {
                    return Err(conflict("Steering already has a different disposition."));
                }
                return Ok(message);
            }
            if message["delivery"] == "dispatched" {
                s.transition_steering(&mut message, "received", None)?;
            }
            message["dispositionReason"] = json!(reason);
            message["dispositionToolCallId"] = json!(tool_call_id);
            s.transition_steering(&mut message, disposition, Some(reason))?;
            Ok(message)
        })
    }
}
