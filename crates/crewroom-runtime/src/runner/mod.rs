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
pub(crate) use claude_profile::LAUNCH_FLAGS as CLAUDE_LAUNCH_FLAGS;
pub(crate) use profile::{launch_files as codex_launch_files, pinned_row as codex_pinned};

/// OMP RPC launch flags shared by every attempt and by the FOR-69
/// compatibility probe; cwd, session, model and prompt arguments follow them.
pub const OMP_RPC_FLAGS: &[&str] = &[
    "--mode",
    "rpc",
    "--no-ui",
    "--no-tools",
    "--no-extensions",
    "--no-rules",
    "--no-skills",
    "--no-lsp",
    "--no-title",
    "--no-pty",
];

pub enum Runner {
    Omp(crate::rpc::OmpRpc),
    Codex(Arc<codex::Codex>),
    Claude(Arc<claude::Claude>),
}

/// `--version` outputs whose execution adapter passed live acceptance. A newer
/// installed CLI may list models before it is verified to run them. OMP
/// 18.11.3 (FOR-52) passed the actual-binary synthetic-provider suites and one
/// live `verify_catalog_workflow.py` launch.
pub fn execution_versions(adapter: &str) -> &'static [&'static str] {
    match adapter {
        "codex" => profile::VERSIONS,
        "claude-code" => claude_profile::VERSIONS,
        "omp" => &["omp/18.4.12", "omp/18.11.3"],
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
/// FOR-69: a native adapter launches a listed `--version`, or an unlisted
/// newer one whose frozen selection carries a passed compatibility probe for
/// this exact version, canonical executable and content hash. Anything else
/// is refused before the harness starts.
pub(crate) fn admitted_version(
    harness: &crate::model_catalog::Harness,
    verified: &[&str],
    reported: &[u8],
    plan: &Value,
    binary: &Path,
    content: &[u8],
) -> Result<String> {
    // FOR-71: the bundled known-broken list refuses a release at the adapter
    // too, whether it is listed or probed.
    if let Some(entry) = std::str::from_utf8(reported)
        .ok()
        .and_then(|v| v.strip_suffix('\n'))
        .and_then(|v| crate::model_catalog::compatibility::broken(harness, v).map(|e| (v, e)))
    {
        return Err(conflict(&crate::model_catalog::known_broken::refusal(
            entry.1, entry.0,
        )));
    }
    if let Some(v) = verified
        .iter()
        .find(|v| reported == format!("{v}\n").as_bytes())
    {
        return Ok((*v).to_owned());
    }
    let refused = || {
        conflict(&format!(
            "{} requires a verified version ({}) or a passed compatibility probe for this exact executable. Refresh models, then re-admit the assignment.",
            harness.label(),
            verified.join(", ")
        ))
    };
    let reported = std::str::from_utf8(reported)
        .ok()
        .and_then(|v| v.strip_suffix('\n'))
        .ok_or_else(refused)?;
    let identity: crate::model_catalog::discovery::Identity =
        serde_json::from_value(plan["selection"]["identity"].clone()).map_err(|_| refused())?;
    let canonical = binary.canonicalize().map_err(|_| refused())?;
    let digest = profile::digest(content);
    let probed = identity.version == reported
        && identity.canonical_binary == canonical
        && identity.binary_sha256 == digest
        && crate::model_catalog::compatibility::launchable(harness, &identity);
    probed.then(|| reported.to_owned()).ok_or_else(refused)
}
/// Native adapters refuse to launch a CLI whose `--version` is neither listed
/// in `execution_versions` nor probed (`admitted_version`); OMP's version is
/// observed through discovery.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::{
        Harness,
        compatibility::{Basis, Compatibility, ProbeReceipt},
        discovery::Identity,
    };

    /// FOR-69: launch accepts a listed version, or the exact probed version
    /// and executable the frozen selection recorded; nothing else.
    #[test]
    fn launch_admits_listed_or_exactly_probed_versions() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("claude");
        std::fs::write(&binary, b"fixture executable").unwrap();
        let content = std::fs::read(&binary).unwrap();
        let canonical = binary.canonicalize().unwrap();
        let sha = profile::digest(&content);
        let listed = claude_profile::VERSIONS;
        let harness = Harness::ClaudeCode;
        let plan = |version: &str, probe: Option<ProbeReceipt>| {
            json!({"selection":{"identity":Identity {
                binary: binary.clone(),
                canonical_binary: canonical.clone(),
                binary_sha256: sha.clone(),
                binary_stamp: "stamp".into(),
                version: version.into(),
                profile: "fixture".into(),
                compatibility: Some(Compatibility {
                    basis: if probe.is_some() { Basis::Probed } else { Basis::Verified },
                    probe,
                }),
            }}})
        };
        let admit = |reported: &[u8], plan: &Value, content: &[u8]| {
            admitted_version(&harness, listed, reported, plan, &binary, content)
        };
        assert_eq!(
            admit(b"2.1.302 (Claude Code)\n", &json!({}), &content).unwrap(),
            "2.1.302 (Claude Code)"
        );
        let newer = "3.0.0 (Claude Code)";
        let receipt = ProbeReceipt::new(&harness, newer, &canonical, &sha, 1);
        assert_eq!(
            admit(
                b"3.0.0 (Claude Code)\n",
                &plan(newer, Some(receipt.clone())),
                &content
            )
            .unwrap(),
            newer
        );
        // No receipt, another version, changed content or another harness's
        // receipt: refused before the harness starts.
        let other = ProbeReceipt::new(&Harness::Codex, newer, &canonical, &sha, 1);
        for (reported, plan, content) in [
            (
                &b"3.0.0 (Claude Code)\n"[..],
                plan(newer, None),
                &content[..],
            ),
            (&b"3.0.0 (Claude Code)\n"[..], json!({}), &content[..]),
            (
                &b"3.0.1 (Claude Code)\n"[..],
                plan(newer, Some(receipt.clone())),
                &content[..],
            ),
            (
                &b"3.0.0 (Claude Code)\n"[..],
                plan(newer, Some(receipt.clone())),
                &b"rebuilt"[..],
            ),
            (
                &b"3.0.0 (Claude Code)\n"[..],
                plan(newer, Some(other)),
                &content[..],
            ),
        ] {
            let error = admit(reported, &plan, content).unwrap_err().to_string();
            assert!(error.contains("passed compatibility probe"), "{error}");
            assert!(error.contains("2.1.302 (Claude Code)"), "{error}");
        }
    }
}
