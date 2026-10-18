use super::*;
gpui_kit::actions!(crewroom_conversation, [SendInstruction]);
pub(crate) use SendInstruction as Send;

fn timestamp(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis())
        })
        .unwrap_or(0)
}
fn recorded(value: &Value) -> String {
    let at = timestamp(value);
    if at <= 0 {
        return "Time unknown".into();
    }
    chrono::DateTime::from_timestamp_millis(at)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Time unknown".into())
}
/// Shared by steering, peer messages and Work packages so one disposition reads the same everywhere.
pub(super) fn delivery(state: &str) -> &'static str {
    match state {
        "stored" | "pending" => "Stored · Receipt unconfirmed",
        "accepted" => "Accepted · Not yet dispatched",
        "uncertain" => "Delivery uncertain · Reconciliation needed",
        "dispatched" => "Dispatched · Receipt unconfirmed",
        "received" => "Receipt confirmed",
        "applied" => "Applied to plan",
        "rejected" => "Received · Coordinator declined",
        "not_delivered" => "Not delivered",
        _ => "Delivery state unavailable",
    }
}
fn transitions(message: &Value) -> Vec<Value> {
    if message["transitions"].is_array() {
        return records(message, "transitions");
    }
    let state = message["delivery"].as_str().unwrap_or("pending");
    if state == "pending" {
        return vec![];
    }
    if state == "not_delivered" {
        return vec![json!({"state":state,"reason":message["dispositionReason"]})];
    }
    let mut values = vec![json!({"state":"dispatched"})];
    if ["received", "applied", "rejected"].contains(&state) {
        values.push(json!({"state":"received"}));
    }
    if ["applied", "rejected"].contains(&state) {
        values.push(json!({"state":state,"reason":message["dispositionReason"]}));
    }
    values
}
fn reply(event: &Value, attempts: &HashSet<String>) -> Option<(i64, String)> {
    let value = &event["event"];
    let message = &value["message"];
    if value["type"] != "message_end"
        || message["role"] != "assistant"
        || !attempts.contains(&string(event, "attemptId"))
    {
        return None;
    }
    let body = records(message, "content")
        .iter()
        .filter(|part| part["type"] == "text")
        .map(|part| string(part, "text"))
        .collect::<Vec<_>>()
        .join("\n");
    if body.is_empty() {
        None
    } else {
        Some((timestamp(&message["timestamp"]), body))
    }
}
fn message_card(message: &Value) -> Div {
    let mut card = section(if message["kind"] == "requirements_adoption" {
        "Requirement decision"
    } else {
        "You"
    })
    .child(crate::accessibility::text(string(message, "body")))
    .child(hint(delivery(message["delivery"].as_str().unwrap_or(""))))
    .child(hint(format!("Stored: {}", recorded(&message["createdAt"]))));
    if message["delivery"] == "applied" {
        card=card.child(hint("Reconciled into the plan or an explicit response. This does not confirm completed work."));
    }
    if let Some(reason) = message["dispositionReason"].as_str() {
        card = card.child(hint(reason.to_owned()));
    }
    for (index, transition) in transitions(message).iter().enumerate() {
        let label = if transition["state"] == "pending" {
            "Delivery deferred"
        } else {
            delivery(transition["state"].as_str().unwrap_or(""))
        };
        card = card.child(
            hint(format!("{label}: {}", recorded(&transition["at"])))
                .id(SharedString::from(format!("transition-{index}"))),
        );
        if let Some(reason) = transition["reason"].as_str() {
            card = card.child(
                hint(reason.to_owned())
                    .id(SharedString::from(format!("transition-reason-{index}"))),
            );
        }
    }
    div().child(card.id(SharedString::from(format!(
        "operator-message-{}",
        string(message, "id")
    ))))
}
impl Workbench {
    fn send_instruction(&mut self, run: &str, cx: &mut Context<Self>) {
        let Some(saved) = records(&self.workspace, "runs")
            .into_iter()
            .find(|r| r["id"] == run)
        else {
            return;
        };
        if ended(&saved) || self.blocked() {
            return;
        }
        let Some(input) = self.steering.get(run) else {
            return;
        };
        let body = input.read(cx).value().to_string();
        if body.trim().is_empty() {
            return;
        }
        self.send(
            format!("/runs/{run}/steering"),
            json!({"body":body}),
            true,
            cx,
        );
    }
    fn load_reply_page(&mut self, after: u64, cx: &mut Context<Self>) {
        let Some(id) = &self.observation.id else {
            return;
        };
        if !self.connected || self.observation.archive_path.is_some() {
            return;
        }
        let path = format!("/runs/{id}/events?after={after}&limit=200");
        match self
            .client
            .as_ref()
            .ok_or("Connection closed.")
            .and_then(|c| c.read(path.clone()).map_err(|_| "Connection closed."))
        {
            Ok(()) => {
                self.observation.archive_path = Some(path);
                self.observation.archive_pending_start = Some(after);
                self.observation.archive_error.clear();
            }
            Err(error) => self.observation.archive_error = error.into(),
        }
        cx.notify();
    }
    pub(in crate::workbench) fn conversation(
        &mut self,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let intake_panel =
            if run["intakeId"].is_string() && run["purpose"] == "grill" && self.tab != "Brief" {
                records(&self.workspace, "tasks")
                    .into_iter()
                    .find(|t| t["id"] == run["taskId"])
                    .map(|t| self.intake_controls(&t, window, cx))
            } else {
                None
            };
        let id = string(run, "id");
        let runtime = &self.observation.runtime;
        let assignments = records(runtime, "assignments");
        let coordinator = assignments.iter().find(|a| a["coordinator"] == true);
        let attempts: HashSet<_> = records(runtime, "attempts")
            .iter()
            .filter(|a| coordinator.is_some_and(|c| a["assignmentId"] == c["id"]))
            .map(|a| string(a, "id"))
            .collect();
        let mut conversation = section("Coordinator conversation").key_context("CrewroomSteering");
        if let Some(panel) = intake_panel {
            conversation = conversation.child(panel);
        }
        if let Some(error) = self.observation.errors.get("events") {
            conversation = conversation.child(hint(error.clone()));
        }
        if !self.observation.runtime_fresh {
            conversation=conversation.child(hint("Run observation is not current. Retained replies and delivery records may be stale."));
        }
        if self.observation.truncated {
            conversation = conversation.child(hint(
                "Showing retained recent events. Browse stored replies below for earlier pages.",
            ));
        }
        let mut entries: Vec<(i64, Div)> = records(&self.workspace, "messages")
            .iter()
            .filter(|m| m["runId"] == id)
            .map(|m| (timestamp(&m["createdAt"]), message_card(m)))
            .collect();
        let mut replied = false;
        for event in &self.observation.events {
            if let Some((time, body)) = reply(event, &attempts) {
                replied = true;
                entries.push((
                    time,
                    div().child(
                        section("Coordinator")
                            .id(SharedString::from(format!(
                                "coordinator-event-{}",
                                event["cursor"]
                            )))
                            .child(crate::accessibility::text(body))
                            .child(hint(recorded(&json!(time)))),
                    ),
                ));
            }
        }
        entries.sort_by_key(|(time, _)| *time);
        if replied {
            conversation = conversation.child(hint(
                "A reply does not mean your steering was applied; check each message's status.",
            ));
        }
        if entries.is_empty() {
            conversation = conversation.child(hint(
                "The coordinator’s replies and your steering will appear here.",
            ));
        }
        for (_, entry) in entries {
            conversation = conversation.child(entry);
        }
        let closed = ended(run);
        let input = self
            .steering
            .entry(id.clone())
            .or_insert_with(|| {
                cx.new(|cx| {
                    TextareaState::new(window, cx).placeholder("Send direction to the coordinator…")
                })
            })
            .clone();
        let empty = input.read(cx).value().trim().is_empty();
        let send_id = id.clone();
        let keyboard_id = id.clone();
        conversation = conversation
            .child(hint(if closed {
                "This run has ended. New steering is unavailable."
            } else if !self.connected {
                "Connection unavailable. Reconnect to store steering."
            } else if run["desiredState"] == "paused"
                || ["paused", "pausing"].contains(&run["status"].as_str().unwrap_or(""))
            {
                "Saved now. Delivery waits for Resume."
            } else {
                "Stored first. Receipt and application show on each message."
            }))
            .child(
                Textarea::new(&input)
                    .accessibility_id("steering-input")
                    .aria_label("Coordinator instruction")
                    .h(px(140.))
                    .disabled(self.busy || closed),
            )
            .child(hint("Ctrl/⌘ + Enter to send"))
            .children(self.feedback(crate::workbench::feedback::Place::Composer))
            .child(
                Button::new("save-steering")
                    .primary()
                    .label("Send to coordinator")
                    .disabled(self.blocked() || closed || empty)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.send_instruction(&send_id, cx)),
                    ),
            )
            .on_action(cx.listener(move |this, _: &SendInstruction, _, cx| {
                this.send_instruction(&keyboard_id, cx)
            }));
        let mut peers = section("Peer messages").child(hint(
            "Mailbox receipt confirms delivery, not completed work.",
        ));
        for message in records(runtime, "messages") {
            let name = |id: &Value| {
                assignments
                    .iter()
                    .find(|a| a["id"] == *id)
                    .map(|a| string(a, "name"))
                    .unwrap_or_else(|| "Unknown assignment".into())
            };
            let row = div()
                .id(SharedString::from(format!(
                    "peer-message-{}",
                    string(&message, "id")
                )))
                .child(hint(format!(
                    "{} → {} · {}",
                    name(&message["from"]),
                    name(&message["to"]),
                    delivery(message["disposition"].as_str().unwrap_or(""))
                )))
                .child(crate::accessibility::text(string(&message, "body")));
            peers = peers.child(row);
        }
        for notice in records(runtime, "deliveryNotices") {
            if notice["payload"]["kind"] == "operator_specialist_proposed" {
                let row = div()
                    .id(SharedString::from(format!(
                        "delivery-notice-{}",
                        string(&notice, "id")
                    )))
                    .child(hint(format!(
                        "Specialist insertion · {} · {}",
                        delivery(notice["disposition"].as_str().unwrap_or("pending")),
                        recorded(&notice["createdAt"])
                    )))
                    .child(hint(
                        "Proposal delivery is separate from assignment admission.",
                    ));
                peers = peers.child(row);
            }
        }
        let mut archive = section("Stored coordinator replies").child(hint(
            "Browse durable event pages independently of the live conversation.",
        ));
        let waiting = self.observation.archive_path.is_some() || !self.connected;
        archive = archive
            .child(
                Button::new("browse-replies")
                    .label("Read from the beginning")
                    .disabled(waiting)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.load_reply_page(0, cx);
                    })),
            )
            .child(hint(self.observation.archive_error.clone()));
        if let Some(page) = &self.observation.archive {
            let replies: Vec<_> = records(page, "events")
                .iter()
                .filter_map(|e| reply(e, &attempts))
                .collect();
            if replies.is_empty() {
                archive = archive.child(hint("No coordinator replies on this event page."));
            }
            for (index, (time, body)) in replies.into_iter().enumerate() {
                archive = archive.child(
                    div()
                        .id(SharedString::from(format!("archived-reply-{index}")))
                        .child(hint(recorded(&json!(time))))
                        .child(crate::accessibility::text(body)),
                );
            }
            let next = page["after"].as_u64().unwrap_or(0);
            let has_more = page["hasMore"] == true;
            let previous = self.observation.archive_starts.len() > 1;
            archive = archive.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("previous-replies")
                            .label("Previous page")
                            .disabled(waiting || !previous)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let starts = &this.observation.archive_starts;
                                let after = starts
                                    .get(starts.len().saturating_sub(2))
                                    .copied()
                                    .unwrap_or(0);
                                this.load_reply_page(after, cx);
                            })),
                    )
                    .child(
                        Button::new("next-replies")
                            .label("Next page")
                            .disabled(waiting || !has_more)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_reply_page(next, cx);
                            })),
                    ),
            );
        }
        conversation
            .child(self.package_panel(run, cx))
            .child(peers)
            .child(archive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversation_never_promotes_receipt_or_specialist_output_to_coordinator_completion() {
        assert_eq!(delivery("stored"), "Stored · Receipt unconfirmed");
        assert_eq!(delivery("received"), "Receipt confirmed");
        assert_eq!(delivery("applied"), "Applied to plan");
        let attempts = HashSet::from(["current".into(), "previous".into()]);
        let mut event = json!({"attemptId":"previous","event":{"type":"message_end","message":{"role":"assistant","timestamp":1700000000000i64,"content":[{"type":"thinking","thinking":"Hidden"},{"type":"text","text":"Recorded reply"}]}}});
        assert_eq!(
            reply(&event, &attempts),
            Some((1700000000000, "Recorded reply".into()))
        );
        event["attemptId"] = json!("specialist");
        assert!(reply(&event, &attempts).is_none());
        event["attemptId"] = json!("current");
        event["event"]["message"]["role"] = json!("user");
        assert!(reply(&event, &attempts).is_none());
        assert_eq!(recorded(&Value::Null), "Time unknown");
    }
}
