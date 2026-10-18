//! Read only the assignment's recorded Claude transcript before any replacement
//! can execute. A missing receipt is a hold, never evidence of no effect.
use super::journal::RetainedToolReceipt;
use super::*;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{BufRead, BufReader},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

pub struct History {
    pub receipts: BTreeMap<String, RetainedToolReceipt>,
    pub inputs: BTreeMap<String, String>,
    pub usage: Option<Value>,
}
pub fn read(path: &Path, session: &str, model: &str) -> Result<History> {
    canonical(
        path.parent()
            .ok_or_else(|| conflict("Missing Claude history parent."))?,
    )?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| {
            conflict("Claude retained history is unavailable; no input or effect was replayed.")
        })?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.len() > 128 * 1024 * 1024
    {
        return Err(conflict(
            "Claude history is not an owned bounded regular file.",
        ));
    }
    let mut reader = BufReader::new(file);
    let mut inputs = BTreeMap::new();
    let mut calls = BTreeMap::<String, Value>::new();
    let mut results = BTreeMap::<String, Value>::new();
    let mut usage = None;
    let mut leaf = None::<String>;
    let mut nodes = std::collections::BTreeSet::new();
    let mut batch = None::<String>;
    let mut batch_calls = BTreeMap::<String, String>::new();
    loop {
        let mut line = Vec::new();
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buf.len(), |i| i + 1);
            if line.len() + n > 8 * 1024 * 1024 {
                return Err(conflict("Claude transcript frame exceeds its bound."));
            }
            let done = buf[n - 1] == b'\n';
            line.extend_from_slice(&buf[..n]);
            reader.consume(n);
            if done {
                break;
            }
        }
        if line.is_empty() {
            break;
        }
        if !line.ends_with(b"\n") {
            return Err(conflict("Claude history contains an incomplete append."));
        }
        let row: Value =
            serde_json::from_slice(&line).map_err(|_| conflict("Claude history is malformed."))?;
        if row.get("sessionId").is_some() && row["sessionId"] != session {
            return Err(conflict("Claude history belongs to another session."));
        }
        if row["isSidechain"] == true {
            return Err(conflict("Claude history contains an unadmitted subagent."));
        }
        // Native --resume follows a selected parent chain, not all JSONL rows.
        // Support an append-only chain plus results parented to their own tool
        // block in the current streamed assistant message (Claude 2.1.296).
        // The CLI restores these batch siblings when resuming. Arbitrary user
        // branches are not tool receipts. Compaction rewrites/relinks that
        // chain; until those rules are verified, retain receipts and hold.
        if row["subtype"] == "compact_boundary"
            || row["type"] == "content-replacement"
            || row.get("logicalParentUuid").is_some()
            || row.get("preservedSegment").is_some()
            || row.get("preservedMessages").is_some()
        {
            return Err(conflict(
                "Claude history was compacted or relinked; reconcile its retained context before resume.",
            ));
        }
        if row.get("parentUuid").is_some() {
            let id = text(&row, "uuid")?;
            let content = row["message"]["content"].as_array();
            let tool_result = row["type"] == "user"
                && content.is_some_and(|c| c.iter().any(|i| i["type"] == "tool_result"));
            let batch_result = tool_result
                && batch.is_some()
                && content.is_some_and(|c| {
                    !c.is_empty()
                        && c.iter().all(|item| {
                            item["type"] == "tool_result"
                                && item["tool_use_id"].as_str().is_some_and(|call| {
                                    batch_calls.get(call).map(String::as_str)
                                        == row["parentUuid"].as_str()
                                        && batch_calls.contains_key(call)
                                        && !results.contains_key(call)
                                })
                        })
                });
            if row["sessionId"] != session
                || (row["parentUuid"].as_str() != leaf.as_deref() && !batch_result)
                || (tool_result && !batch_result)
                || !nodes.insert(id.to_owned())
            {
                return Err(conflict(
                    "Claude history has an ambiguous or disconnected resume chain.",
                ));
            }
            leaf = Some(id.to_owned());
        } else if row["type"] == "assistant" || row["type"] == "user" {
            return Err(conflict(
                "Claude retained message has no parent-chain identity.",
            ));
        }
        if row["type"] == "last-prompt"
            && row.get("leafUuid").is_some()
            && row["leafUuid"].as_str() != leaf.as_deref()
        {
            return Err(conflict(
                "Claude resume checkpoint selects an earlier branch.",
            ));
        }
        if row["type"] == "assistant" {
            let message_id = row["message"]["id"].as_str().map(str::to_owned);
            if message_id.is_none() || message_id != batch {
                batch_calls.clear();
                batch = message_id;
            }
            if row["message"]["model"] != model && row["message"]["model"] != "<synthetic>" {
                return Err(conflict(
                    "Claude retained model differs from the frozen assignment.",
                ));
            }
            for item in row["message"]["content"].as_array().into_iter().flatten() {
                if item["type"] == "tool_use" {
                    let id = text(item, "id")?.to_owned();
                    let call = json!({"tool":item["name"].as_str().and_then(|n|n.strip_prefix("mcp__crewroom__")).ok_or_else(||conflict("Claude history contains an unadmitted tool."))?,"arguments":item["input"]});
                    if calls.get(&id).is_some_and(|old| old != &call) {
                        return Err(conflict("Claude tool history reused a call identity."));
                    }
                    calls.insert(id, call);
                    batch_calls
                        .insert(text(item, "id")?.to_owned(), text(&row, "uuid")?.to_owned());
                }
            }
        }
        if row["type"] == "user" {
            if let Some(body) = row["message"]["content"].as_str() {
                batch_calls.clear();
                batch = None;
                let id = text(&row, "uuid")?.to_owned();
                let hash = profile::digest(body.as_bytes());
                if inputs.get(&id).is_some_and(|old| old != &hash) {
                    return Err(conflict("Claude input history changed identity."));
                }
                inputs.insert(id, hash);
            }
            for item in row["message"]["content"].as_array().into_iter().flatten() {
                if item["type"] == "tool_result" {
                    let id = text(item, "tool_use_id")?.to_owned();
                    let value =
                        json!({"success":item["is_error"]!=true,"contentItems":item["content"]});
                    if results.get(&id).is_some_and(|old| old != &value) {
                        return Err(conflict("Claude result history changed identity."));
                    }
                    results.insert(id, value);
                }
            }
        }
        if row["type"] == "cost-state" {
            usage = Some(row["modelUsage"].clone());
        }
    }
    Ok(History {
        receipts: calls
            .into_iter()
            .map(|(id, call)| {
                let receipt = RetainedToolReceipt {
                    tool: call["tool"].as_str().unwrap().into(),
                    arguments: call["arguments"].clone(),
                    result: results.remove(&id),
                };
                (id, receipt)
            })
            .collect(),
        inputs,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(rows: &[Value]) -> Result<History> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(
            &path,
            rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )
        .unwrap();
        read(&path, "session", "model")
    }
    fn parallel_tools() -> Vec<Value> {
        // Minimized from Claude Code 2.1.296's actual retained transcript:
        // streamed blocks share message.id; each result parents its own call.
        vec![
            json!({"type":"assistant","uuid":"a","parentUuid":null,"sessionId":"session","message":{"id":"turn","model":"model","content":[{"type":"tool_use","id":"call-a","name":"mcp__crewroom__repository_read","input":{"path":"a"}}]}}),
            json!({"type":"assistant","uuid":"b","parentUuid":"a","sessionId":"session","message":{"id":"turn","model":"model","content":[{"type":"tool_use","id":"call-b","name":"mcp__crewroom__repository_read","input":{"path":"b"}}]}}),
            json!({"type":"user","uuid":"result-a","parentUuid":"a","sessionId":"session","message":{"content":[{"type":"tool_result","tool_use_id":"call-a","content":"a bytes"}]}}),
            json!({"type":"user","uuid":"result-b","parentUuid":"b","sessionId":"session","message":{"content":[{"type":"tool_result","tool_use_id":"call-b","content":"b bytes"}]}}),
        ]
    }
    #[test]
    fn parallel_tool_results_retain_both_receipts_without_accepting_branches() {
        let rows = parallel_tools();
        let history = fixture(&rows).unwrap();
        assert_eq!(history.receipts.len(), 2);
        assert!(history.receipts.values().all(|r| r.result.is_some()));
        let mut wrong = rows.clone();
        wrong[2]["parentUuid"] = json!("b");
        assert!(fixture(&wrong).is_err());
        let mut branch = rows.clone();
        branch[2]["message"]["content"] = json!("Another user branch");
        assert!(fixture(&branch).is_err());
        let mut different_turn = rows.clone();
        different_turn[1]["message"]["id"] = json!("other-turn");
        assert!(fixture(&different_turn).is_err());
        let mut duplicate = rows.clone();
        duplicate.push(json!({"type":"user","uuid":"again","parentUuid":"a","sessionId":"session","message":{"content":[{"type":"tool_result","tool_use_id":"call-a","content":"a bytes"}]}}));
        assert!(fixture(&duplicate).is_err());
        let mut reversed = rows[..2].to_vec();
        reversed.push(rows[3].clone());
        reversed.push(rows[2].clone());
        assert_eq!(fixture(&reversed).unwrap().receipts.len(), 2);
    }
}
