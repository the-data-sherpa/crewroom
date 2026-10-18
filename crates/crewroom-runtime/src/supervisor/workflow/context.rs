//! Bounded continuation projections. Omitted bodies remain in their durable
//! ledgers/artifacts; a projection never changes delivery or evidence state.
use super::{array, present, remove, text};
use crate::{conflict, host_protocol::parse_request};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};

pub fn present_briefs(own: &Value, assignments: &[Value]) -> Result<String> {
    let active: Vec<_> = assignments
        .iter()
        .filter(|a| !present(a, "retiredAt"))
        .collect();
    let referenced: Vec<_> = active
        .iter()
        .flat_map(|a| array(&a["brief"]["dependsOn"]))
        .collect();
    let describe = |a: &Value| {
        if present(a, "brief") {
            let mut brief =
                json!({"criteria":a["brief"]["criteria"],"dependsOn":a["brief"]["dependsOn"]});
            if let Some(principles) = a["brief"].get("principles") {
                brief["principles"] = principles.clone();
            }
            brief
        } else {
            json!("No structured brief (admitted before structured briefs).")
        }
    };
    let team: Vec<_> = assignments
        .iter()
        .filter(|a| !present(a, "retiredAt") || referenced.contains(&&a["name"]))
        .map(|a| {
            let mut v = json!({"name":a["name"],"role":a["role"]["name"]});
            if present(a, "retiredAt") {
                v["retired"] = json!(true);
            }
            v["brief"] = describe(a);
            v
        })
        .collect();
    Ok(format!(
        "Structured brief for {}: {}\nTeam briefs (criterion index is zero-based; reviewers assess coverage against these obligations): {}",
        text(own, "name")?,
        stringify(&describe(own)),
        stringify(&json!(team))
    ))
}

pub fn coding_context(record: Option<&Value>) -> Result<Option<Value>> {
    let Some(record) = record else {
        return Ok(None);
    };
    if !record.is_object() {
        return Err(conflict("Invalid coding record."));
    }
    let mut result = record.clone();
    if let Some(edits) = record.get("edits") {
        let edits = edits
            .as_array()
            .ok_or_else(|| conflict("Invalid coding edits."))?;
        result["edits"] = json!(
            edits
                .iter()
                .map(|edit| {
                    let mut v = edit.clone();
                    let mut input = json!({});
                    for key in ["path", "expectedHash"] {
                        if let Some(value) = edit["input"].get(key) {
                            input[key] = value.clone();
                        }
                    }
                    v["input"] = input;
                    v
                })
                .collect::<Vec<_>>()
        );
    }
    Ok(Some(result))
}

