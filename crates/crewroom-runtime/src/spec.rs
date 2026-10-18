use crate::{canonical, conflict, durable_new, read};
use crewroom_core::{Result, serialized::stringify};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckedProcess {
    pub version: u8,
    pub root: PathBuf,
    pub token: String,
    pub spec_sha256: String,
    pub epoch: String,
    pub controller_root: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedSpec {
    pub root: PathBuf,
    pub token: String,
    #[serde(flatten)]
    pub command: CommandSpec,
    pub epoch: String,
    pub controller_root: PathBuf,
}
pub(crate) fn token_valid(token: &str) -> bool {
    token.len() == 36
        && token.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
impl CheckedProcess {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.version != 1
            || !token_valid(&self.token)
            || self.spec_sha256.len() != 64
            || !self.spec_sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(conflict("Invalid checked descriptor."));
        }
        Ok(())
    }
    pub(crate) fn manifest(&self) -> Result<serde_json::Value> {
        self.validate()?;
        canonical(&self.root)?;
        let raw = read(&self.root.join("launch.json"))?;
        let value: serde_json::Value = serde_json::from_slice(&raw)?;
        if value["version"] != 1
            || value["token"] != self.token
            || value["epoch"] != self.epoch
            || value["controllerRoot"] != self.controller_root.to_str().unwrap_or("")
            || format!("{:x}", Sha256::digest(&raw)) != self.spec_sha256
        {
            return Err(conflict(
                "Checked process manifest does not match its descriptor.",
            ));
        }
        Ok(value)
    }
}
fn checked_bytes(spec: &CheckedSpec) -> Result<Vec<u8>> {
    let command = &spec.command;
    if !token_valid(&spec.token) || !Path::new(&command.command).is_absolute() {
        return Err(conflict("Invalid checked command identity."));
    }
    for path in [&command.cwd, &spec.controller_root, &spec.root] {
        if !path.is_absolute()
            || path.components().any(|p| {
                matches!(
                    p,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
        {
            return Err(conflict("Invalid retained checked path identity."));
        }
    }
    const ALLOWED: &[&str] = &[
        "PATH",
        "HOME",
        "LANG",
        "SSH_AUTH_SOCK",
        "GIT_TERMINAL_PROMPT",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_NOSYSTEM",
        "GIT_SSH_COMMAND",
    ];
    if command.env.keys().any(|k| !ALLOWED.contains(&k.as_str())) {
        return Err(conflict("Checked command environment is not allowlisted."));
    }
    Ok(stringify(&serde_json::json!({"version":1,"token":spec.token,"command":command.command,"args":command.args,"cwd":command.cwd,"env":command.env,"epoch":spec.epoch,"controllerRoot":spec.controller_root})).into_bytes())
}
/// Derive immutable historical identity without requiring an old command's
/// working directory to survive cleanup. This never admits a launch.
pub(crate) fn describe_retained_checked(spec: &CheckedSpec) -> Result<CheckedProcess> {
    let raw = checked_bytes(spec)?;
    Ok(CheckedProcess {
        version: 1,
        root: spec.root.clone(),
        token: spec.token.clone(),
        spec_sha256: format!("{:x}", Sha256::digest(raw)),
        epoch: spec.epoch.clone(),
        controller_root: spec.controller_root.clone(),
    })
}
pub fn describe_checked(spec: &CheckedSpec) -> Result<CheckedProcess> {
    canonical(&spec.command.cwd)?;
    canonical(&spec.controller_root)?;
    canonical(
        spec.root
            .parent()
            .ok_or_else(|| conflict("Missing process parent."))?,
    )?;
    describe_retained_checked(spec)
}
pub fn validate_launch(descriptor: &CheckedProcess, epoch: &str, root: &Path) -> Result<()> {
    let _guard = crate::helper::DispatchGuard::acquire(descriptor)?;
    descriptor.manifest()?;
    if descriptor.epoch != epoch || descriptor.controller_root != root {
        return Err(conflict("Stale controller epoch."));
    }
    if descriptor.root.join("owner.json").try_exists()?
        || descriptor.root.join("seal.json").try_exists()?
    {
        return Err(conflict("Checked process was already launched or sealed."));
    }
    Ok(())
}
/// Caller persists the descriptor before preparation. This never repairs or overwrites a partial root.
pub fn prepare_checked(spec: &CheckedSpec) -> Result<CheckedProcess> {
    let descriptor = describe_checked(spec)?;
    let _guard = crate::helper::DispatchGuard::acquire(&descriptor)?;
    if !descriptor.root.try_exists()? {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&descriptor.root)?;
        std::fs::File::open(descriptor.root.parent().unwrap())?.sync_all()?;
        durable_new(&descriptor.root.join("launch.json"), &checked_bytes(spec)?)?;
    }
    descriptor.manifest()?;
    Ok(descriptor)
}
/// Plain ownership is for trusted hosts; it does not contain untrusted command descendants.
pub fn prepare_owned(root: &Path, spec: &CommandSpec) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)?;
    let value = serde_json::json!({"token":uuid::Uuid::new_v4().to_string(),"command":spec.command,"args":spec.args,"cwd":spec.cwd,"env":spec.env});
    durable_new(&root.join("launch.json"), stringify(&value).as_bytes())
}
