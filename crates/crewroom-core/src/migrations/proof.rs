//! Read-only proof for the narrowly sealed legacy promotion terminal. This does
//! not probe or kill PIDs, modify manifests, or establish ownership of other Git.
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}
fn rows(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
pub(super) fn checked_terminal(work: &Value, record: &Value, refresh: &Value) -> bool {
    let workspace = &refresh["resolutionWorkspace"];
    let process = &workspace["promotionProcess"];
    let Some(operation) = text(workspace, "operationId") else {
        return false;
    };
    let Some(head) = text(workspace, "toHead") else {
        return false;
    };
    let Some(completion) = rows(&record["completions"])
        .iter()
        .find(|r| r["id"] == operation)
    else {
        return false;
    };
    let Some(source) = rows(&record["candidates"])
        .iter()
        .find(|r| r["id"] == refresh["sourceCandidateId"])
    else {
        return false;
    };
    let rejection = &completion["rejection"];
    if process["version"] != 1
        || completion["conflictRefreshId"] != refresh["requestId"]
        || completion["state"] != "rejected"
        || !completion["candidateId"].is_null()
        || completion["head"] != refresh["fromHead"]
        || source["revision"]["base"] != refresh["fromBase"]
        || source["revision"]["head"] != refresh["fromHead"]
        || !rows(&record["claims"]).iter().any(|c| {
            c["writerAttemptId"] == completion["attemptId"]
                && c["targetRefreshId"] == refresh["requestId"]
        })
        || !["target_changed", "promotion_interrupted"]
            .contains(&rejection["code"].as_str().unwrap_or(""))
        || rejection["expectedTarget"] != refresh["toBase"]
        || text(rejection, "at").is_none()
        || text(rejection, "reason").is_none()
        || rejection["code"] == "target_changed"
            && (text(rejection, "observedTarget").is_none()
                || rejection["observedTarget"] == refresh["toBase"])
        || text(process, "token").is_none()
        || text(process, "epoch").is_none()
        || !text(process, "controllerRoot").is_some_and(|s| s.starts_with('/'))
        || !text(process, "command").is_some_and(|s| s.starts_with('/'))
        || process["cwd"] != work["path"]
    {
        return false;
    }
    let Some(root) = text(process, "root") else {
        return false;
    };
    if !root.ends_with(&format!(
        "/{operation}-{}",
        process["token"].as_str().unwrap()
    )) || workspace["branch"]
        != format!(
            "{}-refresh-{}",
            work["branch"].as_str().unwrap_or(""),
            refresh["requestId"].as_str().unwrap_or("")
        )
    {
        return false;
    }
    let Some(args) = process["args"].as_array() else {
        return false;
    };
    if args.len() < 4
        || args[args.len() - 4..]
            != [
                json!("reset"),
                json!("--keep"),
                json!("--no-recurse-submodules"),
                json!(head),
            ]
    {
        return false;
    }
    let Some(env) = process["env"].as_object() else {
        return false;
    };
    // These are exactly the keys admitted by the retained checkedSpec.
    // Foreign environment names or non-string values cannot prove a legacy seal.
    let admitted = [
        "PATH",
        "HOME",
        "LANG",
        "SSH_AUTH_SOCK",
        "GIT_TERMINAL_PROMPT",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_NOSYSTEM",
        "GIT_SSH_COMMAND",
    ];
    if env
        .iter()
        .any(|(k, v)| !admitted.contains(&k.as_str()) || !v.is_string())
    {
        return false;
    }
    let mut keys: Vec<_> = env.keys().collect();
    keys.sort_by_key(|k| {
        k.bytes()
            .map(|c| if c == b'_' { 0 } else { c })
            .collect::<Vec<_>>()
    });
    let mut sorted = Map::new();
    for key in keys {
        sorted.insert(key.clone(), env[key].clone());
    }
    let raw = json!({"version":1,"token":process["token"],"command":process["command"],"args":args,"cwd":process["cwd"],"env":sorted,"epoch":process["epoch"],"controllerRoot":process["controllerRoot"]});
    let expected = digest(crate::serialized::stringify(&raw));
    if process["specSha256"] != expected {
        return false;
    }
    let path = Path::new(root);
    if std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return false;
    }
    let Ok(seal) = std::fs::read(path.join("seal.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .ok_or(())
    else {
        return false;
    };
    if seal != json!({"token":process["token"],"specSha256":expected}) {
        return false;
    }
    match std::fs::read(path.join("launch.json")) {
        Ok(bytes) => digest(bytes) == expected,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}
