//! The native tool policy is bound to the app-server version tested by FOR-7.
use super::*;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, process::Stdio};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{Duration, timeout},
};

pub const VERSION: &str = "codex-cli 0.162.0";
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn environment(home: &Path, state: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".into(), home.to_string_lossy().into()),
        ("CODEX_HOME".into(), state.to_string_lossy().into()),
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ])
}
async fn output(binary: &Path, args: &[&str], home: &Path, state: &Path) -> Result<Vec<u8>> {
    bounded_output(
        Command::new(binary)
            .args(args)
            .env_clear()
            .envs(environment(home, state))
            .current_dir(home),
    )
    .await
}
pub(super) async fn bounded_output(command: &mut Command) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap().take(16 * 1024 * 1024 + 1);
    let mut bytes = Vec::new();
    let result: Result<()> = timeout(Duration::from_secs(15), async {
        stdout.read_to_end(&mut bytes).await?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(conflict(
                "Native profile inspection exceeded its output bound.",
            ));
        }
        if !child.wait().await?.success() {
            return Err(conflict("Native profile inspection failed."));
        }
        Ok(())
    })
    .await
    .unwrap_or_else(|_| Err(conflict("Native profile inspection timed out.")));
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result?;
    Ok(bytes)
}
pub async fn prepare(binary: &Path, home: &Path, state: &Path, plan: &Value) -> Result<String> {
    canonical(binary)?;
    if output(binary, &["--version"], home, state).await? != format!("{VERSION}\n").as_bytes() {
        return Err(conflict(
            "Codex requires the verified 0.162.0 app-server profile.",
        ));
    }
    let raw: Value = serde_json::from_slice(
        &output(binary, &["debug", "models", "--bundled"], home, state).await?,
    )?;
    let expected = &plan["resolved"];
    let mut row = raw["models"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["slug"] == expected["id"]))
        .cloned()
        .ok_or_else(|| conflict("Selected model is absent from the pinned Codex catalog."))?;
    if !row["supported_reasoning_levels"]
        .as_array()
        .is_some_and(|levels| levels.iter().any(|l| l["effort"] == expected["effort"]))
    {
        return Err(conflict(
            "Selected effort is absent from the pinned Codex catalog.",
        ));
    }
    for (key, value) in [
        ("apply_patch_tool_type", Value::Null),
        ("supports_search_tool", json!(false)),
        ("experimental_supported_tools", json!([])),
        ("node_repl_disabled", json!(true)),
        ("tool_mode", json!("direct")),
        ("use_responses_lite", json!(false)),
    ] {
        row[key] = value;
    }
    let catalog = stringify(&json!({"models":[row]}));
    let path = home.join("models.json");
    durable_new(&path, catalog.as_bytes())?;
    let config = format!(
        "model = {}\nmodel_provider = \"openai\"\nmodel_reasoning_effort = {}\nweb_search = \"disabled\"\ncli_auth_credentials_store = \"ephemeral\"\nproject_doc_max_bytes = 0\nmodel_catalog_json = {}\n{}",
        stringify(&expected["id"]),
        stringify(&expected["effort"]),
        stringify(&json!(path)),
        RESTRICTED
    );
    // Attempt-specific configuration; CODEX_HOME is retained only for rollouts.
    let config_path = state.join("config.toml");
    if config_path.try_exists()? {
        let old = std::fs::symlink_metadata(&config_path)?;
        if !old.is_file() || old.permissions().mode() & 0o077 != 0 {
            return Err(conflict("Codex configuration changed identity."));
        }
        std::fs::remove_file(&config_path)?;
    }
    durable_new(&config_path, config.as_bytes())?;
    // Configuration contains an attempt path; the binding uses its stable content policy.
    Ok(digest(
        format!(
            "{}\n{}\n{}",
            digest(&crate::read(binary)?),
            catalog,
            RESTRICTED
        )
        .as_bytes(),
    ))
}
const RESTRICTED: &str = r#"
[tools.update_plan]
enabled = false
[tools.experimental_request_user_input]
enabled = false
[agents]
enabled = false
[features]
shell_tool = false
unified_exec = false
shell_snapshot = false
multi_agent = false
multi_agent_v2 = false
apps = false
plugins = false
hooks = false
code_mode_host = false
code_mode = false
code_mode_only = false
code_mode_tool_search = false
image_generation = false
browser_use = false
computer_use = false
view_image = false
skill_search = false
goals = false
sleep_tool = false
tool_suggest = false
enable_request_compression = false
"#;
