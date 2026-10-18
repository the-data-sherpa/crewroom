//! OMP adapter. Retains v2 negotiation, private settings and legacy session identity.
use super::*;
use crate::{
    model_policy::{configure_assignment, isolated_model_settings},
    rpc::OmpRpc,
};
use std::{collections::BTreeMap, os::unix::fs::MetadataExt};
use url::Url;

pub(super) async fn open(
    helper: &Path,
    root: &Path,
    config: &Value,
    origin: &str,
    tools: &[Value],
    event: Event,
    stopping: &tokio_util::sync::CancellationToken,
) -> Result<(Runner, Value)> {
    let private = root.join("config");
    private_dir(&private)?;
    let temporary = root.join("tmp");
    private_dir(&temporary)?;
    let sessions = root.join("sessions");
    private_dir(&sessions)?;
    let omp_root = root.join("omp-root");
    private_dir(&omp_root)?;
    let mut env: BTreeMap<String, String> = serde_json::from_value(config["env"].clone())?;
    const ALLOWED: &[&str] = &[
        "PATH",
        "HOME",
        "LANG",
        "TMPDIR",
        "PI_CODING_AGENT_DIR",
        "PI_CODING_AGENT_SESSION_DIR",
        "PI_CONFIG_DIR",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "NO_COLOR",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_NOSYSTEM",
        "CREWROOM_PROCESS_INSPECTOR",
    ];
    if env.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(conflict("Host environment contains unadmitted fields."));
    }
    let home = Path::new(
        env.get("HOME")
            .ok_or_else(|| conflict("Runner HOME is required."))?,
    );
    let root_setting = env
        .get("PI_CONFIG_DIR")
        .ok_or_else(|| conflict("Runner private configuration root is required."))?;
    if home.join(root_setting).canonicalize()? != omp_root {
        return Err(conflict(
            "Runner configuration root is not private to its attempt.",
        ));
    }
    env.insert("TMPDIR".into(), temporary.to_string_lossy().into());
    env.insert(
        "PI_CODING_AGENT_DIR".into(),
        private.to_string_lossy().into(),
    );
    env.insert(
        "PI_CODING_AGENT_SESSION_DIR".into(),
        sessions.to_string_lossy().into(),
    );
    for (key, child) in [("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")] {
        env.insert(key.into(), root.join(child).to_string_lossy().into());
    }
    let mut proxy = config["upstreamProvider"].clone();
    let upstream =
        Url::parse(text(&proxy, "baseUrl")?).map_err(|_| conflict("Invalid provider URL."))?;
    proxy["baseUrl"] = json!(format!(
        "{}/provider{}",
        origin,
        upstream.path().trim_end_matches('/')
    ));
    proxy["apiKey"] = config["providerToken"].clone();
    proxy["headers"] = json!({});
    proxy
        .as_object_mut()
        .ok_or_else(|| conflict("Invalid provider configuration."))?
        .remove("authMode");
    let provider = text(&config["model"]["resolved"], "provider")?;
    let models = json!({"providers":{provider:proxy}});
    for (name, value) in [
        ("models.yml", models),
        ("config.yml", isolated_model_settings(&config["model"])),
        ("mcp.json", json!({"mcpServers":{}})),
    ] {
        let path = private.join(name);
        // Configuration belongs to this fresh attempt; replace only an ordinary
        // private file prepared by its controller, never a symlink or alias.
        if path.try_exists()? {
            let m = std::fs::symlink_metadata(&path)?;
            if !m.is_file() || m.mode() & 0o077 != 0 {
                return Err(conflict("Runner configuration is not private."));
            }
            std::fs::remove_file(&path)?;
        }
        durable_new(&path, stringify(&value).as_bytes())?;
    }
    let command = text(config, "command")?.to_owned();
    if !Path::new(&command).is_absolute() {
        return Err(conflict("Runner command must be absolute."));
    }
    let spec = CommandSpec {
        command,
        args: serde_json::from_value(config["args"].clone())?,
        cwd: root.to_path_buf(),
        env,
    };
    let observer = event.clone();
    let rpc = OmpRpc::start(helper, &spec, move |frame| observer(frame)).await?;
    if stopping.is_cancelled() {
        rpc.close().await?;
        return Err(conflict("Host stopped during setup."));
    }
    let result: Result<Value> = async {
        let mut recovery = if config.get("previousAttemptId").is_some() {
            Some("replacement")
        } else {
            None
        };
        let mut session_directory = sessions.clone();
        if let Some(previous) = config["resumeSessionDir"].as_str() {
            let restored = rpc
                .command("open_session", json!({"sessionDir":previous}))
                .await?;
            let page = rpc.command("get_messages_page", json!({"limit":1})).await?;
            event(json!({"type":"session_restore_inspected","restored":restored,"messageCount":page.get("totalMessages").cloned().unwrap_or_else(||json!(page["messages"].as_array().map(Vec::len)))}));
            if restored["cancelled"] != false {
                return Err(conflict("Session restore was not confirmed."));
            }
            if restored["resumed"] == true
                && restored["sessionId"] == config["previousSessionId"]
                && page["messages"].as_array().is_some_and(|a| !a.is_empty())
            {
                recovery = Some("session");
                session_directory = PathBuf::from(previous);
            } else {
                // Rebind to this attempt's empty directory before a fresh session.
                let fresh = rpc
                    .command("open_session", json!({"sessionDir":sessions}))
                    .await?;
                if fresh["cancelled"] != false || fresh["resumed"] != false {
                    return Err(conflict("Fresh replacement session was not confirmed."));
                }
            }
        }
        let mut identity = configure_assignment(&rpc, &config["model"]).await?;
        let state = rpc.command("get_state", json!({})).await?;
        canonical(&session_directory)?;
        let file = Path::new(text(&state, "sessionFile")?);
        if !file.is_absolute()
            || file.parent() != Some(session_directory.as_path())
            || file.is_symlink()
        {
            return Err(conflict(
                "Runner session directory does not match its receipt.",
            ));
        }
        let registered = rpc
            .command("set_host_tools", json!({"tools":tools}))
            .await?;
        let expected: Vec<_> = tools
            .iter()
            .map(|t| t["name"].clone())
            .collect();
        if registered["toolNames"] != json!(expected) {
            return Err(conflict("Runner host tool registration did not match."));
        }
        identity["hostPid"] = json!(std::process::id());
        identity["ompPid"] = json!(rpc.pid);
        identity["sessionDirectory"] = json!(session_directory);
        if let Some(mode) = recovery {
            identity["recoveryMode"] = json!(mode);
        }
            Ok(identity)
        }.await;
    match result {
        Ok(identity) => Ok((Runner::Omp(rpc), identity)),
        Err(error) => {
            rpc.close().await?;
            Err(error)
        }
    }
}
