//! Durable per-harness refresh lifecycle. Catalog writes never hold workflow
//! authority while a metadata process runs, and never admit agent work.
use super::{
    discovery::{self, Config, Identity, Receipt},
    *,
};
use crewroom_core::authority::write_private;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

fn write_snapshot(path: &Path, snapshot: &Snapshot) -> Result<()> {
    let encoded = serde_json::to_vec(snapshot)?;
    if encoded.len() > MAX_SNAPSHOT_BYTES {
        return Err(conflict("Catalog snapshot exceeds the storage limit."));
    }
    write_private(path, &encoded)
}

#[derive(Clone)]
pub struct Source {
    pub config: Config,
    /// Hash of this harness's private configuration/profile inputs. No raw
    /// configuration is persisted in the public snapshot.
    pub configuration_revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Idle,
    Refreshing,
    Success,
    Error,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessSnapshot {
    pub harness: Harness,
    pub outcome: Outcome,
    pub last_success: Option<u64>,
    pub last_attempt: Option<u64>,
    pub identity: Option<Identity>,
    pub configuration_revision: Option<String>,
    pub offerings: Vec<Offering>,
    pub error: Option<String>,
}
impl HarnessSnapshot {
    pub fn fresh(&self, now: u64, configuration_revision: &str) -> bool {
        self.outcome == Outcome::Success
            && self.configuration_revision.as_deref() == Some(configuration_revision)
            && self.identity.is_some()
            && self
                .last_success
                .is_some_and(|t| now >= t && now - t < FRESH_FOR_SECONDS)
    }
    /// Listing models and running them are separate verifications. Explain a
    /// discovered version whose execution adapter has not passed live launch.
    pub fn execution_note(&self) -> Option<String> {
        let version = &self.identity.as_ref()?.version;
        let verified = crate::runner::execution_versions(self.harness.as_str());
        (!verified.contains(&version.as_str())).then(|| {
            format!(
                "{} {version} lists models, but Crewroom has verified launching agents only with {}. Its models stay unavailable for new work until this version is verified.",
                self.harness.label(),
                verified.join(", ")
            )
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub revision: u64,
    pub generation: u64,
    pub state: String,
    pub harnesses: Vec<HarnessSnapshot>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            generation: 0,
            state: "idle".into(),
            harnesses: [Harness::Omp, Harness::Codex, Harness::ClaudeCode]
                .into_iter()
                .map(|harness| HarnessSnapshot {
                    harness,
                    outcome: Outcome::Idle,
                    last_success: None,
                    last_attempt: None,
                    identity: None,
                    configuration_revision: None,
                    offerings: Vec::new(),
                    error: None,
                })
                .collect(),
        }
    }
}
impl Snapshot {
    fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION
            || self.harnesses.len() != 3
            || !["idle", "refreshing", "success", "partial", "error"].contains(&self.state.as_str())
        {
            return Err(conflict("Unsupported model catalog snapshot."));
        }
        let mut names = HashSet::new();
        for entry in &self.harnesses {
            if !names.insert(entry.harness.as_str())
                || entry.offerings.iter().any(|o| o.harness != entry.harness)
            {
                return Err(conflict("Invalid catalog harness identity."));
            }
            validate_contract(&entry.offerings, &[])?;
        }
        Ok(())
    }
    fn advance(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| conflict("Catalog revision exhausted."))?;
        Ok(())
    }
    fn begin(&mut self, now: u64) -> Result<u64> {
        self.advance()?;
        self.generation = self.revision;
        self.state = "refreshing".into();
        for entry in &mut self.harnesses {
            entry.outcome = Outcome::Refreshing;
            entry.last_attempt = Some(now);
            entry.error = None;
        }
        Ok(self.generation)
    }
    fn record(
        &mut self,
        generation: u64,
        harness: &Harness,
        configuration: &str,
        result: std::result::Result<Receipt, String>,
        now: u64,
    ) -> Result<()> {
        if self.generation != generation || self.state != "refreshing" {
            return Err(conflict("A newer catalog refresh superseded this result."));
        }
        let entry = self
            .harnesses
            .iter_mut()
            .find(|v| &v.harness == harness)
            .ok_or_else(|| conflict("Unknown catalog harness."))?;
        if entry.outcome != Outcome::Refreshing {
            return Err(conflict("Catalog refresh result was already recorded."));
        }
        match result {
            Ok(receipt) => {
                if receipt.inference_requested
                    || !receipt.cleanup_verified
                    || receipt.offerings.iter().any(|o| &o.harness != harness)
                {
                    return Err(conflict(
                        "Discovery receipt does not establish metadata-only cleanup.",
                    ));
                }
                validate_contract(&receipt.offerings, &[])?;
                entry.outcome = Outcome::Success;
                entry.last_success = Some(now);
                entry.identity = Some(receipt.identity);
                entry.configuration_revision = Some(configuration.into());
                entry.offerings = receipt.offerings;
                entry.error = None;
            }
            Err(error) => {
                entry.outcome = Outcome::Error;
                entry.error = Some(error);
            }
        }
        self.advance()?;
        if self
            .harnesses
            .iter()
            .all(|h| h.outcome != Outcome::Refreshing)
        {
            let successes = self
                .harnesses
                .iter()
                .filter(|h| h.outcome == Outcome::Success)
                .count();
            self.state = match successes {
                0 => "error",
                3 => "success",
                _ => "partial",
            }
            .into();
        }
        Ok(())
    }
}
struct State {
    snapshot: Snapshot,
    persistence_error: bool,
    closing: bool,
    worker: Option<tokio::task::JoinHandle<()>>,
    configurations: BTreeMap<String, String>,
}
struct Inner {
    path: PathBuf,
    processes: PathBuf,
    sources: Vec<Source>,
    state: Mutex<State>,
    cancel: CancellationToken,
    changed: Arc<dyn Fn() + Send + Sync>,
}
#[derive(Clone)]
pub struct Catalog(Arc<Inner>);
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Catalog {
    pub fn open(
        root: &Path,
        sources: Vec<Source>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        crate::canonical(root)?;
        let meta = std::fs::metadata(root)?;
        if meta.mode() & 0o077 != 0 {
            return Err(conflict("Catalog directory must be private."));
        }
        let mut names = HashSet::new();
        if sources
            .iter()
            .any(|s| !names.insert(s.config.harness.as_str()))
        {
            return Err(conflict("Duplicate discovery source."));
        }
        let path = root.join("catalog.json");
        let mut snapshot: Snapshot = if path.exists() {
            let meta = std::fs::symlink_metadata(&path)?;
            if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.len() > MAX_SNAPSHOT_BYTES as u64
            {
                return Err(conflict("Invalid private catalog snapshot."));
            }
            serde_json::from_slice(&crate::read(&path)?)
                .map_err(|_| conflict("Invalid model catalog snapshot."))?
        } else {
            Snapshot::default()
        };
        snapshot.validate()?;
        let mut persistence_error = false;
        if snapshot.state == "refreshing" {
            for h in &mut snapshot.harnesses {
                if h.outcome == Outcome::Refreshing {
                    h.outcome = Outcome::Interrupted;
                    h.error = Some(
                        "Refresh interrupted by service restart. Refresh again to recover.".into(),
                    );
                }
            }
            snapshot.state = "error".into();
            snapshot.advance()?;
            persistence_error = write_snapshot(&path, &snapshot).is_err();
        }
        let configurations = sources
            .iter()
            .map(|s| {
                (
                    s.config.harness.as_str().into(),
                    s.configuration_revision.clone(),
                )
            })
            .collect();
        Ok(Self(Arc::new(Inner {
            path,
            processes: root.join("processes"),
            sources,
            state: Mutex::new(State {
                snapshot,
                persistence_error,
                closing: false,
                worker: None,
                configurations,
            }),
            cancel: CancellationToken::new(),
            changed: Arc::new(changed),
        })))
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let state = self.0.state.lock().unwrap();
        if state.persistence_error {
            return Err(conflict(
                "Catalog persistence failed. Refresh again before admission.",
            ));
        }
        Ok(state.snapshot.clone())
    }
    pub fn source(&self, harness: &Harness) -> Option<&Source> {
        self.0.sources.iter().find(|s| &s.config.harness == harness)
    }
    pub fn view(&self, now: u64) -> Value {
        let state = self.0.state.lock().unwrap();
        let mut view = serde_json::to_value(&state.snapshot).unwrap();
        view["persistenceError"] = if state.persistence_error {
            json!("Catalog persistence failed. Refresh again before admission.")
        } else {
            Value::Null
        };
        view["freshForSeconds"] = json!(FRESH_FOR_SECONDS);
        for h in view["harnesses"].as_array_mut().unwrap() {
            let entry = state
                .snapshot
                .harnesses
                .iter()
                .find(|s| h["harness"] == s.harness.as_str())
                .unwrap();
            h["fresh"] = json!(
                !state.persistence_error
                    && self.source(&entry.harness).is_some_and(|s| entry.fresh(
                        now,
                        state
                            .configurations
                            .get(s.config.harness.as_str())
                            .unwrap_or(&s.configuration_revision)
                    ) && entry
                        .identity
                        .as_ref()
                        .is_some_and(|i| discovery::binary_stamp(&s.config.binary)
                            .is_ok_and(|stamp| stamp == i.binary_stamp)))
            );
            h["configured"] = json!(self.source(&entry.harness).is_some());
        }
        view
    }
    fn persist(&self, state: &mut State, next: Snapshot) -> Result<()> {
        if write_snapshot(&self.0.path, &next).is_err() {
            state.persistence_error = true;
            return Err(conflict(
                "Catalog persistence failed. Refresh again before admission.",
            ));
        }
        state.snapshot = next;
        state.persistence_error = false;
        Ok(())
    }
    /// Synchronous reservation, then an owned background worker. Losing the HTTP
    /// response does not cancel work; repeated requests join the same generation.
    pub fn refresh(&self) -> Result<Value> {
        self.refresh_with_revisions(BTreeMap::new())
    }
    pub fn refresh_with_revisions(&self, revisions: BTreeMap<String, String>) -> Result<Value> {
        let mut state = self.0.state.lock().unwrap();
        if state.closing {
            return Err(conflict("Catalog discovery is stopping."));
        }
        if state.worker.as_ref().is_some_and(|w| !w.is_finished()) {
            drop(state);
            return Ok(self.view(now()));
        }
        let mut next = state.snapshot.clone();
        let generation = next.begin(now())?;
        self.persist(&mut state, next)?;
        state.configurations.extend(revisions);
        let mut sources = self.0.sources.clone();
        for source in &mut sources {
            if let Some(revision) = state.configurations.get(source.config.harness.as_str()) {
                source.configuration_revision = revision.clone();
            }
        }
        let this = self.clone();
        state.worker = Some(tokio::spawn(async move {
            let recovery = this.recover_processes().await;
            for harness in [Harness::Omp, Harness::Codex, Harness::ClaudeCode] {
                let source = sources.iter().find(|s| s.config.harness == harness);
                let configuration = source
                    .map(|s| s.configuration_revision.as_str())
                    .unwrap_or("");
                let result = if recovery.is_err() {
                    Err("Previous discovery process ownership needs inspection.".into())
                } else if let Some(source) = source {
                    let root = this
                        .0
                        .processes
                        .join(format!("refresh-{generation}"))
                        .join(harness.as_str());
                    discovery::discover(
                        &source.config,
                        &root,
                        now(),
                        &this.0.cancel,
                        Duration::from_secs(30),
                    )
                    .await
                    .map_err(|error| match error {
                        crewroom_core::Error::Conflict(message) => message,
                        _ => {
                            "Model discovery failed; inspect the configured executable and profile."
                                .into()
                        }
                    })
                } else {
                    Err("Harness is not configured.".into())
                };
                let mut state = this.0.state.lock().unwrap();
                let mut next = state.snapshot.clone();
                let recorded = next
                    .record(generation, &harness, configuration, result, now())
                    .and_then(|()| this.persist(&mut state, next));
                if recorded.is_err() {
                    state.persistence_error = true;
                }
                drop(state);
                (this.0.changed)();
                if recorded.is_err() {
                    break;
                }
            }
            (this.0.changed)();
        }));
        drop(state);
        (self.0.changed)();
        Ok(self.view(now()))
    }
    pub(crate) async fn recover_processes(&self) -> Result<()> {
        if !self.0.processes.exists() {
            return Ok(());
        }
        for run in std::fs::read_dir(&self.0.processes)? {
            let run = run?;
            if !run.file_type()?.is_dir() {
                return Err(conflict("Invalid discovery recovery directory."));
            }
            for harness in ["omp", "codex", "claude-code"] {
                for command in ["version", "metadata"] {
                    let root = run.path().join(harness).join(command);
                    if root.exists() && crate::stop_owned(&root).await.state != "stopped" {
                        return Err(conflict(
                            "Previous discovery process ownership needs inspection.",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn close(&self) -> Result<()> {
        let worker = {
            let mut state = self.0.state.lock().unwrap();
            state.closing = true;
            self.0.cancel.cancel();
            state.worker.take()
        };
        if let Some(worker) = worker {
            worker
                .await
                .map_err(|_| conflict("Discovery worker did not stop cleanly."))?;
        }
        self.recover_processes().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt(harness: Harness) -> Receipt {
        Receipt {
            identity: Identity {
                binary: "/fixture/binary".into(),
                canonical_binary: "/fixture/binary".into(),
                binary_sha256: "hash".into(),
                binary_stamp: "stamp".into(),
                version: "fixture".into(),
                profile: "fixture".into(),
            },
            offerings: vec![Offering {
                key: offering_key(&harness, "p", "m"),
                harness,
                provider: "p".into(),
                selector: "m".into(),
                display_name: "Model".into(),
                api: None,
                observed_model: None,
                capabilities: Capabilities {
                    efforts: None,
                    inputs: None,
                    tools: None,
                },
                access: Access::Unknown,
                execution_profile: None,
                inference: None,
                billing: None,
            }],
            inference_requested: false,
            cleanup_verified: true,
        }
    }
    #[test]
    fn partial_refresh_retains_history_without_freshening_failed_entries() {
        let mut s = Snapshot::default();
        let g = s.begin(100).unwrap();
        for h in [Harness::Omp, Harness::Codex, Harness::ClaudeCode] {
            s.record(g, &h, "config", Ok(receipt(h.clone())), 100)
                .unwrap();
        }
        assert_eq!(s.state, "success");
        let old = s.harnesses[0].offerings.clone();
        let g = s.begin(200).unwrap();
        s.record(g, &Harness::Omp, "changed", Err("Unavailable".into()), 200)
            .unwrap();
        s.record(
            g,
            &Harness::Codex,
            "config",
            Ok(receipt(Harness::Codex)),
            200,
        )
        .unwrap();
        s.record(
            g,
            &Harness::ClaudeCode,
            "config",
            Err("Unavailable".into()),
            200,
        )
        .unwrap();
        assert_eq!(s.state, "partial");
        assert_eq!(s.harnesses[0].offerings, old);
        assert_eq!(s.harnesses[0].last_success, Some(100));
        assert!(!s.harnesses[0].fresh(200, "config"));
        assert!(s.harnesses[1].fresh(200, "config"));
        assert!(!s.harnesses[1].fresh(3800, "config"));
        assert!(!s.harnesses[1].fresh(199, "config"));
        assert!(!s.harnesses[1].fresh(200, "changed"));
        assert!(
            s.record(
                g - 1,
                &Harness::Codex,
                "config",
                Ok(receipt(Harness::Codex)),
                201
            )
            .is_err()
        );
    }
    #[test]
    fn restart_marks_interrupted_refresh_and_atomic_failure_blocks_admission() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(&root.path().join("catalog"), vec![], || {}).unwrap();
        {
            let mut state = catalog.0.state.lock().unwrap();
            let mut next = state.snapshot.clone();
            next.begin(100).unwrap();
            catalog.persist(&mut state, next).unwrap();
        }
        drop(catalog);
        let catalog = Catalog::open(&root.path().join("catalog"), vec![], || {}).unwrap();
        assert_eq!(
            catalog.snapshot().unwrap().harnesses[0].outcome,
            Outcome::Interrupted
        );
        std::fs::remove_file(&catalog.0.path).unwrap();
        std::fs::create_dir(&catalog.0.path).unwrap();
        {
            let mut state = catalog.0.state.lock().unwrap();
            let mut next = state.snapshot.clone();
            next.begin(200).unwrap();
            assert!(catalog.persist(&mut state, next).is_err());
        }
        assert!(catalog.snapshot().is_err());
        assert!(catalog.view(200)["persistenceError"].is_string());
    }
    #[test]
    fn oversized_snapshot_keeps_the_last_readable_snapshot_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(&root.path().join("catalog"), vec![], || {}).unwrap();
        {
            let mut state = catalog.0.state.lock().unwrap();
            let initial = state.snapshot.clone();
            catalog.persist(&mut state, initial).unwrap();
        }
        let before = std::fs::read(&catalog.0.path).unwrap();
        {
            let mut state = catalog.0.state.lock().unwrap();
            let mut next = state.snapshot.clone();
            let generation = next.begin(100).unwrap();
            for harness in [Harness::Omp, Harness::Codex] {
                let mut response = receipt(harness.clone());
                let original = response.offerings[0].clone();
                response.offerings = (0..6000)
                    .map(|index| {
                        let mut offering = original.clone();
                        offering.selector = format!("{}-{index}", "a".repeat(800));
                        offering.key = offering_key(&harness, "p", &offering.selector);
                        offering.display_name = "n".repeat(100);
                        offering
                    })
                    .collect();
                next.record(generation, &harness, "config", Ok(response), 100)
                    .unwrap();
            }
            assert!(serde_json::to_vec(&next).unwrap().len() > 16 * 1024 * 1024);
            assert!(catalog.persist(&mut state, next).is_err());
        }
        assert!(catalog.snapshot().is_err());
        assert_eq!(std::fs::read(&catalog.0.path).unwrap(), before);
        drop(catalog);
        assert_eq!(
            Catalog::open(&root.path().join("catalog"), vec![], || {})
                .unwrap()
                .snapshot()
                .unwrap()
                .revision,
            0
        );
    }
    #[tokio::test]
    async fn interrupted_snapshot_at_limit_stays_readable_and_can_refresh_again() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog");
        let catalog = Catalog::open(&path, vec![], || {}).unwrap();
        let mut snapshot = Snapshot::default();
        snapshot.begin(100).unwrap();
        snapshot.harnesses[0].configuration_revision = Some(String::new());
        let length = serde_json::to_vec(&snapshot).unwrap().len();
        // Leave room for a new timestamp, but not three interrupted diagnostics.
        snapshot.harnesses[0].configuration_revision =
            Some("a".repeat(MAX_SNAPSHOT_BYTES - length - 64));
        write_snapshot(&catalog.0.path, &snapshot).unwrap();
        let before = std::fs::read(&catalog.0.path).unwrap();
        drop(catalog);
        let catalog = Catalog::open(&path, vec![], || {}).unwrap();
        assert!(catalog.snapshot().is_err());
        assert_eq!(catalog.view(200)["harnesses"][0]["outcome"], "interrupted");
        assert_eq!(std::fs::read(&catalog.0.path).unwrap(), before);
        // A new refresh clears interrupted diagnostics and can persist again.
        catalog.refresh().unwrap();
        catalog.close().await.unwrap();
        assert!(std::fs::metadata(&catalog.0.path).unwrap().len() <= MAX_SNAPSHOT_BYTES as u64);
        assert!(Catalog::open(&path, vec![], || {}).is_ok());
    }
    #[tokio::test]
    async fn concurrent_refresh_requests_share_a_generation_and_shutdown_fences_refresh() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(&root.path().join("catalog"), vec![], || {}).unwrap();
        let a = catalog.refresh().unwrap();
        let b = catalog.refresh().unwrap();
        assert_eq!(a["generation"], b["generation"]);
        catalog.close().await.unwrap();
        assert_eq!(catalog.snapshot().unwrap().state, "error");
        assert!(catalog.refresh().is_err());
    }
}
