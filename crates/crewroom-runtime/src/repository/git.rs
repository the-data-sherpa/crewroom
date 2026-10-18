//! Fixed Git execution policy and durable ownership of every mutating child.
mod lifecycle;
mod records;
pub(super) use records::Records;
mod shutdown;
use crate::{CheckedProcess, CheckedSpec, CommandSpec, canonical, conflict, durable_new};
use crewroom_core::{Error, Result, authority::Authority};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const LIMIT: usize = 16 * 1024 * 1024;
#[derive(Clone)]
pub(super) struct Git {
    pub authority: Arc<Authority>,
    executable: PathBuf,
    helper: PathBuf,
    jobs: PathBuf,
    operations: lifecycle::Operations,
    cleanup: crate::supervisor::tasks::RunTasks,
}
#[derive(Debug)]
pub(super) struct Failure {
    pub code: Option<i32>,
    pub message: String,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
impl From<Failure> for Error {
    fn from(v: Failure) -> Self {
        conflict(&v.message)
    }
}
/// A numeric exit was observed from the exact live child, followed by stopped
/// ownership. Signal, timeout, lost parent, or incomplete capture is not an exit
/// proof and remains an error. Recovery still verifies the actual Git effect.
pub(super) struct MutationOutcome {
    pub code: i32,
    pub stdout: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Intent {
    pub descriptor: CheckedProcess,
    pub spec: CheckedSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion: Option<String>,
    #[serde(skip)]
    ticket: Option<lifecycle::Ticket>,
}
fn environment() -> BTreeMap<String, String> {
    [
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("HOME", std::env::var("HOME").unwrap_or_default()),
        ("LANG", "C".into()),
        ("GIT_TERMINAL_PROMPT", "0".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
fn executable(name: &str) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    for folder in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|p| p.is_absolute())
    {
        let candidate = folder.join(name);
        if std::fs::metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Ok(std::fs::canonicalize(candidate)?);
        }
    }
    Err(conflict("Git executable is unavailable."))
}
impl Git {
    pub fn new(root: &Path, helper: PathBuf, authority: Arc<Authority>) -> Result<Self> {
        canonical(root)?;
        canonical(&helper)?;
        authority.assert_current()?;
        let jobs = root.join(".git-operations");
        crate::create_root(&jobs)?;
        Ok(Self {
            authority,
            executable: executable("git")?,
            helper,
            jobs,
            operations: lifecycle::Operations::default(),
            cleanup: crate::supervisor::tasks::RunTasks::default(),
        })
    }
    async fn plain(&self, cwd: &Path, args: &[String]) -> std::result::Result<Vec<u8>, Failure> {
        let failure = |message: &str| Failure {
            code: None,
            message: message.into(),
        };
        let mut child = Command::new(&self.executable)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(environment())
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| failure("Git could not start."))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let result = timeout(Duration::from_secs(30), async {
            let read = async |stream: tokio::process::ChildStdout| {
                let mut bytes = Vec::new();
                stream
                    .take((LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() > LIMIT {
                    return Err(std::io::Error::other("Git output exceeded its limit"));
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let err = async {
                let mut bytes = Vec::new();
                stderr
                    .take((LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() > LIMIT {
                    return Err(std::io::Error::other("Git output exceeded its limit"));
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let (out, err) = tokio::try_join!(read(stdout), err)?;
            if out.len() > LIMIT || err.len() > LIMIT {
                child.kill().await?;
                return Err(std::io::Error::other("Git output exceeded its limit"));
            }
            Ok::<_, std::io::Error>((child.wait().await?, out))
        })
        .await;
        match result {
            Ok(Ok((status, bytes))) if status.success() => Ok(bytes),
            Ok(Ok((status, _))) => Err(Failure {
                code: status.code(),
                message: format!(
                    "Git could not complete {}. Retained work needs inspection.",
                    args.last().map(String::as_str).unwrap_or("operation")
                ),
            }),
            _ => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(failure(
                    "Git was interrupted or exceeded its bounded output/deadline; retained work needs inspection.",
                ))
            }
        }
    }
    async fn options(&self, cwd: &Path) -> Result<Vec<String>> {
        let filters = match self
            .plain(
                cwd,
                &[
                    "config",
                    "--includes",
                    "--name-only",
                    "--get-regexp",
                    "^filter\\.",
                ]
                .map(String::from),
            )
            .await
        {
            Ok(v) => String::from_utf8(v)
                .map_err(|_| conflict("Repository configuration is not valid UTF-8."))?,
            Err(e) if e.code == Some(1) => String::new(),
            Err(_) => return Err(conflict("Repository configuration is unavailable.")),
        };
        let mut options = vec![
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let mut seen = std::collections::HashSet::new();
        for filter in filters
            .lines()
            .filter_map(|line| line.rsplit_once('.').map(|(prefix, _)| prefix))
        {
            if seen.insert(filter) {
                for operation in ["clean", "smudge", "process"] {
                    options.extend(["-c".into(), format!("{filter}.{operation}=")]);
                }
                options.extend(["-c".into(), format!("{filter}.required=false")]);
            }
        }
        Ok(options)
    }
    pub async fn raw(&self, cwd: &Path, args: &[&str]) -> std::result::Result<String, Failure> {
        let mut options = self.options(cwd).await.map_err(|e| Failure {
            code: None,
            message: e.to_string(),
        })?;
        options.extend(args.iter().map(|v| (*v).into()));
        Ok(String::from_utf8_lossy(&self.plain(cwd, &options).await?).into_owned())
    }
    pub async fn metadata(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut options = self.options(cwd).await?;
        options.extend(args.iter().map(|v| (*v).into()));
        String::from_utf8(self.plain(cwd, &options).await?)
            .map_err(|_| conflict("Repository metadata requires UTF-8 paths."))
    }
    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        Ok(self.raw(cwd, args).await?.trim_end().into())
    }
    pub async fn reference(&self, cwd: &Path, name: &str) -> Result<String> {
        self.run(
            cwd,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{name}^{{commit}}"),
            ],
        )
        .await
    }
    fn run_root(&self, run: &str) -> Result<PathBuf> {
        if uuid::Uuid::parse_str(run).is_err() {
            return Err(conflict("Invalid repository run identity."));
        }
        let path = self.jobs.join(run);
        crate::create_root(&path)?;
        Ok(path)
    }
    /// Seal admitted-but-unlaunched operations as well as observing prior children.
    /// A stopped child proves only quiescence; callers must reconcile actual Git state.
    pub async fn reconcile(&self, run: &str) -> Result<()> {
        let root = self.run_root(run)?;
        let mut records = Records::default();
        self.read_run(run, &root, &mut records)?;
        if let Some((_, error)) = records.errors.first() {
            return Err(conflict(error));
        }
        if records
            .roots
            .iter()
            .any(|(_, root)| !records.saved.iter().any(|(_, saved)| &saved.root == root))
        {
            return Err(conflict(
                "Git process directory has no verified ownership record; retain repository ownership.",
            ));
        }
        for (_, descriptor) in records.saved {
            if crate::seal_checked(&descriptor).await.state != "stopped" {
                return Err(conflict(
                    "Prior Git operation is live or uncertain; retain repository ownership.",
                ));
            }
        }
        self.authority.assert_current()
    }
    pub async fn mutate(
        &self,
        run: &str,
        cwd: &Path,
        args: &[&str],
        dispatch: impl FnOnce(
            &mut dyn FnMut() -> Result<tokio::process::Child>,
        ) -> Result<tokio::process::Child>,
    ) -> Result<String> {
        let outcome = self.mutate_outcome(run, cwd, args, dispatch).await?;
        if outcome.code != 0 {
            return Err(conflict(
                "Git mutation failed; retained work needs inspection.",
            ));
        }
        Ok(outcome.stdout)
    }
    pub async fn mutate_outcome(
        &self,
        run: &str,
        cwd: &Path,
        args: &[&str],
        dispatch: impl FnOnce(
            &mut dyn FnMut() -> Result<tokio::process::Child>,
        ) -> Result<tokio::process::Child>,
    ) -> Result<MutationOutcome> {
        let intent = self.describe_mutation(run, cwd, args).await?;
        self.execute_mutation(run, intent, dispatch).await
    }
    pub async fn describe_mutation(&self, run: &str, cwd: &Path, args: &[&str]) -> Result<Intent> {
        let ticket = self.operations.begin(run, self.authority.clone())?;
        let mut options = self.options(cwd).await?;
        options.extend(args.iter().map(|v| (*v).into()));
        let parent = self.run_root(run)?;
        let token = uuid::Uuid::new_v4().to_string();
        let spec = CheckedSpec {
            root: parent.join(&token),
            token: token.clone(),
            command: CommandSpec {
                command: self.executable.to_string_lossy().into_owned(),
                args: options,
                cwd: cwd.into(),
                env: environment(),
            },
            epoch: self.authority.epoch().into(),
            controller_root: self.authority.root().into(),
        };
        let descriptor = crate::describe_checked(&spec)?;
        Ok(Intent {
            descriptor,
            spec,
            promotion: None,
            ticket: Some(ticket),
        })
    }
    pub async fn describe_promotion(
        &self,
        run: &str,
        cwd: &Path,
        operation: &str,
        head: &str,
    ) -> Result<Intent> {
        if !crewroom_core::store::valid_uuid(operation) {
            return Err(conflict("Invalid promotion identity."));
        }
        let mut intent = self
            .describe_mutation(
                run,
                cwd,
                &["reset", "--keep", "--no-recurse-submodules", head],
            )
            .await?;
        let parent = self.jobs.parent().unwrap().join("promotion-processes");
        crate::create_root(&parent)?;
        intent.spec.root = parent.join(format!("{operation}-{}", intent.spec.token));
        intent.descriptor = crate::describe_checked(&intent.spec)?;
        intent.promotion = Some(operation.into());
        Ok(intent)
    }
    pub fn promotion_record(intent: &Intent) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(&intent.descriptor)?;
        value.as_object_mut().unwrap().extend(
            serde_json::to_value(&intent.spec.command)?
                .as_object()
                .unwrap()
                .clone(),
        );
        Ok(value)
    }
    pub fn saved_promotion(
        &self,
        run: &str,
        operation: &str,
        saved: &serde_json::Value,
        cwd: &Path,
        head: &str,
    ) -> Result<Intent> {
        let descriptor: CheckedProcess = serde_json::from_value(saved.clone())?;
        let command: CommandSpec = serde_json::from_value(saved.clone())?;
        let args = &command.args;
        let suffix = ["reset", "--keep", "--no-recurse-submodules", head];
        let prefix = [
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
        ];
        if command.cwd != cwd
            || args.len() < 10
            || args[..6] != prefix
            || args[args.len() - 4..] != suffix
            || !(args.len() - 10).is_multiple_of(2)
            || args[6..args.len() - 4].chunks_exact(2).any(|p| {
                p[0] != "-c"
                    || !p[1].starts_with("filter.")
                    || ![".clean=", ".smudge=", ".process=", ".required=false"]
                        .iter()
                        .any(|end| {
                            p[1].strip_suffix(end)
                                .is_some_and(|name| name.len() > 7 && !name.contains('='))
                        })
            })
        {
            return Err(conflict(
                "Checked Git promotion command changed; retain the recorded intent.",
            ));
        }
        let spec = CheckedSpec {
            root: descriptor.root.clone(),
            token: descriptor.token.clone(),
            command,
            epoch: descriptor.epoch.clone(),
            controller_root: descriptor.controller_root.clone(),
        };
        let intent = Intent {
            descriptor,
            spec,
            promotion: Some(operation.into()),
            ticket: None,
        };
        self.intent_identity(run, &intent)?;
        Ok(intent)
    }
    fn intent_identity(&self, run: &str, intent: &Intent) -> Result<()> {
        let root = self.run_root(run)?;
        let expected = if let Some(id) = &intent.promotion {
            if !crewroom_core::store::valid_uuid(id) {
                return Err(conflict("Invalid retained promotion identity."));
            }
            self.jobs
                .parent()
                .unwrap()
                .join("promotion-processes")
                .join(format!("{id}-{}", intent.descriptor.token))
        } else {
            root.join(&intent.descriptor.token)
        };
        if intent.descriptor.root != expected
            || intent.descriptor.controller_root != self.authority.root()
            || crate::spec::describe_retained_checked(&intent.spec)? != intent.descriptor
        {
            return Err(conflict("Retained Git operation identity changed."));
        }
        Ok(())
    }
    /// The caller first persists any domain-specific intent, then finishes its
    /// asynchronous source/target checks. This method does not yield before the
    /// guarded durable manifest and actual OS spawn.
    pub async fn execute_mutation(
        &self,
        run: &str,
        intent: Intent,
        dispatch: impl FnOnce(
            &mut dyn FnMut() -> Result<tokio::process::Child>,
        ) -> Result<tokio::process::Child>,
    ) -> Result<MutationOutcome> {
        self.intent_identity(run, &intent)?;
        let ticket = intent.ticket.clone().ok_or_else(|| {
            conflict("Saved Git intent requires a new authorized operation before dispatch.")
        })?;
        let parent = self.run_root(run)?;
        let descriptor = intent.descriptor.clone();
        let spec = &intent.spec;
        let token = &descriptor.token;
        let mut launch = || {
            self.authority.assert_current()?;
            durable_new(
                &parent.join(format!("{token}.json")),
                &serde_json::to_vec(&intent)?,
            )?;
            crate::prepare_checked(spec)?;
            crate::validate_launch(&descriptor, self.authority.epoch(), self.authority.root())?;
            Ok(Command::new(&self.helper)
                .arg("checked-exec-report")
                .arg(&descriptor.root)
                .arg(&descriptor.token)
                .arg(&descriptor.spec_sha256)
                .arg(&descriptor.epoch)
                .arg(&descriptor.controller_root)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?)
        };
        // The caller keeps its workflow mutex through validation, durable intent
        // and dispatch. Pause/revocation cannot slip between these operations.
        let mut child = ticket.dispatch(&descriptor, || dispatch(&mut launch))?;
        let report = child
            .stdout
            .take()
            .ok_or_else(|| conflict("Checked Git outcome channel is unavailable."))?;
        // Cancellation cannot abandon an untracked mutable child. The descriptor
        // precedes dispatch, and this worker settles or stops the exact owned group.
        tokio::spawn(async move {
            let _ticket = ticket;
            let status = match timeout(Duration::from_secs(30), child.wait()).await {
                Ok(result) => result?,
                Err(_) => {
                    let state = crate::stop_owned(&descriptor.root).await;
                    let _ = timeout(Duration::from_secs(5), child.wait()).await;
                    return Err(conflict(if state.state == "stopped" {
                        "Git timed out; inspect retained work before retrying."
                    } else {
                        "Git timed out with unresolved ownership."
                    }));
                }
            };
            if crate::inspect_checked(&descriptor).await.state != "stopped" {
                return Err(conflict("Git operation ownership remains uncertain."));
            }
            let code = status.code().ok_or_else(|| {
                conflict("Git mutation was interrupted without a numeric exit; retain its intent.")
            })?;
            let mut bytes = Vec::new();
            timeout(
                Duration::from_secs(5),
                report.take(257).read_to_end(&mut bytes),
            )
            .await
            .map_err(|_| conflict("Live Git outcome channel did not settle."))??;
            if bytes.len() > 256
                || serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                    != Some(serde_json::json!({"code":code,"signal":null}))
            {
                return Err(conflict(
                    "Live Git outcome was not confirmed; retain its mutation intent.",
                ));
            }
            let capture: serde_json::Value = serde_json::from_slice(
                &super::files::regular_bytes(&descriptor.root, "capture.json", 64 * 1024)?.0,
            )?;
            if capture["stdout"]["truncated"] != false || capture["stderr"]["truncated"] != false {
                return Err(conflict(
                    "Git mutation output exceeded its capture limit; reconcile actual Git state.",
                ));
            }
            let stdout = String::from_utf8(
                super::files::regular_bytes(&descriptor.root, "stdout.log", 128 * 1024)?.0,
            )
            .map_err(|_| conflict("Git mutation output is not valid UTF-8."))?;
            Ok(MutationOutcome { code, stdout })
        })
        .await
        .map_err(|_| conflict("Git operation worker was interrupted; retain its durable intent."))?
    }
}
