//! Readable observation only. No workflow or evidence consumer parses pane output.
use crewroom_core::serialized::stringify;
use serde_json::Value;
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        _ => true,
    }
}
fn js(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js(Some(v))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(v) => stringify(v),
    }
}
fn or(v: Option<&Value>, default: &str) -> String {
    js(v.filter(|v| !v.is_null())
        .or(Some(&Value::String(default.into()))))
}
// As elsewhere in the native port, truncate only at complete Unicode scalars.
fn truncate(s: &str, limit: usize) -> String {
    let mut n = 0;
    s.chars()
        .take_while(|c| {
            n += c.len_utf16();
            n <= limit
        })
        .collect()
}
pub fn summarize_arguments(value: &Value, limit: usize) -> String {
    if !value.is_object() && !value.is_array() {
        return String::new();
    }
    let mut parts: Vec<_> = [
        "path",
        "checkId",
        "commandId",
        "to",
        "name",
        "messageId",
        "disposition",
    ]
    .iter()
    .filter_map(|k| {
        value
            .get(k)
            .filter(|v| v.is_string() || v.is_number())
            .map(|v| format!("{k}={}", js(Some(v))))
    })
    .collect();
    if let Some(s) = value["content"].as_str() {
        parts.push(format!("{} lines", s.split('\n').count()));
    }
    if let Some(a) = value["specialists"].as_array() {
        parts.push(format!(
            "+{}",
            a.iter()
                .map(|v| or(v.get("name"), "?"))
                .collect::<Vec<_>>()
                .join(", +")
        ));
    }
    if parts.is_empty() {
        // JSON.stringify's property ordering also covers integer-like object keys.
        let ordered: Value = serde_json::from_str(&stringify(value)).unwrap();
        let keys: Vec<String> = match &ordered {
            Value::Object(o) => o.keys().take(4).cloned().collect(),
            Value::Array(a) => (0..a.len().min(4)).map(|i| i.to_string()).collect(),
            _ => vec![],
        };
        parts.push(keys.join(" "));
    }
    let text = parts.join(" ");
    if text.encode_utf16().count() > limit {
        format!("{}…", truncate(&text, limit.saturating_sub(1)))
    } else {
        text
    }
}
#[derive(Default)]
pub struct PaneFormatter {
    streaming: Option<&'static str>,
}
impl PaneFormatter {
    fn end(&mut self, out: &mut String) {
        if self.streaming.take().is_some() {
            out.push_str(RESET);
            out.push('\n');
        }
    }
    pub fn line(&mut self, stamp: &str, color: &str, text: &str) -> String {
        let mut out = String::new();
        self.end(&mut out);
        out.push_str(&format!("{DIM}{stamp}{RESET}  {color}{text}{RESET}\n"));
        out
    }
    pub fn output(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        self.end(&mut out);
        let mut body = String::new();
        let mut consumed = 0;
        for (i, part) in chunk.split_inclusive('\n').enumerate() {
            consumed += part.len();
            if i > 0 {
                body.push_str(&format!("{DIM}│{RESET} "));
            }
            // Preserve final CRLF; normalize interior CRLF as the retained regex does.
            if part.ends_with("\r\n") && consumed < chunk.len() {
                body.push_str(&part[..part.len() - 2]);
                body.push('\n');
            } else {
                body.push_str(part);
            }
        }
        out.push_str(&format!(
            "{DIM}│{RESET} {body}{}",
            if body.ends_with('\n') { "" } else { "\n" }
        ));
        out
    }
    pub fn frame(&mut self, f: &Value, stamp: &str) -> String {
        let mut out = String::new();
        let (color, text) = match f["type"].as_str().unwrap_or("") {
            "host_ready" => (
                "32",
                format!(
                    "✓ host ready · host {} · {} {}{}",
                    js(f.get("hostPid")),
                    f["runnerKind"].as_str().unwrap_or("omp"),
                    js(f.get("runnerPid").or_else(|| f.get("ompPid"))),
                    if truth(&f["model"]) {
                        format!(" · {}", js(f.get("model")))
                    } else {
                        String::new()
                    }
                ),
            ),
            "session_restore_inspected" => (
                "2",
                format!(
                    "session restore inspected · {}",
                    if truth(&f["restored"]["resumed"]) {
                        "resumed"
                    } else {
                        "new session"
                    }
                ),
            ),
            "agent_start" => ("2", "responding…".into()),
            "message_end" => {
                self.end(&mut out);
                return out;
            }
            "message_update" => {
                let e = &f["assistantMessageEvent"];
                match e["type"].as_str() {
                    Some("text_delta") if e["delta"].is_string() => {
                        if self.streaming != Some("text") {
                            self.end(&mut out);
                            out.push_str(&format!("\x1b[1m›{RESET} "));
                            self.streaming = Some("text");
                        }
                        out.push_str(&e["delta"].as_str().unwrap().replace('\n', "\n  "));
                    }
                    Some("thinking_start") => {
                        self.end(&mut out);
                        out.push_str(&format!("{DIM}\x1b[3mthinking…"));
                        self.streaming = Some("thinking");
                    }
                    Some("toolcall_end") if truth(&e["toolCall"]) => {
                        return self.line(
                            stamp,
                            "\x1b[35m",
                            &format!(
                                "⋯ calling {} {}",
                                js(e["toolCall"].get("name")),
                                summarize_arguments(&e["toolCall"]["arguments"], 90)
                            ),
                        );
                    }
                    _ => (),
                };
                return out;
            }
            "host_tool_call" => (
                "36",
                format!(
                    "→ {} {}",
                    js(f.get("toolName")),
                    summarize_arguments(&f["arguments"], 90)
                ),
            ),
            "tool_execution_start" => (
                "36",
                format!(
                    "→ {} {}",
                    or(f.get("toolName"), "tool"),
                    summarize_arguments(&f["args"], 90)
                ),
            ),
            "tool_execution_end" => (
                if truth(&f["isError"]) { "31" } else { "2" },
                format!(
                    "{} {} finished",
                    if truth(&f["isError"]) { "✗" } else { "✓" },
                    or(f.get("toolName"), "tool")
                ),
            ),
            "tool_started" => (
                "34",
                format!(
                    "▶ {} started in container",
                    or(f.get("commandId"), "command")
                ),
            ),
            "tool_receipt_reused" => (
                "34",
                format!(
                    "▶ {} reattached to retained container",
                    or(f.get("commandId"), "command")
                ),
            ),
            "tool_exited" => (
                if f["result"]["exitCode"].as_f64() == Some(0.0) {
                    "32"
                } else {
                    "33"
                },
                format!(
                    "■ command exited {}",
                    or(f["result"].get("exitCode"), "with unknown status")
                ),
            ),
            "tool_cancelled" => ("33", "■ command cancelled".into()),
            "prompt_result" => {
                if f["status"] == "error" {
                    (
                        "31",
                        format!(
                            "✗ turn failed: {}",
                            or(f["error"].get("message"), "no detail")
                        ),
                    )
                } else {
                    ("2", format!("turn {}", js(f.get("status"))))
                }
            }
            "auto_retry_start" => (
                "33",
                format!(
                    "retrying provider request{}",
                    if truth(&f["attempt"]) {
                        format!(" (attempt {})", js(f.get("attempt")))
                    } else {
                        String::new()
                    }
                ),
            ),
            "auto_compaction_start" => ("2", "compacting context…".into()),
            "model_policy_rejected" => (
                "31",
                "✗ blocked an unapproved model or reasoning setting".into(),
            ),
            "ownership_unresolved" => (
                "31",
                format!("✗ ownership unresolved: {}", or(f.get("reason"), "")),
            ),
            "process_exit" => (
                "33",
                format!(
                    "■ runner exited{}{}",
                    f.get("code")
                        .filter(|v| !v.is_null())
                        .map(|v| format!(" (code {})", js(Some(v))))
                        .unwrap_or_default(),
                    if truth(&f["signal"]) {
                        format!(" ({})", js(f.get("signal")))
                    } else {
                        String::new()
                    }
                ),
            ),
            _ => return out,
        };
        self.line(stamp, &format!("\x1b[{color}m"), &text)
    }
}
