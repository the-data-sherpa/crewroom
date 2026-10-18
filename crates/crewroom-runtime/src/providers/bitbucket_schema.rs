use super::{schema::*, *};
use serde_json::{Value, json};
pub(super) fn repository_identity(value: &Value) -> Result<Value> {
    let id = string(&value["uuid"])?;
    let bytes = id.as_bytes();
    if bytes.len() != 38
        || bytes[0] != b'{'
        || bytes[37] != b'}'
        || !bytes[1..37]
            .iter()
            .all(|b| b.is_ascii_hexdigit() || *b == b'-')
    {
        return Err(invalid());
    }
    Ok(json!({"uuid":id}))
}
pub fn repository(value: &Value) -> Result<Value> {
    strict(value, &["uuid", "workspace", "slug"])?;
    let mut result = repository_identity(value)?;
    for key in ["workspace", "slug"] {
        let s = text(&value[key], 1, usize::MAX, false)?;
        if !s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || key == "slug" && b == b'.'
        }) {
            return Err(invalid());
        }
        result[key] = json!(s);
    }
    Ok(result)
}
fn reference(value: &Value) -> Result<Value> {
    Ok(
        json!({"branch":{"name":string(&value["branch"]["name"])?},"commit":{"hash":hash(&value["commit"]["hash"],true)?},"repository":repository_identity(&value["repository"])?}),
    )
}
pub(super) fn pull_request(value: &Value) -> Result<Value> {
    let state = string(&value["state"])?;
    if !["OPEN", "MERGED", "DECLINED", "SUPERSEDED"].contains(&state.as_str()) {
        return Err(invalid());
    }
    let mut result = json!({"id":integer(&value["id"],1)?,"title":string(&value["title"])?,"description":string(&value["description"])?,"state":state,"updated_on":date(&value["updated_on"])?,"source":reference(&value["source"])?,"destination":reference(&value["destination"])?,"links":{"html":{"href":url_text(&value["links"]["html"]["href"])?}}});
    if let Some(v) = value.get("merge_commit") {
        result["merge_commit"] = if v.is_null() {
            Value::Null
        } else {
            json!({"hash":hash(&v["hash"],true)?})
        };
    }
    if let Some(v) = value.get("draft") {
        result["draft"] = json!(boolean(v)?);
    }
    Ok(result)
}
fn state(value: &Value) -> Result<Value> {
    let mut result = json!({"name":string(&value["name"])?});
    if let Some(v) = value.get("result") {
        result["result"] = json!({"name":string(&v["name"])?});
    }
    Ok(result)
}
pub(super) fn status(value: &Value) -> Result<Value> {
    let mut result = json!({"key":string(&value["key"])?,"state":string(&value["state"])?,"name":string(&value["name"])?,"url":url_text(&value["url"])?,"updated_on":date(&value["updated_on"])?});
    if let Some(v) = value.get("refname") {
        result["refname"] = json!(string(v)?);
    }
    Ok(result)
}
pub(super) fn step(value: &Value) -> Result<Value> {
    Ok(
        json!({"uuid":string(&value["uuid"])?,"name":string(&value["name"])?,"state":state(&value["state"])?}),
    )
}
pub(super) fn pipeline(value: &Value) -> Result<Value> {
    let raw = &value["target"];
    let mut target =
        json!({"type":string(&raw["type"])?,"commit":{"hash":hash(&raw["commit"]["hash"],false)?}});
    if let Some(v) = raw.get("destination_commit") {
        target["destination_commit"] = json!({"hash":hash(&v["hash"],true)?});
    }
    for key in ["source", "destination"] {
        if let Some(v) = raw.get(key) {
            target[key] = json!(string(v)?);
        }
    }
    if let Some(v) = raw.get("pullrequest") {
        object(v)?;
        let id = v.get("id").ok_or_else(invalid)?;
        target["pullrequest"] = json!({"id":coerced_id(id)?});
    }
    target["selector"] = json!({"type":string(&raw["selector"]["type"])?,"pattern":string(&raw["selector"]["pattern"])?});
    let mut result = json!({"uuid":string(&value["uuid"])?,"build_number":integer(&value["build_number"],-9_007_199_254_740_991)?});
    if let Some(v) = value.get("run_number") {
        result["run_number"] = json!(integer(v, -9_007_199_254_740_991)?);
    }
    result["created_on"] = json!(date(&value["created_on"])?);
    result["state"] = state(&value["state"])?;
    result["target"] = target;
    Ok(result)
}
fn user(value: &Value) -> Result<Value> {
    object(value)?;
    let mut result = json!({});
    for key in ["uuid", "display_name"] {
        if let Some(v) = value.get(key) {
            result[key] = json!(string(v)?);
        }
    }
    Ok(result)
}
pub(super) fn comment(value: &Value) -> Result<Value> {
    let mut result = json!({"id":integer(&value["id"],1)?,"created_on":date(&value["created_on"])?,"updated_on":date(&value["updated_on"])?,"deleted":value.get("deleted").map(boolean).transpose()?.unwrap_or(false)});
    if let Some(v) = value.get("content") {
        result["content"] = json!({"raw":text(&v["raw"],0,40000,false)?});
    }
    if let Some(v) = value.get("user") {
        result["user"] = user(v)?;
    }
    for key in ["parent", "pullrequest"] {
        if let Some(v) = value.get(key) {
            result[key] = json!({"id":integer(&v["id"],1)?});
        }
    }
    if let Some(v) = value.get("inline") {
        let mut inline = json!({"path":text(&v["path"],0,1000,false)?});
        for key in ["from", "to", "start_from", "start_to"] {
            if let Some(v) = v.get(key) {
                inline[key] = if v.is_null() {
                    Value::Null
                } else {
                    json!(integer(v, 1)?)
                };
            }
        }
        if let Some(v) = v.get("outdated") {
            inline["outdated"] = json!(boolean(v)?);
        }
        result["inline"] = inline;
    }
    if let Some(v) = value.get("resolution") {
        let mut resolution = Value::Null;
        if !v.is_null() {
            object(v)?;
            resolution = json!({});
            if let Some(v) = v.get("created_on") {
                resolution["created_on"] = json!(date(v)?);
            }
            if let Some(v) = v.get("user") {
                resolution["user"] = user(v)?;
            }
        }
        result["resolution"] = resolution;
    }
    Ok(result)
}
