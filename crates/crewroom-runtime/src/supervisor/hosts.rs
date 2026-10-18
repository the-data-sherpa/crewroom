//! Exact host/OMP ownership and passive event observation. The workflow layer
//! chooses assignments and recovery; this layer never invents a replacement.
mod launch;
mod observe;
mod recovery;
mod terminal;
use super::{
    client::HostClient,
    tasks::{RunTasks, Task},
};
use crate::{canonical, conflict, messaging::StoreAccess};
use crewroom_core::{Result, authority::Authority, store::Store};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct HostOptions {
    pub root: PathBuf,
    pub helper: PathBuf,
    pub host: PathBuf,
    pub tmux: PathBuf,
    pub socket: PathBuf,
    pub backend: String,
    pub command_engine: Option<PathBuf>,
}
struct Slot {
    run: String,
    attempt: String,
    root: PathBuf,
    cancel: CancellationToken,
    client: Mutex<Option<HostClient>>,
    observer: Mutex<Option<Task>>,
    status: Mutex<Value>,
    launch: Option<(u64, Value)>,
    stop: AsyncMutex<()>,
    wake: tokio::sync::Notify,
}
impl Slot {
    fn new(run: &str, attempt: &str, root: PathBuf, launch: Option<(u64, Value)>) -> Self {
        Self {
            run: run.into(),
            attempt: attempt.into(),
            root,
            cancel: CancellationToken::new(),
            client: Mutex::new(None),
            observer: Mutex::new(None),
            status: Mutex::new(json!({"state":"starting"})),
            launch,
            stop: AsyncMutex::new(()),
            wake: tokio::sync::Notify::new(),
        }
    }
}
#[derive(Default)]
struct Registry {
    closing: bool,
    stopping: HashSet<String>,
    stop_epochs: HashMap<String, u64>,
    slots: HashMap<String, Arc<Slot>>,
}
struct Inner<S: StoreAccess> {
    store: S,
    authority: Arc<Authority>,
    options: HostOptions,
    tasks: RunTasks,
    observers: RunTasks,
    cleanup: RunTasks,
    registry: Mutex<Registry>,
    tmux: AsyncMutex<()>,
    changed: Arc<dyn Fn(&str) + Send + Sync>,
}
#[derive(Clone)]
pub struct HostManager<S: StoreAccess>(Arc<Inner<S>>);
pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| conflict("Missing supervised process identity."))
}
impl<S: StoreAccess> HostManager<S> {
    /// Hash the exact launch helpers owned by this manager, including replacements
    /// at the same configured path. Comparisons bind this before admission and
    /// recheck it before every launch/recovery; no helper is executed to inspect it.
    pub(crate) fn execution_identity(&self) -> Result<Value> {
        Ok(json!({
            "host": executable_identity(&self.0.options.host)?,
            "process": executable_identity(&self.0.options.helper)?,
            "tmux": executable_identity(&self.0.options.tmux)?,
        }))
    }
    pub fn new(
        store: S,
        authority: Arc<Authority>,
        options: HostOptions,
        changed: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<Self> {
        authority.assert_current()?;
        for path in [&options.root, &options.helper, &options.host, &options.tmux] {
            canonical(path)?;
        }
        use std::os::unix::fs::MetadataExt;
        let root_metadata = std::fs::metadata(&options.root)?;
        // SAFETY: geteuid has no preconditions.
        if !root_metadata.is_dir()
            || root_metadata.mode() & 0o077 != 0
            || root_metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(conflict(
                "Supervised runtime directory must be private and owned.",
            ));
        }
        canonical(
            options
                .socket
                .parent()
                .ok_or_else(|| conflict("Missing private tmux socket directory."))?,
        )?;
        if options.socket.parent() != Some(options.root.as_path())
            || options.socket.file_name().is_none()
        {
            return Err(conflict("Tmux socket must be inside this runtime root."));
        }
        let socket_limit = std::mem::size_of::<libc::sockaddr_un>()
            - std::mem::offset_of!(libc::sockaddr_un, sun_path);
        if options.socket.as_os_str().len() >= socket_limit {
            return Err(conflict(
                "Runtime path is too long for the private tmux socket; configure a shorter runtime directory.",
            ));
        }
        super::client::origin(&options.backend)?;
        Ok(Self(Arc::new(Inner {
            store,
            authority,
            options,
            tasks: RunTasks::default(),
            observers: RunTasks::default(),
            cleanup: RunTasks::default(),
            registry: Mutex::default(),
            tmux: AsyncMutex::new(()),
            changed: Arc::new(changed),
        })))
    }
    fn store<T>(&self, operation: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        self.0.authority.assert_current()?;
        self.0.store.with_store(operation)
    }
    fn root(&self, attempt: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(attempt).map_err(|_| conflict("Invalid attempt identity."))?;
        let root = self.0.options.root.join(attempt);
        existing_directory(&root)?;
        Ok(root)
    }
    fn launch_guard(s: &Store, id: &str, revision: u64) -> Result<Value> {
        let attempt = s.get_attempt(id)?;
        let run = s.get_run(text(&attempt, "runId")?)?;
        if run["status"] != "awaiting_runner"
            || run["desiredState"] != "running"
            || run["controlRevision"].as_u64() != Some(revision)
            || run.get("sourceConflict").is_some_and(|v| !v.is_null())
        {
            return Err(conflict(
                "A newer control or requirements decision stopped agent launch.",
            ));
        }
        if !s.runtime(text(&attempt, "runId")?)?["assignments"]
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|a| {
                    a["currentAttemptId"] == id
                        && a.get("retiredAt").is_none_or(Value::is_null)
                        && a.get("rebriefRequired").is_none_or(Value::is_null)
                })
            })
        {
            return Err(conflict("Attempt no longer owns the current assignment."));
        }
        Ok(attempt)
    }
    fn permitted<T>(
        &self,
        slot: &Slot,
        revision: u64,
        operation: impl FnOnce(&Store) -> Result<T>,
    ) -> Result<T> {
        self.0.tasks.with_open(|| {
            let registry = self.0.registry.lock().unwrap();
            if registry.closing
                || registry.stopping.contains(&slot.run)
                || slot.cancel.is_cancelled()
            {
                return Err(conflict("Agent launch was stopped."));
            }
            self.store(|s| {
                Self::launch_guard(s, &slot.attempt, revision)?;
                operation(s)
            })
        })
    }
    fn status(&self, slot: &Slot, state: &str, reason: Option<&str>) {
        *slot.status.lock().unwrap() = json!({"state":state,"reason":reason});
        (self.0.changed)(&slot.run);
    }
    pub fn observation(&self, run: &str) -> Value {
        let registry = self.0.registry.lock().unwrap();
        let mut slots:Vec<_>=registry.slots.values().filter(|v|v.run==run).map(|slot|json!({"attemptId":slot.attempt,"observation":slot.status.lock().unwrap().clone()})).collect();
        slots.sort_by(|a, b| a["attemptId"].as_str().cmp(&b["attemptId"].as_str()));
        json!(slots)
    }
    pub fn authenticates(&self, attempt: &str, authorization: &str) -> bool {
        let registry = self.0.registry.lock().unwrap();
        !registry.closing
            && registry.slots.get(attempt).is_some_and(|slot| {
                !slot.cancel.is_cancelled()
                    && slot
                        .client
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|c| c.authenticates(authorization))
            })
    }
    pub fn observes(&self, attempt: &str) -> bool {
        self.0
            .registry
            .lock()
            .unwrap()
            .slots
            .get(attempt)
            .is_some_and(|slot| {
                !slot.cancel.is_cancelled()
                    && matches!(
                        slot.status.lock().unwrap()["state"].as_str(),
                        Some("active" | "degraded")
                    )
            })
    }
    pub fn busy(&self, run: &str) -> bool {
        self.0.tasks.busy(run) || self.0.cleanup.busy(run)
    }
    pub fn pane_exited(&self, pane: &str) -> Result<()> {
        let slots: Vec<_> = self
            .0
            .registry
            .lock()
            .unwrap()
            .slots
            .values()
            .cloned()
            .collect();
        for slot in slots {
            if self.store(|s| Ok(s.get_attempt(&slot.attempt)?["pane"] == pane))? {
                slot.wake.notify_one();
            }
        }
        Ok(())
    }
    pub async fn request(&self, attempt: &str, body: &Value) -> Result<Value> {
        let slot = self
            .0
            .registry
            .lock()
            .unwrap()
            .slots
            .get(attempt)
            .cloned()
            .ok_or_else(|| conflict("Agent host is not observed."))?;
        let client = slot
            .client
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| conflict("Agent endpoint is unavailable."))?;
        client.request(body, &slot.cancel).await
    }
}