/// Native strings never contain an unpaired surrogate. If JavaScript's
/// 300-UTF16-unit cut would split a pair, keep the preceding complete scalar
/// and count the whole omitted scalar. No original ledger body is changed.
fn brief(body: &str, keep: bool) -> String {
    let length = body.encode_utf16().count();
    if keep || length <= 300 {
        return body.into();
    }
    let mut units = 0;
    let end = body
        .char_indices()
        .find_map(|(i, c)| {
            if units + c.len_utf16() > 300 {
                Some(i)
            } else {
                units += c.len_utf16();
                None
            }
        })
        .unwrap_or(body.len());
    format!(
        "{}… [{} characters omitted; reconciled earlier]",
        &body[..end],
        length - units
    )
}
fn projected_messages(messages: &[Value], steering: bool) -> Result<Vec<Value>> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if steering && message["delivery"] == "not_delivered" {
                let mut v = json!({"id":message["id"],"delivery":message["delivery"]});
                if let Some(reason) = message.get("dispositionReason") {
                    v["dispositionReason"] = reason.clone();
                }
                return Ok(v);
            }
            let mut v = message.clone();
            let keep = if steering {
                message["delivery"] != "applied" || index + 5 >= messages.len()
            } else {
                message["disposition"] != "received" || index + 10 >= messages.len()
            };
            v["body"] = json!(brief(text(message, "body")?, keep));
            Ok(v)
        })
        .collect()
}
pub struct ContinuationContext<'a> {
    pub coding: Option<&'a Value>,
    pub work: Option<&'a Value>,
    pub publication: Option<&'a Value>,
    pub steering: &'a [Value],
    pub messages: &'a [Value],
    pub notices: &'a [Value],
}
pub fn continuation_context(input: ContinuationContext<'_>) -> Result<Value> {
    let ContinuationContext {
        coding,
        work,
        publication,
        steering,
        messages,
        notices,
    } = input;
    let mut result = json!({});
    if let Some(mut coding) = coding_context(coding)? {
        let checks = coding["checks"]
            .as_array()
            .ok_or_else(|| conflict("Invalid coding check history."))?;
        coding["checks"] = json!(
            checks
                .iter()
                .map(|check| {
                    let mut v = check.clone();
                    remove(&mut v, &["definition", "output", "source"]);
                    if let Some(hash) = check["output"].get("sha256") {
                        v["outputSha256"] = hash.clone();
                    }
                    if let Some(hash) = check["source"].get("sha256") {
                        v["sourceSha256"] = hash.clone();
                    }
                    v
                })
                .collect::<Vec<_>>()
        );
        if let Some(operations) = coding.get("checkOperations") {
            let operations = operations
                .as_array()
                .ok_or_else(|| conflict("Invalid check operation history."))?;
            coding["checkOperations"] = json!(
                operations
                    .iter()
                    .map(|op| {
                        let mut v = op.clone();
                        remove(&mut v, &["definition", "workspace", "source"]);
                        if let Some(id) = op["definition"].get("id") {
                            v["checkId"] = id.clone();
                        }
                        v
                    })
                    .collect::<Vec<_>>()
            );
        }
        result["coding"] = coding;
    }
    if let Some(work) = work {
        let mut projected = work.clone();
        let instructions = work["instructions"]
            .as_array()
            .ok_or_else(|| conflict("Invalid repository instructions."))?;
        projected["instructions"] = json!(
            instructions
                .iter()
                .map(|i| json!({"path":i["path"],"sha256":i["sha256"]}))
                .collect::<Vec<_>>()
        );
        if let Some(refreshes) = work.get("refreshes") {
            let refreshes = refreshes
                .as_array()
                .ok_or_else(|| conflict("Invalid target refresh history."))?;
            projected["refreshes"] = json!(
                refreshes
                    .iter()
                    .map(|r| {
                        let mut v = r.clone();
                        remove(&mut v, &["resolutionWorkspace"]);
                        if present(r, "resolutionWorkspace") {
                            let mut workspace = json!({});
                            for key in ["state", "toHead"] {
                                if let Some(field) = r["resolutionWorkspace"].get(key) {
                                    workspace[key] = field.clone();
                                }
                            }
                            v["resolutionWorkspace"] = workspace;
                        }
                        v
                    })
                    .collect::<Vec<_>>()
            );
        }
        let checkpoints = work["checkpoints"]
            .as_array()
            .ok_or_else(|| conflict("Invalid repository checkpoints."))?;
        projected["checkpoints"] = json!(&checkpoints[checkpoints.len().saturating_sub(3)..]);
        projected["earlierCheckpoints"] = json!(checkpoints.len().saturating_sub(3));
        result["repositoryWork"] = projected;
        if let Some(target) = array(&work["refreshes"])
            .iter()
            .rev()
            .find(|r| r["state"] == "conflicted" && !present(r, "supersededBy"))
        {
            result["targetConflict"] = json!({"requestId":target["requestId"],
                "state":target["resolutionWorkspace"]["state"].as_str().unwrap_or("operator_required"),
                "conflicts":target.get("conflicts").cloned().unwrap_or_else(||json!([]))});
        }
    }
    result["steering"] = json!(projected_messages(steering, true)?);
    result["peerMessages"] = json!(projected_messages(messages, false)?);
    result["deliveryNotices"] = json!(
        notices
            .iter()
            .enumerate()
            .filter(|(i, n)| n["disposition"] == "pending" || i + 5 >= notices.len())
            .map(|(_, n)| n)
            .collect::<Vec<_>>()
    );
    result["earlierDeliveryNotices"] = json!(
        notices
            .iter()
            .filter(|n| n["disposition"] != "pending")
            .count()
            .saturating_sub(5)
    );
    if let Some(publication) = publication {
        let mut p = publication.clone();
        remove(&mut p, &["description", "observation", "feedback"]);
        result["publication"] = p;
    }
    Ok(result)
}

pub fn session_needs_replacement(events: &[Value], attempt: &str) -> bool {
    if events.iter().any(|e| {
        e["attemptId"] == attempt
            && e["event"]["type"] == "model_policy_rejected"
            && e["event"]["code"] == "input_budget"
            && e["event"]["recovery"] == "fresh_session"
    }) {
        return true;
    }
    let last = events.iter().rev().find(|e| {
        e["attemptId"] == attempt
            && e["event"]["type"] == "message_end"
            && e["event"]["message"]["role"] == "assistant"
    });
    last.is_some_and(|e| {
        let m = &e["event"]["message"];
        let message = m["errorMessage"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase();
        m["stopReason"] == "error"
            && (["prompt is too long", "maximum context", "too many tokens"]
                .iter()
                .any(|p| message.contains(p))
                || ["", " ", "_", "-"].iter().any(|sep| {
                    ["length", "window"]
                        .iter()
                        .any(|end| message.contains(&format!("context{sep}{end}")))
                }))
    })
}

pub fn input_history(inputs: &[Value], attempt: &str) -> Vec<Value> {
    inputs
        .iter()
        .map(|i| {
            let mut v = i.clone();
            if i["disposition"] == "completed"
                || i["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("continuation:"))
            {
                remove(&mut v, &["message"]);
            }
            v["attemptId"] = json!(attempt);
            v
        })
        .collect()
}

/// Validate the complete JSON envelope, including its escaped message, using
/// the host's actual UTF16 input limit. Oversized work is retained, not truncated.
pub fn host_input(id: &str, message: &Value, mode: Option<&str>) -> Result<Value> {
    let mut body = json!({"op":"input","id":id,"message":stringify(message)});
    if let Some(mode) = mode {
        body["mode"] = json!(mode);
    }
    parse_request(stringify(&body).as_bytes())?;
    Ok(body)
}
