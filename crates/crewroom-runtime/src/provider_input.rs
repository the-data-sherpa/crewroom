//! Conservative protocol-position input checks and durable opaque-reasoning bounds.
use crate::{conflict, durable_new, read};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};
pub(crate) fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
pub(crate) fn count(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= 9_007_199_254_740_991.0)
}
fn list(v: &Value, mut check: impl FnMut(&Value) -> bool) -> bool {
    v.as_array()
        .is_some_and(|a| a.iter().all(|v| v.is_object() && check(v)))
}
fn text(v: &Value, check: impl FnMut(&Value) -> bool) -> bool {
    v.is_string() || list(v, check)
}
fn openai_block(v: &Value) -> bool {
    ([
        "input_text",
        "output_text",
        "summary_text",
        "reasoning_text",
    ]
    .contains(&v["type"].as_str().unwrap_or(""))
        && v["text"].is_string())
        || v["type"] == "refusal" && v["refusal"].is_string()
}
fn anthropic_block(v: &Value, opaque: &mut dyn FnMut(&str, &str) -> bool) -> bool {
    match v["type"].as_str().unwrap_or("") {
        "text" => v["text"].is_string(),
        "thinking" => v["thinking"].is_string(),
        "redacted_thinking" => v["data"]
            .as_str()
            .is_some_and(|s| opaque("anthropic-thinking", s)),
        "tool_use" => v["input"].is_object(),
        "tool_result" => text(&v["content"], |v| anthropic_block(v, opaque)),
        _ => false,
    }
}
fn google_part(v: &Value, opaque: &mut dyn FnMut(&str, &str) -> bool) -> bool {
    if v.as_object().is_none_or(|o| {
        o.keys().any(|k| {
            ![
                "text",
                "thought",
                "thoughtSignature",
                "functionCall",
                "functionResponse",
            ]
            .contains(&k.as_str())
        })
    }) {
        return false;
    }
    if let Some(signature) = v.get("thoughtSignature")
        && !signature.as_str().is_some_and(|s| {
            s == "skip_thought_signature_validator" || opaque("google-thinking", s)
        })
    {
        return false;
    }
    if let Some(response) = v.get("functionResponse") {
        return response.is_object()
            && response["response"].is_object()
            && (response.get("parts").is_none()
                || list(&response["parts"], |v| google_part(v, opaque)));
    }
    v["text"].is_string() || v["functionCall"].is_object()
}
pub fn has_bounded_text_input(
    api: &str,
    body: &Value,
    opaque: &mut dyn FnMut(&str, &str) -> bool,
) -> bool {
    match api {
        "openai-responses" => {
            !truth(&body["previous_response_id"])
                && !truth(&body["conversation"])
                && !truth(&body["prompt"])
                && (body.get("tools").is_none()
                    || list(&body["tools"], |v| v["type"] == "function"))
                && text(&body["input"], |v| {
                    if v.get("type").is_none() || v["type"] == "message" {
                        return text(&v["content"], openai_block);
                    }
                    match v["type"].as_str().unwrap_or("") {
                        "function_call" => v["arguments"].is_string(),
                        "function_call_output" => text(&v["output"], openai_block),
                        "reasoning" => {
                            (v["encrypted_content"].is_null()
                                || v["encrypted_content"]
                                    .as_str()
                                    .is_some_and(|s| opaque("openai-reasoning", s)))
                                && (v.get("summary").is_none() || list(&v["summary"], openai_block))
                                && (v.get("content").is_none() || list(&v["content"], openai_block))
                        }
                        _ => false,
                    }
                })
        }
        "anthropic-messages" => {
            (body.get("system").is_none()
                || text(&body["system"], |v| {
                    v["type"] == "text" && v["text"].is_string()
                }))
                && (body.get("tools").is_none()
                    || list(&body["tools"], |v| {
                        v.get("type").is_none() || v["type"] == "custom"
                    }))
                && list(&body["messages"], |v| {
                    text(&v["content"], |v| anthropic_block(v, opaque))
                })
        }
        "google-generative-ai" => {
            !truth(&body["cachedContent"])
                && (body.get("tools").is_none()
                    || list(&body["tools"], |v| {
                        v.as_object()
                            .unwrap()
                            .keys()
                            .all(|k| k == "functionDeclarations")
                    }))
                && (body.get("systemInstruction").is_none()
                    || body["systemInstruction"].is_object()
                        && list(&body["systemInstruction"]["parts"], |v| {
                            google_part(v, opaque)
                        }))
                && list(&body["contents"], |v| {
                    list(&v["parts"], |v| google_part(v, opaque))
                })
        }
        _ => false,
    }
}
pub struct ReasoningBudget {
    values: BTreeMap<String, f64>,
    path: PathBuf,
    selector: String,
}
impl ReasoningBudget {
    pub fn open(root: &Path, selector: &str) -> Result<Self> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        let path = root.join("reasoning-budget.json");
        let value: Value = match read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(crewroom_core::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                json!({})
            }
            Err(e) => return Err(e),
        };
        let invalid =
            || conflict("Saved reasoning budget is invalid; retain the session for inspection.");
        let values = value
            .as_object()
            .ok_or_else(invalid)?
            .iter()
            .map(|(k, v)| {
                if k.len() != 64
                    || !k
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(invalid());
                }
                Ok((k.clone(), count(v).ok_or_else(invalid)?))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            values,
            path,
            selector: selector.into(),
        })
    }
    pub fn key(&self, kind: &str, value: &str) -> String {
        format!(
            "{:x}",
            Sha256::digest(stringify(&json!([self.selector, kind, value])))
        )
    }
    pub fn tokens(&self, kind: &str, value: &str) -> Option<f64> {
        self.values.get(&self.key(kind, value)).copied()
    }
    pub fn remember(&mut self, blocks: &[(String, String)], output: f64) -> Result<()> {
        self.remember_keys(
            blocks
                .iter()
                .map(|(kind, value)| self.key(kind, value))
                .collect(),
            output,
        )
    }
    pub fn remember_keys(&mut self, keys: Vec<String>, output: f64) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        if !output.is_finite()
            || output < 0.0
            || output.fract() != 0.0
            || output > 9_007_199_254_740_991.0
        {
            return Err(conflict(
                "Provider output count cannot establish a reasoning budget.",
            ));
        }
        let mut next = self.values.clone();
        for key in keys {
            next.entry(key)
                .and_modify(|v| *v = v.max(output))
                .or_insert(output);
        }
        let tmp = self
            .path
            .with_extension(format!("json.{}", uuid::Uuid::new_v4()));
        durable_new(&tmp, stringify(&serde_json::to_value(&next)?).as_bytes())?;
        std::fs::rename(&tmp, &self.path)?;
        File::open(self.path.parent().unwrap())?.sync_all()?;
        self.values = next;
        Ok(())
    }
}
