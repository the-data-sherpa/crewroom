//! Bounded metadata-only exchanges. Each child retains the existing process
//! ownership receipt; shutdown and restart can verify cleanup independently.
use super::*;
use crate::{CommandSpec, canonical, durable_new, prepare_owned, stop_owned};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODELS: usize = 10_000;
const MAX_PAGES: usize = 100;

/// Private configuration: never serialized to the workspace or diagnostic.
#[derive(Clone)]
pub struct Config {
    pub harness: Harness,
    pub binary: PathBuf,
    pub helper: PathBuf,
    pub path: String,
    pub omp_models: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub binary: PathBuf,
    pub canonical_binary: PathBuf,
    pub binary_sha256: String,
    pub binary_stamp: String,
    pub version: String,
    pub profile: String,
}

/// `--version` outputs whose metadata exchange was executed against the real
/// CLI. Discovery only lists models; execution keeps its own pinned profile.
pub fn verified_versions(harness: &Harness) -> &'static [&'static str] {
    match harness {
        // FOR-9 verified the first entries; FOR-38 the installed Linux CLIs.
        Harness::Codex => &["codex-cli 0.162.0", "codex-cli 0.163.0"],
        Harness::ClaudeCode => &["2.1.296 (Claude Code)", "2.1.302 (Claude Code)"],
        Harness::Omp => &["omp/18.4.12", "omp/18.11.3"],
    }
}
/// Operator-facing drift explanation. The reported version is bounded,
/// printable harness output, never configuration.
pub fn version_message(harness: &Harness, reported: &str) -> String {
    let reported: String = reported
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    format!(
        "{} reported version \"{}\", which Crewroom has not verified for model discovery (verified: {}). Install a verified version or update Crewroom, then refresh.",
        harness.label(),
        if reported.is_empty() {
            "unknown"
        } else {
            &reported
        },
        verified_versions(harness).join(", ")
    )
}

/// Cheap freshness observation for UI reads. Admission also compares SHA-256.
pub fn binary_stamp(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let real = path
        .canonicalize()
        .map_err(|_| conflict("Harness executable is unavailable."))?;
    let m = std::fs::metadata(&real)?;
    Ok(format!(
        "{}:{}:{}:{}:{}:{}",
        real.display(),
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec()
    ))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub identity: Identity,
    pub offerings: Vec<Offering>,
    pub inference_requested: bool,
    pub cleanup_verified: bool,
}

/// Hashing a large CLI blocks for seconds in debug builds. Keep it off async
/// workers so service reads stay responsive while discovery runs.
async fn binary_digest_blocking(path: &Path) -> Result<(PathBuf, String)> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || binary_digest(&path))
        .await
        .map_err(|_| conflict("Harness executable hashing did not finish."))?
}
pub fn binary_digest(path: &Path) -> Result<(PathBuf, String)> {
    if !path.is_absolute() {
        return Err(conflict("Discovery requires an absolute executable path."));
    }
    let real = path
        .canonicalize()
        .map_err(|_| conflict("Harness executable is unavailable."))?;
    let mut file =
        std::fs::File::open(&real).map_err(|_| conflict("Harness executable is unavailable."))?;
    if !file.metadata()?.is_file() {
        return Err(conflict("Harness executable is not a regular file."));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok((real, format!("{:x}", digest.finalize())))
}

fn private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    canonical(path)
}

