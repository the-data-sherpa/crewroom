//! Fixed-remote publication commands with durable, checked process ownership.
//! A live exit is transport evidence only; callers reconcile the remote effect.
use crate::{
    CheckedProcess, CheckedSpec, CommandSpec, canonical, conflict, create_root, durable_new,
};
use crewroom_core::{Result, authority::Authority};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::{OnceCell, watch},
    time::{Instant, timeout},
};
const LIMIT: u64 = 128 * 1024;
type Reply<T> = watch::Receiver<Option<std::result::Result<T, String>>>;
type Dispatch = Box<dyn FnOnce(&mut dyn FnMut() -> Result<Child>) -> Result<Child> + Send>;
#[derive(Clone)]
struct Outcome {
    code: i32,
    output: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    descriptor: CheckedProcess,
    spec: CheckedSpec,
}
struct Job {
    descriptor: CheckedProcess,
    reply: Reply<Outcome>,
}
struct State {
    lease: Option<File>,
    closing: Option<Reply<()>>,
    blocked: Option<String>,
    jobs: HashMap<String, Job>,
}
struct Inner {
    root: PathBuf,
    helper: PathBuf,
    executable: PathBuf,
    remote: String,
    protected: Vec<String>,
    authority: Arc<Authority>,
    recovered: OnceCell<std::result::Result<(), String>>,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct PublicationGit {
    inner: Arc<Inner>,
}
pub enum Remote {
    Bitbucket { workspace: String, slug: String },
    Fixture(PathBuf),
}
pub struct Push {
    pub directory: PathBuf,
    pub head: String,
    pub branch: String,
    pub expected_head: Option<String>,
}
pub struct Target {
    pub directory: PathBuf,
    pub branch: String,
    pub destination: String,
}
fn branch(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 200
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
        || value.contains("..")
        || value.contains("//")
        || value.ends_with(['/', '.'])
        || value
            .split('/')
            .any(|s| s.ends_with(".lock") || s.starts_with('.'))
    {
        return Err(conflict("Invalid publication branch."));
    }
    Ok(())
}
fn hash(value: &str) -> Result<()> {
    if !(40..=64).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(conflict("Invalid publication revision."));
    }
    Ok(())
}
fn git_executable() -> Result<PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|p| p.is_absolute())
    {
        let path = directory.join("git");
        if std::fs::metadata(&path)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Ok(std::fs::canonicalize(path)?);
        }
    }
    Err(conflict("Git executable is unavailable."))
}
fn environment() -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        ("HOME".into(), std::env::var("HOME").unwrap_or_default()),
        ("LANG".into(), "C".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        (
            "GIT_SSH_COMMAND".into(),
            "/usr/bin/ssh -o BatchMode=yes -o StrictHostKeyChecking=yes".into(),
        ),
    ]);
    if let Ok(socket) = std::env::var("SSH_AUTH_SOCK") {
        env.insert("SSH_AUTH_SOCK".into(), socket);
    }
    env
}
async fn receive<T: Clone>(mut reply: Reply<T>) -> Result<T> {
    loop {
        if let Some(value) = reply.borrow_and_update().clone() {
            return value.map_err(|e| conflict(&e));
        }
        reply.changed().await.map_err(|_| {
            conflict("Publication command worker was interrupted; reconcile its retained intent.")
        })?;
    }
}
impl PublicationGit {
    pub fn new(
        root: PathBuf,
        remote: Remote,
        protected: Vec<String>,
        helper: PathBuf,
        authority: Arc<Authority>,
    ) -> Result<Self> {
        authority.assert_current()?;
        canonical(&helper)?;
        let remote = match remote {
            Remote::Fixture(path) => {
                if !path.is_absolute() {
                    return Err(conflict("A fixture remote requires an absolute path."));
                }
                std::fs::canonicalize(path)?
                    .to_str()
                    .ok_or_else(|| conflict("Fixture remote path requires UTF-8."))?
                    .into()
            }
            Remote::Bitbucket { workspace, slug } => {
                if workspace.is_empty()
                    || slug.is_empty()
                    || !workspace
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    || !slug
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                {
                    return Err(conflict("Invalid Bitbucket remote identity."));
                }
                format!("git@bitbucket.org:{workspace}/{slug}.git")
            }
        };
        for name in &protected {
            branch(name)?;
        }
        let executable = git_executable()?;
        create_root(&root)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.join("transport.lock"))?;
        if !crate::flock(&lease, libc::LOCK_EX | libc::LOCK_NB)? {
            return Err(conflict(
                "Another publication transport owns this command directory.",
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                helper,
                executable,
                remote,
                protected,
                authority,
                recovered: OnceCell::new(),
                state: Mutex::new(State {
                    lease: Some(lease),
                    closing: None,
                    blocked: None,
                    jobs: HashMap::new(),
                }),
            }),
        })
    }
    async fn run(&self, args: Vec<String>, cwd: PathBuf, dispatch: Dispatch) -> Result<Outcome> {
        // Admission precedes the first await and the driven worker owns completion.
        self.inner.authority.assert_current()?;
        canonical(&cwd)?;
        let token = uuid::Uuid::new_v4().to_string();
        let mut options = [
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "credential.helper=",
        ]
        .map(String::from)
        .to_vec();
        options.extend(args);
        let spec = CheckedSpec {
            root: self.inner.root.join(&token),
            token: token.clone(),
            command: CommandSpec {
                command: self.inner.executable.to_string_lossy().into_owned(),
                args: options,
                cwd,
                env: environment(),
            },
            epoch: self.inner.authority.epoch().into(),
            controller_root: self.inner.authority.root().into(),
        };
        let intent = Intent {
            descriptor: crate::describe_checked(&spec)?,
            spec,
        };
        let (tx, rx) = watch::channel(None);
        {
            let mut state = self.inner.state.lock().unwrap();
            self.inner.assert_owned(&state)?;
            if state.closing.is_some() {
                return Err(conflict("Publication transport is stopping."));
            }
            if let Some(reason) = &state.blocked {
                return Err(conflict(reason));
            }
            state.jobs.insert(
                token.clone(),
                Job {
                    descriptor: intent.descriptor.clone(),
                    reply: rx.clone(),
                },
            );
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            struct Completion {
                inner: Arc<Inner>,
                token: String,
                tx: watch::Sender<Option<std::result::Result<Outcome, String>>>,
            }
            impl Drop for Completion {
                fn drop(&mut self) {
                    self.inner
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .jobs
                        .remove(&self.token);
                    if self.tx.borrow().is_none() {
                        self.tx.send_replace(Some(Err("Publication command worker was interrupted; reconcile its retained intent.".into())));
                    }
                }
            }
            let done = Completion {
                inner: inner.clone(),
                token,
                tx,
            };
            let descriptor = intent.descriptor.clone();
            let result = inner
                .execute(intent, dispatch)
                .await
                .map_err(|e| e.to_string());
            if result.is_err()
                && inner
                    .root
                    .join(format!("{}.json", descriptor.token))
                    .exists()
                && let Err(error) = inner.stop(&descriptor).await
            {
                inner.state.lock().unwrap().blocked = Some(error.to_string());
            }
            done.tx.send_replace(Some(result));
        });
        receive(rx).await
    }
    pub async fn reference(&self, name: &str) -> Result<Option<String>> {
        branch(name)?;
        let expected = format!("refs/heads/{name}");
        let result = self
            .run(
                vec![
                    "ls-remote".into(),
                    "--exit-code".into(),
                    "--refs".into(),
                    self.inner.remote.clone(),
                    expected.clone(),
                ],
                self.inner.root.clone(),
                Box::new(|launch| launch()),
            )
            .await?;
        let output = crewroom_core::store::trim_text(&result.output);
        if result.code == 2 && output.is_empty() {
            return Ok(None);
        }
        if result.code != 0 {
            return Err(conflict("Remote branch observation is unavailable."));
        }
        let lines = output.split('\n').collect::<Vec<_>>();
        let fields = lines[0].split_whitespace().collect::<Vec<_>>();
        if lines.len() != 1
            || fields.len() != 2
            || fields[1] != expected
            || hash(fields[0]).is_err()
        {
            return Err(conflict("Remote branch observation is inconsistent."));
        }
        Ok(Some(fields[0].into()))
    }
    /// The guard must keep workflow authorization synchronous with launch. It may
    /// hold the workflow mutex, but must not call this transport's close method.
    pub async fn push(
        &self,
        input: Push,
        guard: impl FnOnce(&mut dyn FnMut() -> Result<Child>) -> Result<Child> + Send + 'static,
    ) -> Result<()> {
        branch(&input.branch)?;
        hash(&input.head)?;
        if let Some(head) = &input.expected_head {
            hash(head)?;
        }
        if !input.directory.is_absolute() {
            return Err(conflict("Publication requires an absolute owned checkout."));
        }
        if ["main", "master"].contains(&input.branch.as_str())
            || self.inner.protected.contains(&input.branch)
        {
            return Err(conflict(
                "Publication cannot push a protected target branch.",
            ));
        }
        let result = self
            .run(
                vec![
                    "push".into(),
                    "--porcelain".into(),
                    format!(
                        "--force-with-lease=refs/heads/{}:{}",
                        input.branch,
                        input.expected_head.as_deref().unwrap_or("")
                    ),
                    self.inner.remote.clone(),
                    format!("{}:refs/heads/{}", input.head, input.branch),
                ],
                input.directory,
                Box::new(guard),
            )
            .await?;
        if result.code != 0 {
            return Err(conflict(
                "Git publication was not confirmed. Reconcile the remote branch before another write.",
            ));
        }
        Ok(())
    }
    /// The delivery service replays admitted request receipts before reaching this
    /// transport. Each fresh request gets its own target snapshot namespace.
    pub async fn fetch_target(
        &self,
        input: Target,
        guard: impl FnOnce(&mut dyn FnMut() -> Result<Child>) -> Result<Child> + Send + 'static,
    ) -> Result<()> {
        branch(&input.branch)?;
        let parts = input.destination.split('/').collect::<Vec<_>>();
        if parts.len() != 6
            || parts[0] != "refs"
            || parts[1] != "crewroom"
            || parts[3] != "refresh"
            || parts[5] != "target"
            || !crewroom_core::store::valid_uuid(parts[2])
            || !crewroom_core::store::valid_uuid(parts[4])
        {
            return Err(conflict(
                "A target fetch requires a request-owned snapshot ref.",
            ));
        }
        if !input.directory.is_absolute() {
            return Err(conflict(
                "Target refresh requires an absolute owned repository.",
            ));
        }
        let result = self
            .run(
                vec![
                    "fetch".into(),
                    "--no-tags".into(),
                    "--no-write-fetch-head".into(),
                    "--no-recurse-submodules".into(),
                    "--quiet".into(),
                    self.inner.remote.clone(),
                    format!("+refs/heads/{}:{}", input.branch, input.destination),
                ],
                input.directory,
                Box::new(guard),
            )
            .await?;
        if result.code != 0 {
            return Err(conflict("The remote target could not be fetched."));
        }
        Ok(())
    }
    pub async fn close(&self) -> Result<()> {
        let reply = {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(reply) = &state.closing {
                reply.clone()
            } else {
                let (tx, rx) = watch::channel(None);
                state.closing = Some(rx.clone());
                let inner = self.inner.clone();
                tokio::spawn(async move {
                    let result = inner.close().await.map_err(|e| e.to_string());
                    tx.send_replace(Some(result));
                });
                rx
            }
        };
        receive(reply).await
    }
}
impl Inner {
    fn assert_owned(&self, state: &State) -> Result<()> {
        self.authority.assert_current()?;
        canonical(&self.root)?;
        let saved = state
            .lease
            .as_ref()
            .ok_or_else(|| conflict("Publication transport is stopping."))?
            .metadata()?;
        let current = std::fs::symlink_metadata(self.root.join("transport.lock"))?;
        if current.file_type().is_symlink()
            || saved.dev() != current.dev()
            || saved.ino() != current.ino()
        {
            return Err(conflict("Publication transport ownership changed."));
        }
        Ok(())
    }
    fn validate(&self, intent: &Intent) -> Result<()> {
        if intent.descriptor.root != self.root.join(&intent.descriptor.token)
            || intent.descriptor.controller_root != self.authority.root()
            || crate::spec::describe_retained_checked(&intent.spec)? != intent.descriptor
        {
            return Err(conflict("Retained publication command identity changed."));
        }
        Ok(())
    }
    async fn stop(&self, descriptor: &CheckedProcess) -> Result<()> {
        let state = crate::seal_checked(descriptor).await;
        if state.state == "live" {
            crate::stop_owned(&descriptor.root).await;
        }
        if crate::inspect_checked(descriptor).await.state != "stopped" {
            return Err(conflict(
                "Publication process ownership is unresolved. No replacement command was started.",
            ));
        }
        Ok(())
    }
    async fn recover(&self) -> Result<()> {
        self.recovered
            .get_or_init(|| async { self.recover_saved().await.map_err(|e| e.to_string()) })
            .await
            .clone()
            .map_err(|e| conflict(&e))
    }
    async fn recover_saved(&self) -> Result<()> {
        self.assert_owned(&self.state.lock().unwrap())?;
        let mut intents = Vec::new();
        let mut directories = HashSet::new();
        let mut identities = HashSet::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| conflict("Unknown publication command entry."))?;
            if name == "transport.lock" {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.insert(name);
                continue;
            }
            if !kind.is_file() || !name.ends_with(".json") {
                return Err(conflict(
                    "Unknown publication command entry; retain it for inspection.",
                ));
            }
            let raw = crate::repository::files::regular_bytes(&self.root, &name, 1024 * 1024)?.0;
            let intent: Intent = serde_json::from_slice(&raw)
                .map_err(|_| conflict("Retained publication command is invalid."))?;
            self.validate(&intent)?;
            if name != format!("{}.json", intent.descriptor.token)
                || !identities.insert(intent.descriptor.token.clone())
            {
                return Err(conflict("Retained publication command identity changed."));
            }
            intents.push(intent);
        }
        if directories.iter().any(|name| !identities.contains(name)) {
            return Err(conflict(
                "Unrecognized or legacy publication process directory requires inspection.",
            ));
        }
        for intent in intents {
            self.stop(&intent.descriptor).await?;
            self.authority.assert_current()?;
        }
        Ok(())
    }
    async fn execute(&self, intent: Intent, dispatch: Dispatch) -> Result<Outcome> {
        self.recover().await?;
        self.validate(&intent)?;
        let mut child = {
            let state = self.state.lock().unwrap();
            self.assert_owned(&state)?;
            if state.closing.is_some() {
                return Err(conflict("Publication transport is stopping."));
            }
            if let Some(reason) = &state.blocked {
                return Err(conflict(reason));
            }
            let mut launch = || {
                self.authority.assert_current()?;
                durable_new(
                    &self.root.join(format!("{}.json", intent.descriptor.token)),
                    &serde_json::to_vec(&intent)?,
                )?;
                crate::prepare_checked(&intent.spec)?;
                crate::validate_launch(
                    &intent.descriptor,
                    self.authority.epoch(),
                    self.authority.root(),
                )?;
                Ok(Command::new(&self.helper)
                    .arg("checked-exec-report")
                    .arg(&intent.descriptor.root)
                    .arg(&intent.descriptor.token)
                    .arg(&intent.descriptor.spec_sha256)
                    .arg(&intent.descriptor.epoch)
                    .arg(&intent.descriptor.controller_root)
                    .process_group(0)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()?)
            };
            dispatch(&mut launch)?
        };
        let report = child
            .stdout
            .take()
            .ok_or_else(|| conflict("Live publication outcome channel is unavailable."))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            tokio::select! {
                result=child.wait()=>break result?,
                _=tokio::time::sleep_until(deadline)=>{
                    let stopped=self.stop(&intent.descriptor).await;let _=timeout(Duration::from_secs(5),child.wait()).await;stopped?;
                    return Err(conflict("Publication command timed out. Reconcile process ownership and the remote outcome."));
                },
                _=tokio::time::sleep(Duration::from_millis(25))=>{
                    let bytes=["stdout.log","stderr.log"].iter().filter_map(|n|std::fs::symlink_metadata(intent.descriptor.root.join(n)).ok()).map(|m|m.len()).sum::<u64>();
                    let overflow=["stdout.truncated","stderr.truncated"].iter().any(|n|intent.descriptor.root.join(n).exists());
                    if bytes>LIMIT || overflow {let stopped=self.stop(&intent.descriptor).await;let _=timeout(Duration::from_secs(5),child.wait()).await;stopped?;return Err(conflict("Publication command exceeded its output limit. Reconcile the remote outcome."));}
                }
            }
        };
        if crate::inspect_checked(&intent.descriptor).await.state != "stopped" {
            return Err(conflict("Publication process ownership is unresolved."));
        }
        let code = status.code().ok_or_else(|| {
            conflict("Publication command was interrupted. Reconcile the remote outcome.")
        })?;
        let mut bytes = Vec::new();
        timeout(
            Duration::from_secs(5),
            report.take(257).read_to_end(&mut bytes),
        )
        .await
        .map_err(|_| conflict("Live publication outcome channel did not settle."))??;
        if bytes.len() > 256
            || serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                != Some(serde_json::json!({"code":code,"signal":null}))
        {
            return Err(conflict(
                "Live publication outcome was not confirmed. Reconcile its retained intent.",
            ));
        }
        let capture: serde_json::Value = serde_json::from_slice(
            &crate::repository::files::regular_bytes(
                &intent.descriptor.root,
                "capture.json",
                64 * 1024,
            )?
            .0,
        )?;
        let mut total = 0;
        for stream in ["stdout", "stderr"] {
            let size = capture[stream]["retainedBytes"]
                .as_u64()
                .ok_or_else(|| conflict("Publication capture is incomplete."))?;
            total += size;
            if capture[stream]["truncated"] != false || size > LIMIT {
                return Err(conflict(
                    "Publication command exceeded its output limit. Reconcile the remote outcome.",
                ));
            }
            let bytes = crate::repository::files::regular_bytes(
                &intent.descriptor.root,
                &format!("{stream}.log"),
                LIMIT,
            )?
            .0;
            if bytes.len() as u64 != size {
                return Err(conflict("Publication capture identity changed."));
            }
        }
        if total > LIMIT {
            return Err(conflict(
                "Publication command exceeded its output limit. Reconcile the remote outcome.",
            ));
        }
        let output = String::from_utf8_lossy(
            &crate::repository::files::regular_bytes(&intent.descriptor.root, "stdout.log", LIMIT)?
                .0,
        )
        .into_owned();
        Ok(Outcome { code, output })
    }
    async fn close(&self) -> Result<()> {
        let mut error = self.recover().await.err();
        let jobs = self
            .state
            .lock()
            .unwrap()
            .jobs
            .values()
            .map(|j| (j.descriptor.clone(), j.reply.clone()))
            .collect::<Vec<_>>();
        for (descriptor, _) in &jobs {
            // Closing already fences dispatch under the same state mutex. An
            // admitted worker with no durable intent cannot have spawned a child.
            if !self
                .root
                .join(format!("{}.json", descriptor.token))
                .try_exists()?
            {
                continue;
            }
            if let Err(e) = self.stop(descriptor).await {
                error.get_or_insert(e);
            }
        }
        for (_, reply) in jobs {
            let _ = receive(reply).await;
        }
        // A failed/partial preparation may have left a durable intent after the
        // initial recovery; revalidate every retained identity before release.
        if let Err(e) = self.recover_saved().await {
            error.get_or_insert(e);
        }
        if let Some(error) = error {
            return Err(error);
        }
        self.state.lock().unwrap().lease.take();
        Ok(())
    }
}
