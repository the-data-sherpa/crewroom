//! Pinned native login profile. Claude owns its original login and refresh locks;
//! Crewroom never copies refresh credentials into assignment state.
use super::*;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};
use tokio::process::Command;

pub const VERSION: &str = "2.1.296 (Claude Code)";
pub const POLICY: &str = "claude-2.1.296-safe-sdk-tools-v1";

pub fn account(home: &Path) -> Result<String> {
    let path = home.join(".claude.json");
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)
        .map_err(|_| conflict("Native Claude account metadata is unavailable; sign in with Claude before admission."))?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > 16 * 1024 * 1024
    {
        return Err(conflict(
            "Native Claude account metadata must be a private owned file.",
        ));
    }
    let value: Value = serde_json::from_reader(file)
        .map_err(|_| conflict("Native Claude account metadata is invalid."))?;
    let account = text(&value["oauthAccount"], "accountUuid")
        .map_err(|_| conflict("Native Claude login needs an account identity."))?;
    let organization = text(&value["oauthAccount"], "organizationUuid")
        .map_err(|_| conflict("Native Claude login needs an organization identity."))?;
    Ok(profile::digest(
        format!("{account}:{organization}").as_bytes(),
    ))
}

pub fn environment(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".into(), home.to_string_lossy().into()),
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LANG".into(), "C.UTF-8".into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN".into(), "0".into()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ])
}

pub async fn prepare(binary: &Path, home: &Path, cwd: &Path) -> Result<(String, PathBuf)> {
    canonical(binary)?;
    canonical(home)?;
    let output = profile::bounded_output(
        Command::new(binary)
            .arg("--version")
            .env_clear()
            .envs(environment(home))
            .current_dir(cwd),
    )
    .await?;
    if output != format!("{VERSION}\n").as_bytes() {
        return Err(conflict(
            "Claude requires the verified 2.1.296 native profile.",
        ));
    }
    // The standard native HOME preserves the original account metadata/keychain
    // and native refresh locks. All settings/customizations are disabled by argv.
    let encoded: String = cwd
        .to_str()
        .ok_or_else(|| conflict("Claude cwd must be UTF-8."))?
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if encoded.len() > 180 {
        return Err(conflict(
            "Claude assignment path exceeds the verified session-path profile.",
        ));
    }
    let sessions = home.join(".claude/projects").join(encoded);
    private_dir(&sessions)?;
    Ok((
        profile::digest(format!("{}:{POLICY}", profile::digest(&crate::read(binary)?)).as_bytes()),
        sessions,
    ))
}

pub fn arguments(model: &Value, prompt: &str, session: &str, resume: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--safe-mode",
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--replay-user-messages",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{"crewroom":{"type":"sdk","name":"crewroom"}}}"#,
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-chrome",
        "--permission-mode",
        "dontAsk",
        "--model",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.extend([
        model["id"].as_str().unwrap().into(),
        "--effort".into(),
        model["effort"].as_str().unwrap().into(),
        "--system-prompt".into(),
        prompt.into(),
        if resume { "--resume" } else { "--session-id" }.into(),
        session.into(),
    ]);
    args
}