struct Process {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    root: PathBuf,
    buffer: Vec<u8>,
    total: usize,
}
impl Process {
    async fn start(
        config: &Config,
        cwd: &Path,
        name: &str,
        args: Vec<String>,
        env: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let root = cwd.join(name);
        let spec = CommandSpec {
            command: config.binary.to_string_lossy().into(),
            args,
            cwd: cwd.to_path_buf(),
            env: env.clone(),
        };
        prepare_owned(&root, &spec)?;
        let mut child = Command::new(&config.helper)
            .arg("exec")
            .arg(&root)
            .current_dir(cwd)
            .env_clear()
            .env("LANG", "C")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| conflict("Could not start discovery process."))?;
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        Ok(Self {
            child,
            input,
            output,
            root,
            buffer: Vec::new(),
            total: 0,
        })
    }
    async fn send(&mut self, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        self.input
            .as_mut()
            .ok_or_else(|| conflict("Discovery input is closed."))?
            .write_all(&bytes)
            .await
            .map_err(|_| conflict("Harness closed discovery input."))
    }
    async fn chunk(&mut self) -> Result<bool> {
        let mut bytes = [0u8; 8192];
        let n = self
            .output
            .read(&mut bytes)
            .await
            .map_err(|_| conflict("Discovery output is unavailable."))?;
        self.total += n;
        if self.total > MAX_BYTES {
            return Err(conflict("Discovery exceeded its output limit."));
        }
        self.buffer.extend_from_slice(&bytes[..n]);
        Ok(n != 0)
    }
    async fn receive(&mut self, matches: impl Fn(&Value) -> bool) -> Result<Value> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.buffer.drain(..=end).collect();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let frame: Value = serde_json::from_slice(&line)
                    .map_err(|_| conflict("Invalid discovery JSON."))?;
                if !frame.is_object() {
                    return Err(conflict("Invalid discovery frame."));
                }
                if matches(&frame) {
                    return Ok(frame);
                }
            } else if !self.chunk().await? {
                return Err(conflict("Harness exited before discovery response."));
            }
        }
    }
    async fn collect(&mut self) -> Result<Vec<u8>> {
        self.input.take();
        while self.chunk().await? {}
        // Do not reap yet: the reserved leader PID keeps group cleanup safe.
        loop {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            // SAFETY: info is writable; WNOWAIT retains our child's identity.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id().unwrap(),
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                return Err(conflict("Discovery exit could not be observed."));
            }
            let info = unsafe { info.assume_init() };
            if unsafe { info.si_pid() } != 0 {
                if info.si_code != libc::CLD_EXITED || unsafe { info.si_status() } != 0 {
                    return Err(conflict("Harness rejected the discovery command."));
                }
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(std::mem::take(&mut self.buffer))
    }
    async fn close(&mut self) -> Result<()> {
        self.input.take();
        // No wait/try_wait has reaped this child. Its PID still identifies our
        // process group, even when it exited before publishing an owner receipt.
        if let Some(pid) = self.child.id() {
            // SAFETY: a positive, unreaped child PID is the group created at spawn.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        timeout(Duration::from_secs(5), self.child.wait())
            .await
            .map_err(|_| conflict("Discovery cleanup timed out."))?
            .map_err(|_| conflict("Discovery exit could not be verified."))?;
        let ownership = stop_owned(&self.root).await;
        if ownership.state != "stopped" {
            return Err(conflict("Discovery process ownership needs inspection."));
        }
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            // SAFETY: close has not reaped this group leader. Covers cancellation
            // of the caller itself; Child's kill_on_drop arranges eventual reaping.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

async fn bounded<T>(
    process: &mut Process,
    cancel: &CancellationToken,
    seconds: Duration,
    f: impl for<'a> FnOnce(
        &'a mut Process,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>,
    >,
) -> Result<T> {
    tokio::select! {
        _=cancel.cancelled()=>Err(conflict("Discovery was cancelled.")),
        result=timeout(seconds,f(process))=>result.unwrap_or_else(|_| Err(conflict("Model discovery timed out."))),
    }
}

fn text(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .filter(|s| valid_text(s))
        .map(String::from)
        .ok_or_else(|| conflict("Invalid model metadata."))
}
fn strings(value: &Value) -> Result<Option<Vec<String>>> {
    if value.is_null() {
        return Ok(None);
    }
    let rows = value
        .as_array()
        .filter(|v| v.len() <= 100)
        .ok_or_else(|| conflict("Invalid model capabilities."))?;
    let mut result = Vec::new();
    for v in rows {
        let s = v
            .as_str()
            .filter(|s| valid_text(s))
            .ok_or_else(|| conflict("Invalid model capabilities."))?
            .to_owned();
        if !result.contains(&s) {
            result.push(s);
        }
    }
    Ok(Some(result))
}
fn rows(value: &Value) -> Result<&Vec<Value>> {
    value
        .as_array()
        .filter(|v| v.len() <= MAX_MODELS && v.iter().all(Value::is_object))
        .ok_or_else(|| conflict("Invalid discovery model list."))
}

pub fn normalize(harness: &Harness, values: &Value, observed_at: u64) -> Result<Vec<Offering>> {
    let mut offerings = Vec::new();
    for row in rows(values)? {
        let (provider, selector, label, observed, efforts, inputs) = match harness {
            Harness::Codex => {
                if row["hidden"] == true {
                    continue;
                }
                let efforts = if row["supportedReasoningEfforts"].is_null() {
                    None
                } else {
                    let levels = rows(&row["supportedReasoningEfforts"])?;
                    Some(
                        levels
                            .iter()
                            .map(|v| text(v, "reasoningEffort"))
                            .collect::<Result<Vec<_>>>()?,
                    )
                };
                (
                    "codex".into(),
                    text(row, "model")?,
                    text(row, "displayName")?,
                    None,
                    efforts,
                    strings(&row["inputModalities"])?,
                )
            }
            Harness::ClaudeCode => (
                "claude-code".into(),
                text(row, "value")?,
                text(row, "displayName")?,
                row.get("resolvedModel")
                    .filter(|v| !v.is_null())
                    .map(|_| text(row, "resolvedModel"))
                    .transpose()?,
                strings(&row["supportedEffortLevels"])?,
                None,
            ),
            Harness::Omp => {
                let provider = text(row, "provider")?;
                let id = text(row, "id")?;
                if row["selector"] != format!("{provider}/{id}") {
                    return Err(conflict("Inconsistent OMP discovery identity."));
                }
                (
                    provider,
                    id.clone(),
                    text(row, "name")?,
                    Some(id),
                    strings(&row["thinking"])?,
                    strings(&row["input"])?,
                )
            }
        };
        offerings.push(Offering {
            key: offering_key(harness, &provider, &selector),
            harness: harness.clone(),
            provider,
            selector,
            display_name: label,
            api: None,
            observed_model: observed.map(|model| Observation {
                model,
                source: "discovery-metadata".into(),
                observed_at,
            }),
            capabilities: Capabilities {
                efforts,
                inputs,
                tools: None,
            },
            access: Access::Unknown,
            execution_profile: None,
            inference: None,
            billing: None,
        });
    }
    validate_contract(&offerings, &[])?;
    Ok(offerings)
}

async fn request(process: &mut Process, id: u64, method: &str, params: Value) -> Result<Value> {
    process
        .send(&json!({"id":id,"method":method,"params":params}))
        .await?;
    let frame = process.receive(|v| v["id"] == id).await?;
    if frame.get("error").is_some() || !frame["result"].is_object() {
        return Err(conflict("Harness rejected discovery request."));
    }
    Ok(frame["result"].clone())
}
async fn exchange(
    process: &mut Process,
    harness: &Harness,
    observed_at: u64,
) -> Result<Vec<Offering>> {
    match harness {
        Harness::Codex => {
            request(process,1,"initialize",json!({"clientInfo":{"name":"crewroom_model_discovery","title":"Crewroom model discovery","version":"0.1.0"}})).await?;
            process
                .send(&json!({"method":"initialized","params":{}}))
                .await?;
            let mut models = Vec::new();
            let mut cursors = HashSet::new();
            let mut cursor = Value::Null;
            for page in 0..MAX_PAGES {
                let result = request(
                    process,
                    page as u64 + 2,
                    "model/list",
                    json!({"limit":100,"includeHidden":false,"cursor":cursor}),
                )
                .await?;
                models.extend(rows(&result["data"])?.clone());
                if models.len() > MAX_MODELS {
                    return Err(conflict("Discovery exceeded its model limit."));
                }
                cursor = result["nextCursor"].clone();
                if cursor.is_null() {
                    return normalize(harness, &json!(models), observed_at);
                }
                if !cursor
                    .as_str()
                    .is_some_and(|s| valid_text(s) && cursors.insert(s.to_owned()))
                {
                    return Err(conflict("Invalid or repeated discovery cursor."));
                }
            }
            Err(conflict("Discovery exceeded its page limit."))
        }
        Harness::ClaudeCode => {
            process.send(&json!({"type":"control_request","request_id":"crewroom-discovery","request":{"subtype":"initialize","hooks":null}})).await?;
            let frame = process
                .receive(|v| {
                    v["type"] == "control_response"
                        && v["response"]["request_id"] == "crewroom-discovery"
                })
                .await?;
            if frame["response"]["subtype"] != "success" {
                return Err(conflict("Harness rejected discovery initialization."));
            }
            normalize(
                harness,
                &frame["response"]["response"]["models"],
                observed_at,
            )
        }
        Harness::Omp => {
            let bytes = process.collect().await?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| conflict("Invalid discovery JSON."))?;
            normalize(harness, &value["models"], observed_at)
        }
    }
}

pub async fn discover(
    config: &Config,
    root: &Path,
    observed_at: u64,
    cancel: &CancellationToken,
    limit: Duration,
) -> Result<Receipt> {
    if cancel.is_cancelled() {
        return Err(conflict("Discovery was cancelled."));
    }
    let (canonical_binary, binary_sha256) = binary_digest_blocking(&config.binary).await?;
    let stamp = binary_stamp(&config.binary)?;
    private_dir(root)?;
    for child in ["home", "config", "data", "cache", "omp-root"] {
        private_dir(&root.join(child))?;
    }
    let home = root.join("home");
    let private = root.join("config");
    let mut env = BTreeMap::from([
        ("HOME".into(), home.to_string_lossy().into()),
        ("PATH".into(), config.path.clone()),
        ("LANG".into(), "C.UTF-8".into()),
        ("CODEX_HOME".into(), private.to_string_lossy().into()),
        ("CLAUDE_CONFIG_DIR".into(), private.to_string_lossy().into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        (
            "PI_CODING_AGENT_DIR".into(),
            private.to_string_lossy().into(),
        ),
        ("PI_CONFIG_DIR".into(), "../omp-root".into()),
        (
            "XDG_DATA_HOME".into(),
            root.join("data").to_string_lossy().into(),
        ),
        (
            "XDG_CACHE_HOME".into(),
            root.join("cache").to_string_lossy().into(),
        ),
    ]);
    if config.harness == Harness::Omp {
        durable_new(
            &private.join("models.yml"),
            &serde_json::to_vec(&config.omp_models)?,
        )?;
        durable_new(&private.join("mcp.json"), br#"{"mcpServers":{}}"#)?;
        env.insert("NO_COLOR".into(), "1".into());
    }
    let mut process =
        Process::start(config, root, "version", vec!["--version".into()], &env).await?;
    let result = bounded(
        &mut process,
        cancel,
        limit.min(Duration::from_secs(5)),
        |p| Box::pin(p.collect()),
    )
    .await;
    process.close().await?;
    let raw = result?;
    let version = std::str::from_utf8(&raw)
        .map_err(|_| conflict("Invalid harness version."))?
        .trim();
    let Some(expected) = verified_versions(&config.harness)
        .iter()
        .find(|v| **v == version)
    else {
        return Err(conflict(&version_message(&config.harness, version)));
    };
    let args: Vec<String> = match config.harness {
        Harness::Codex => vec![
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "project_doc_max_bytes=0",
            "-c",
            "features.plugins=false",
            "-c",
            "features.hooks=false",
        ],
        Harness::ClaudeCode => vec![
            "--bare",
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--setting-sources",
            "",
            "--settings",
            r#"{"disableAllHooks":true}"#,
        ],
        Harness::Omp => vec!["models", "--json", "--no-extensions"],
    }
    .into_iter()
    .map(String::from)
    .collect();
    let mut process = Process::start(config, root, "metadata", args, &env).await?;
    let harness = config.harness.clone();
    let result = bounded(&mut process, cancel, limit, move |p| {
        Box::pin(async move { exchange(p, &harness, observed_at).await })
    })
    .await;
    process.close().await?;
    let offerings = result?;
    if binary_digest_blocking(&config.binary).await?
        != (canonical_binary.clone(), binary_sha256.clone())
        || binary_stamp(&config.binary)? != stamp
    {
        return Err(conflict("Harness executable changed during discovery."));
    }
    Ok(Receipt {
        identity: Identity {
            binary: config.binary.clone(),
            canonical_binary,
            binary_sha256,
            binary_stamp: stamp,
            version: version.into(),
            profile: format!("{}-{expected}-metadata-v1", config.harness.as_str()),
        },
        offerings,
        inference_requested: false,
        cleanup_verified: true,
    })
}
