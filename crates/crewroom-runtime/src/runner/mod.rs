//! Runner protocol and session controls. Workflow admission, tool effects and
//! stopped-ownership authority remain in the host and supervisor.
mod auth;
mod claude;
mod claude_history;
mod claude_profile;
mod codex;
mod journal;
mod jsonl;
mod omp;
mod profile;
use crate::{CommandSpec, canonical, conflict, durable_new};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

pub type Event = Arc<dyn Fn(Value) + Send + Sync>;

pub enum Runner {
    Omp(crate::rpc::OmpRpc),
    Codex(Arc<codex::Codex>),
    Claude(Arc<claude::Claude>),
}

/// `--version` outputs whose execution adapter passed live acceptance. A newer
/// installed CLI may list models before it is verified to run them.
pub fn execution_versions(adapter: &str) -> &'static [&'static str] {
    match adapter {
        "codex" => profile::VERSIONS,
        "claude-code" => claude_profile::VERSIONS,
        "omp" => &["omp/18.4.12"],
        _ => &[],
    }
}

/// An absent tag is the retained OMP format. Never infer a new adapter from a model name.
pub fn kind(model: &Value) -> Result<&str> {
    crewroom_core::runner_kind(model)
}
/// Read-only login identity for admission. Tokens and raw account IDs never
/// leave the runner module; this does not refresh credentials or prove inference.
pub(crate) fn account_binding(adapter: &str, config: &Value) -> Result<String> {
    match adapter {
        "codex" => Ok(profile::digest(
            auth::read_credentials(Path::new(text(config, "authFile")?))?
                .account
                .as_bytes(),
        )),
        "claude-code" => claude_profile::account(Path::new(text(config, "home")?)),
        _ => Err(conflict("Unknown native account profile.")),
    }
}
/// Native adapters refuse to launch a CLI whose `--version` is not listed in
/// `execution_versions`; OMP's version is observed through discovery.
pub(crate) fn pins_version(adapter: &str) -> bool {
    matches!(adapter, "codex" | "claude-code")
}
pub fn process_dir(model: &Value) -> Result<&'static str> {
    Ok(match kind(model)? {
        "codex" => "codex-process",
        "claude-code" => "claude-process",
        _ => "omp-process",
    })
}
pub fn pid_key(model: &Value) -> Result<&'static str> {
    Ok(if kind(model)? != "omp" {
        "runnerPid"
    } else {
        "ompPid"
    })
}

pub async fn open(
    helper: &Path,
    root: &Path,
    config: &Value,
    origin: &str,
    tools: &[Value],
    event: Event,
    stopping: &tokio_util::sync::CancellationToken,
) -> Result<(Runner, Value)> {
    match kind(&config["model"])? {
        "codex" => codex::open(helper, root, config, tools, event, stopping).await,
        "claude-code" => claude::open(helper, root, config, tools, event, stopping).await,
        _ => omp::open(helper, root, config, origin, tools, event, stopping).await,
    }
}

impl Runner {
    pub async fn state(&self) -> Result<Value> {
        match self {
            Self::Omp(rpc) => rpc.command("get_state", json!({})).await,
            Self::Codex(rpc) => rpc.state().await,
            Self::Claude(rpc) => rpc.state().await,
        }
    }
    pub async fn submit(&self, id: &str, message: &str, steer: bool) -> Result<()> {
        match self {
            Self::Omp(rpc) => {
                let mut args = json!({"message":message});
                if steer {
                    args["streamingBehavior"] = json!("steer");
                }
                rpc.command_with_id("prompt", args, id).await?;
            }
            Self::Codex(rpc) => rpc.submit(id, message, steer).await?,
            Self::Claude(rpc) => rpc.submit(id, message, steer).await?,
        }
        Ok(())
    }
    pub async fn resolve_tool(&self, result: &Value) -> Result<()> {
        match self {
            Self::Omp(rpc) => {
                let mut wire = result.clone();
                wire.as_object_mut().unwrap().remove("outcome");
                rpc.send(&wire).await
            }
            Self::Codex(rpc) => rpc.resolve_tool(result).await,
            Self::Claude(rpc) => rpc.resolve_tool(result).await,
        }
    }
    pub async fn interrupt(&self) -> Result<()> {
        match self {
            Self::Omp(rpc) => {
                rpc.command("abort", json!({})).await?;
            }
            Self::Codex(rpc) => rpc.interrupt().await?,
            Self::Claude(rpc) => rpc.interrupt().await?,
        }
        Ok(())
    }
    pub async fn close(&self) -> Result<()> {
        match self {
            Self::Omp(rpc) => rpc.close().await,
            Self::Codex(rpc) => rpc.close().await,
            Self::Claude(rpc) => rpc.close().await,
        }
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| conflict("Missing runner configuration field."))
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    canonical(path)?;
    if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
        return Err(conflict("Runner directory must be private."));
    }
    Ok(())
}