/// The embedded host catalog deliberately uses this small JSON Schema subset.
/// Reject unsupported schemas at admission instead of silently skipping rules.
pub fn schema(schema: &Value, value: &Value) -> Result<()> {
    if !schema.is_object()
        || schema.as_object().unwrap().keys().any(|k| {
            ![
                "type",
                "properties",
                "required",
                "additionalProperties",
                "items",
                "enum",
                "minimum",
                "maximum",
                "minItems",
                "maxItems",
                "description",
            ]
            .contains(&k.as_str())
        })
    {
        return Err(conflict("Unsupported Crewroom tool schema."));
    }
    let types = schema["type"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(&schema["type"]));
    if types.is_empty() {
        return Err(conflict("Unsupported Crewroom tool schema type."));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut matches = false;
    // Validate every member even when an earlier type matches the value. A
    // nullable field must not turn an unknown type into an accepted schema.
    for kind in types {
        if !seen.insert(kind.as_str()) {
            return Err(conflict("Unsupported Crewroom tool schema type."));
        }
        matches |= match kind.as_str() {
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("string") => value.is_string(),
            Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
            Some("number") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("null") => value.is_null(),
            _ => return Err(conflict("Unsupported Crewroom tool schema type.")),
        };
    }
    if !matches
        || schema["enum"]
            .as_array()
            .is_some_and(|e| !e.contains(value))
    {
        return Err(conflict(
            "Claude tool arguments do not match the admitted schema.",
        ));
    }
    if let Some(object) = value.as_object() {
        if schema["required"].as_array().is_some_and(|a| {
            a.iter()
                .any(|k| k.as_str().is_none_or(|k| !object.contains_key(k)))
        }) {
            return Err(conflict("Claude tool arguments omit required fields."));
        }
        for (k, v) in object {
            if let Some(s) = schema["properties"].get(k) {
                self::schema(s, v)?;
            } else if schema["additionalProperties"] != true {
                return Err(conflict("Claude tool arguments contain unadmitted fields."));
            }
        }
    }
    if let Some(array) = value.as_array() {
        if schema["minItems"]
            .as_u64()
            .is_some_and(|n| array.len() < n as usize)
            || schema["maxItems"]
                .as_u64()
                .is_some_and(|n| array.len() > n as usize)
        {
            return Err(conflict(
                "Claude tool array is outside its admitted bounds.",
            ));
        }
        for item in array {
            self::schema(&schema["items"], item)?;
        }
    }
    if let Some(n) = value.as_f64()
        && (schema["minimum"].as_f64().is_some_and(|m| n < m)
            || schema["maximum"].as_f64().is_some_and(|m| n > m))
    {
        return Err(conflict(
            "Claude tool number is outside its admitted bounds.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_schema() -> Value {
        let catalog: Value = serde_json::from_str(include_str!("../host-tools.json")).unwrap();
        catalog["definitions"]["write_file"]["parameters"].clone()
    }

    #[test]
    fn actual_write_file_schema_accepts_only_nullable_hash_values() {
        let definition = write_schema();
        for hash in [Value::Null, json!("0123456789abcdef")] {
            assert!(
                schema(
                    &definition,
                    &json!({"path":"helper.cjs","content":"new source","expectedHash":hash})
                )
                .is_ok()
            );
        }
        for hash in [json!(false), json!(17), json!([]), json!({})] {
            assert!(
                schema(
                    &definition,
                    &json!({"path":"helper.cjs","content":"new source","expectedHash":hash})
                )
                .is_err()
            );
        }
        assert!(
            schema(
                &definition,
                &json!({"path":"helper.cjs","content":"new source"})
            )
            .is_err()
        );
        assert!(schema(&definition, &json!({"path":"helper.cjs","content":"new source","expectedHash":null,"extra":true})).is_err());
    }

    #[test]
    fn unsupported_union_members_fail_even_after_a_matching_type() {
        for kinds in [
            json!([]),
            json!(["string", "unknown"]),
            json!(["string", 7]),
            json!(["string", "string"]),
        ] {
            assert!(schema(&json!({"type":kinds}), &json!("matches string")).is_err());
        }
        assert!(
            schema(
                &json!({"type":["string","null"],"unknownKeyword":true}),
                &Value::Null
            )
            .is_err()
        );
        let mut definition = write_schema();
        definition["properties"]["expectedHash"]["type"] = json!(["null", "unknown"]);
        assert!(
            schema(
                &definition,
                &json!({"path":"x","content":"x","expectedHash":null})
            )
            .is_err()
        );
    }

    #[test]
    fn existing_array_enum_and_numeric_guards_remain_enforced() {
        let catalog: Value = serde_json::from_str(include_str!("../host-tools.json")).unwrap();
        let definition = &catalog["definitions"]["edit_file"]["parameters"];
        let valid = json!({"path":"helper.cjs","expectedHash":"hash","replacements":[{"oldText":"before","newText":"after"}]});
        assert!(schema(definition, &valid).is_ok());
        for replacements in [
            json!("[{\"oldText\":\"before\",\"newText\":\"after\"}]"),
            json!([]),
            json!([{"oldText":"before"}]),
            json!([{"oldText":"before","newText":17}]),
            json!([{"oldText":"before","newText":"after","extra":true}]),
        ] {
            let mut input = valid.clone();
            input["replacements"] = replacements;
            assert!(schema(definition, &input).is_err());
        }
        assert!(schema(&json!({"type":["integer","null"],"minimum":1}), &json!(1)).is_ok());
        assert!(schema(&json!({"type":["integer","null"],"minimum":1}), &json!(0)).is_err());
        assert!(
            schema(
                &json!({"type":["string","null"],"enum":["complete",null]}),
                &json!("incomplete")
            )
            .is_err()
        );
    }
}