pub(crate) fn executable_identity(configured: &Path) -> Result<Value> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::fs::canonicalize(configured)?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(conflict(
            "Frozen executable identity requires a regular file.",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(
        json!({"configuredPath":configured,"path":path,"sha256":crate::repository::files::hash(&bytes),"bytes":bytes.len()}),
    )
}

fn private_json(path: &Path, limit: u64) -> Result<Value> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    canonical(
        path.parent()
            .ok_or_else(|| conflict("Missing supervised record parent."))?,
    )?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !meta.is_file()
        || meta.mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.len() > limit
    {
        return Err(conflict(
            "Supervised process record must be a bounded private file.",
        ));
    }
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(conflict("Supervised process record exceeded its bound."));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn existing_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => canonical(path),
        Ok(_) => Err(conflict("Supervised process directory changed identity.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn executable_identity_detects_same_path_replacement_and_symlink_retargeting() {
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("host");
        std::fs::write(&host, b"first host").unwrap();
        let before = executable_identity(&host).unwrap();
        assert_eq!(before, executable_identity(&host).unwrap());
        std::fs::write(&host, b"changed host").unwrap();
        let after = executable_identity(&host).unwrap();
        assert_eq!(before["path"], after["path"]);
        assert_ne!(before["sha256"], after["sha256"]);
        let other = root.path().join("other");
        std::fs::write(&other, b"changed host").unwrap();
        let link = root.path().join("alias");
        std::os::unix::fs::symlink(&host, &link).unwrap();
        let first = executable_identity(&link).unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        let second = executable_identity(&link).unwrap();
        assert_eq!(first["sha256"], second["sha256"]);
        assert_ne!(first["path"], second["path"]);
        assert!(executable_identity(root.path()).is_err());
    }
}
