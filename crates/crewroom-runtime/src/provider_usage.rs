//! Provider-reported usage and protocol completion, never inferred from model prose.
use super::provider_input::{count, truth};
use encoding_rs::{CoderResult, Decoder, UTF_8};
use serde_json::{Value, json};
fn trim(s: &str) -> &str {
    s.trim_matches(|c:char|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn meaningful_openai(v: &Value) -> bool {
    v["type"] == "function_call" && v["name"].is_string()
        || v["type"] == "output_text" && v["text"].as_str().is_some_and(|s| !trim(s).is_empty())
        || v["type"] == "refusal" && v["refusal"].as_str().is_some_and(|s| !trim(s).is_empty())
        || v["type"] == "message" && array(&v["content"]).iter().any(meaningful_openai)
}
fn meaningful_anthropic(v: &Value) -> bool {
    v["type"] == "tool_use" && v["name"].is_string()
        || v["text"].as_str().is_some_and(|s| !trim(s).is_empty())
}
pub struct UsageReader {
    api: String,
    stream: bool,
    decoder: Decoder,
    pending: String,
    buffered: usize,
    overflow: bool,
    usage: Option<Value>,
    anthropic: serde_json::Map<String, Value>,
    anthropic_done: bool,
    terminal: bool,
    meaningful: bool,
    malformed: bool,
    opaque: Vec<(String, String)>,
}
impl UsageReader {
    pub fn new(api: &str, stream: bool) -> Self {
        Self {
            api: api.into(),
            stream,
            decoder: UTF_8.new_decoder_with_bom_removal(),
            pending: String::new(),
            buffered: 0,
            overflow: false,
            usage: None,
            anthropic: Default::default(),
            anthropic_done: false,
            terminal: false,
            meaningful: false,
            malformed: false,
            opaque: vec![],
        }
    }
    fn decode(&mut self, bytes: &[u8], last: bool) -> String {
        let mut output = String::new();
        let mut offset = 0;
        loop {
            output.reserve((bytes.len() - offset).saturating_mul(3) + 4);
            let (result, read, _) =
                self.decoder
                    .decode_to_string(&bytes[offset..], &mut output, last);
            offset += read;
            if result == CoderResult::InputEmpty {
                break;
            }
        }
        if last {
            self.decoder = UTF_8.new_decoder_with_bom_removal();
        }
        output
    }
    pub fn push(&mut self, bytes: &[u8]) {
        if self.overflow {
            return;
        }
        if !self.stream {
            self.buffered = self.buffered.saturating_add(bytes.len());
            if self.buffered > 8 * 1024 * 1024 {
                self.overflow = true;
                self.pending.clear();
                return;
            }
        }
        let decoded = self.decode(bytes, false);
        self.pending.push_str(&decoded);
        if !self.stream {
            return;
        }
        while let Some(end) = self.pending.find('\n') {
            let line = self.pending[..end].to_owned();
            self.pending.drain(..end + 1);
            if let Some(data) = trim(&line).strip_prefix("data:") {
                self.data(trim(data));
            }
        }
        if self.pending.encode_utf16().count() > 1024 * 1024 {
            self.pending.clear();
        }
    }
    /// Drain after each chunk. Callers can retain hashes rather than opaque content.
    pub fn take_opaque(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.opaque)
    }
    /// Semantic completion can precede transport EOF. This proves only the
    /// budget for observed opaque output; billing still waits for the transport.
    pub fn completed_output_tokens(&self) -> Option<f64> {
        if self.terminal && !self.malformed && !self.overflow {
            self.usage
                .as_ref()
                .and_then(|usage| count(&usage["output"]))
        } else {
            None
        }
    }
    pub fn finish(&mut self, status: u16, interrupted: bool) -> Value {
        if !self.stream && !self.overflow {
            let suffix = self.decode(&[], true);
            let text = format!("{}{suffix}", self.pending);
            self.data(&text);
        }
        let unavailable = |reason: String| json!({"type":"provider_usage","status":status,"state":"unavailable","reason":reason});
        if interrupted {
            return unavailable(
                "The provider response was interrupted before it completed.".into(),
            );
        }
        if !(200..300).contains(&status) {
            return unavailable(format!(
                "The provider returned HTTP {status} without a usage report."
            ));
        }
        self.usage.as_ref().map(|usage|json!({"type":"provider_usage","status":status,"state":"actual","usage":usage})).unwrap_or_else(||unavailable("The provider response did not include a usage report.".into()))
    }
    pub fn outcome(&self) -> Option<&'static str> {
        if self.malformed || !self.terminal {
            Some("malformed")
        } else if !self.meaningful {
            Some("empty_final")
        } else {
            None
        }
    }
    fn data(&mut self, text: &str) {
        if text.is_empty() || text == "[DONE]" {
            return;
        }
        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => {
                self.malformed = true;
                return;
            }
        };
        if let Value::Array(a) = value {
            for v in a {
                self.accept(&v);
            }
        } else {
            self.accept(&value);
        }
    }
    fn collect(&mut self, kind: &str, values: impl Iterator<Item = Value>, ty: &str, key: &str) {
        for v in values {
            if v["type"] == ty
                && let Some(s) = v[key].as_str()
            {
                self.opaque.push((kind.into(), s.into()));
            }
        }
    }
    fn accept(&mut self, event: &Value) {
        match self.api.as_str() {
            "openai-responses" => {
                let response = &event["response"];
                if event["type"] == "response.completed"
                    || !self.stream
                        && event["object"] == "response"
                        && event["status"] == "completed"
                {
                    self.terminal = true;
                }
                if array(&response["output"]).iter().any(meaningful_openai)
                    || array(&event["output"]).iter().any(meaningful_openai)
                    || meaningful_openai(&event["item"])
                    || ["response.output_text.delta", "response.refusal.delta"]
                        .contains(&event["type"].as_str().unwrap_or(""))
                        && event["delta"].as_str().is_some_and(|s| !trim(s).is_empty())
                {
                    self.meaningful = true;
                }
                self.collect(
                    "openai-reasoning",
                    std::iter::once(event["item"].clone())
                        .chain(array(&response["output"]).iter().cloned())
                        .chain(array(&event["output"]).iter().cloned()),
                    "reasoning",
                    "encrypted_content",
                );
                let terminal = [
                    "response.completed",
                    "response.incomplete",
                    "response.failed",
                ]
                .contains(&event["type"].as_str().unwrap_or(""));
                let usage = if terminal {
                    &response["usage"]
                } else if !self.stream
                    && event["object"] == "response"
                    && ["completed", "incomplete", "failed"]
                        .contains(&event["status"].as_str().unwrap_or(""))
                {
                    &event["usage"]
                } else {
                    &Value::Null
                };
                let (Some(input), Some(output)) = (
                    count(&usage["input_tokens"]),
                    count(&usage["output_tokens"]),
                ) else {
                    return;
                };
                self.usage = Some(
                    json!({"input":input,"output":output,"cacheRead":count(&usage["input_tokens_details"]["cached_tokens"]).unwrap_or(0.0),"cacheWrite":0,"reasoning":count(&usage["output_tokens_details"]["reasoning_tokens"]).unwrap_or(0.0),"total":count(&usage["total_tokens"]).unwrap_or(input+output)}),
                );
            }
            "anthropic-messages" => {
                if event["type"] == "message_stop"
                    || !self.stream && event["type"] == "message" && truth(&event["stop_reason"])
                {
                    self.terminal = true;
                }
                if meaningful_anthropic(&event["content_block"])
                    || meaningful_anthropic(&event["delta"])
                    || array(&event["content"]).iter().any(meaningful_anthropic)
                {
                    self.meaningful = true;
                }
                self.collect(
                    "anthropic-thinking",
                    std::iter::once(event["content_block"].clone())
                        .chain(array(&event["content"]).iter().cloned()),
                    "redacted_thinking",
                    "data",
                );
                let usage = if event["type"] == "message_start" {
                    &event["message"]["usage"]
                } else if event["type"] == "message_delta"
                    || !self.stream && event["type"] == "message"
                {
                    &event["usage"]
                } else {
                    return;
                };
                if let Some(o) = usage.as_object() {
                    for (k, v) in o {
                        if count(v).is_some() {
                            self.anthropic.insert(k.clone(), v.clone());
                        }
                    }
                }
                if event["type"] != "message_start" {
                    self.anthropic_done = true;
                }
                let a = &self.anthropic;
                let (Some(uncached), Some(output)) = (
                    count(a.get("input_tokens").unwrap_or(&Value::Null)),
                    count(a.get("output_tokens").unwrap_or(&Value::Null)),
                ) else {
                    return;
                };
                if !self.anthropic_done {
                    return;
                }
                let cache_read =
                    count(a.get("cache_read_input_tokens").unwrap_or(&Value::Null)).unwrap_or(0.0);
                let cache_write =
                    count(a.get("cache_creation_input_tokens").unwrap_or(&Value::Null))
                        .unwrap_or(0.0);
                let input = uncached + cache_read + cache_write;
                self.usage = Some(
                    json!({"input":input,"output":output,"cacheRead":cache_read,"cacheWrite":cache_write,"reasoning":0,"total":input+output}),
                );
            }
            "google-generative-ai" => {
                for candidate in array(&event["candidates"]) {
                    if candidate["finishReason"].is_string() {
                        self.terminal = true;
                    }
                    let parts = array(&candidate["content"]["parts"]);
                    if parts.iter().any(|p| {
                        p["thought"] != true
                            && p["text"].as_str().is_some_and(|s| !trim(s).is_empty())
                            || truth(&p["functionCall"])
                    }) {
                        self.meaningful = true;
                    }
                    for part in parts {
                        if let Some(s) = part["thoughtSignature"].as_str() {
                            self.opaque.push(("google-thinking".into(), s.into()));
                        }
                    }
                }
                let usage = &event["usageMetadata"];
                let (Some(input), Some(candidates)) = (
                    count(&usage["promptTokenCount"]),
                    count(&usage["candidatesTokenCount"]),
                ) else {
                    return;
                };
                if !array(&event["candidates"])
                    .iter()
                    .any(|v| v["finishReason"].is_string())
                {
                    return;
                }
                let reasoning = count(&usage["thoughtsTokenCount"]).unwrap_or(0.0);
                self.usage = Some(
                    json!({"input":input,"output":candidates+reasoning,"cacheRead":count(&usage["cachedContentTokenCount"]).unwrap_or(0.0),"cacheWrite":0,"reasoning":reasoning,"total":count(&usage["totalTokenCount"]).unwrap_or(input+candidates+reasoning)}),
                );
            }
            _ => (),
        }
    }
}
