//! Progress coalescing never suppresses completed messages, tool calls or receipts.
use serde_json::Value;
#[derive(Default)]
pub struct ObservationStream {
    message_id: Option<Value>,
    at: Option<i64>,
}
fn same_id(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(a), Some(b)) if !a.is_object() && !a.is_array() => a == b,
        _ => false,
    }
}
impl ObservationStream {
    pub fn accept(&mut self, event: &Value, now: i64) -> Option<Value> {
        if event["type"] != "message_update" {
            return Some(event.clone());
        }
        if same_id(self.message_id.as_ref(), event.get("messageId"))
            && self
                .at
                .is_some_and(|at| i128::from(now) - i128::from(at) < 250)
        {
            return None;
        }
        self.message_id = event.get("messageId").cloned();
        self.at = Some(now);
        Some(crewroom_core::compact_observation(event))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn bursts_coalesce_but_completed_frames_and_new_messages_remain() {
        let mut stream = ObservationStream::default();
        let mut count = 0;
        for now in 0..12000 {
            if stream.accept(&json!({"type":"message_update","messageId":"large","message":{"content":"repeated"},"assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0.0}}),now).is_some(){count+=1;}
        }
        assert_eq!(count, 48);
        let completed = json!({"type":"host_tool_call","arguments":{"content":"complete"}});
        assert_eq!(stream.accept(&completed, 12000), Some(completed));
        assert!(
            stream
                .accept(&json!({"type":"message_update","messageId":"new"}), 12000)
                .is_some()
        );
    }
    #[test]
    fn identity_and_safe_integer_spelling_follow_json_wire_semantics() {
        let mut stream = ObservationStream::default();
        for now in [0, 1] {
            assert!(
                stream
                    .accept(&json!({"type":"message_update","messageId":{}}), now)
                    .is_some()
            );
        }
        let event: Value =
            serde_json::from_str(r#"{"type":"message_update","contentIndex":-0.0}"#).unwrap();
        assert_eq!(stream.accept(&event, 2).unwrap()["contentIndex"], 0);
        assert!(
            stream
                .accept(&json!({"type":"message_update","messageId":null}), 3)
                .is_some()
        );
        assert!(
            stream
                .accept(&json!({"type":"message_update","messageId":null}), 4)
                .is_none()
        );
    }
}
