//! Read-only provider identity and request-owned target snapshots. Repository
//! callbacks never reacquire repository operation locks.
mod host;
mod lifecycle;
use crate::{
    conflict,
    messaging::StoreAccess,
    providers::remote::{PublicationProvider, UnavailablePublication},
    publication_git::{PublicationGit, Remote, Target},
    repository::{Repository, array, request, text},
};
use crewroom_core::{Error, Result, authority::Authority, serialized::stringify};
pub use host::PullRequestHost;
use lifecycle::Operations;
pub(crate) use lifecycle::Ticket;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct Connection {
    pub(crate) path: PathBuf,
    pub(crate) repository: Value,
    pub(crate) host: PullRequestHost,
    pub(crate) policy: Value,
    fixture: Option<PathBuf>,
}
impl Connection {
    /// `host` is the provider resolved from the repository's publish remote;
    /// a provider client converts into it directly.
    pub fn new(
        path: &Path,
        repository: &Value,
        host: impl Into<PullRequestHost>,
        policy: &Value,
        fixture: Option<&Path>,
    ) -> Result<Self> {
        let host = host.into();
        host.check_configured(repository)?;
        request(policy, &["statusKeys", "pipelineSteps", "optionalSteps"])?;
        let mut normalized = json!({});
        for key in ["statusKeys", "pipelineSteps", "optionalSteps"] {
            let empty = json!([]);
            let values = policy
                .get(key)
                .or_else(|| (key == "optionalSteps").then_some(&empty))
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Invalid("Invalid hosted check policy.".into()))?;
            let mut seen = HashSet::new();
            if values.len() > 100
                || (key != "optionalSteps" && values.is_empty())
                || values.iter().any(|v| {
                    v.as_str()
                        .is_none_or(|s| s.is_empty() || s.chars().count() > 200 || !seen.insert(s))
                })
            {
                return Err(Error::Invalid("Invalid hosted check policy.".into()));
            }
            normalized[key] = json!(values);
        }
        Ok(Self {
            path: std::fs::canonicalize(path)?,
            repository: repository.clone(),
            host,
            policy: normalized,
            fixture: fixture.map(std::fs::canonicalize).transpose()?,
        })
    }
    pub fn policy(&self) -> &Value {
        &self.policy
    }
    pub fn repository_path(&self) -> &Path {
        &self.path
    }
    pub fn provider(&self) -> PublicationProvider {
        self.host.provider()
    }
    fn remote(&self) -> Result<Remote> {
        match &self.fixture {
            Some(path) => Ok(Remote::Fixture(path.clone())),
            None => self.host.git_remote(),
        }
    }
}
/// The provider that wrote a durable publication record. GitHub identities
/// carry their provider; every earlier record is Bitbucket.
pub(crate) fn recorded_provider(record: &Value) -> PublicationProvider {
    if crate::providers::github_pulls::is_github(&record["repository"]) {
        PublicationProvider::GitHub
    } else {
        PublicationProvider::Bitbucket
    }
}
/// Explains why a record's repository has no matching configured provider.
pub(crate) fn unmatched(record: &Value) -> String {
    format!(
        "The configured {} repository no longer matches this publication.",
        recorded_provider(record).name()
    )
}
struct Inner<S: StoreAccess> {
    store: S,
    root: PathBuf,
    helper: PathBuf,
    authority: Arc<Authority>,
    connections: Vec<Connection>,
    unavailable: Vec<UnavailablePublication>,
    operations: Operations,
}
#[derive(Clone)]
pub struct PublicationRemote<S: StoreAccess>(Arc<Inner<S>>);
impl<S: StoreAccess> PublicationRemote<S> {
    pub fn new(
        store: S,
        root: PathBuf,
        helper: PathBuf,
        authority: Arc<Authority>,
        connections: Vec<Connection>,
    ) -> Result<Self> {
        Self::with_unavailable(store, root, helper, authority, connections, vec![])
    }
    /// `unavailable` lists configured repositories whose publish remote has no
    /// publication provider; they are reported, never published.
    pub fn with_unavailable(
        store: S,
        root: PathBuf,
        helper: PathBuf,
        authority: Arc<Authority>,
        connections: Vec<Connection>,
        unavailable: Vec<UnavailablePublication>,
    ) -> Result<Self> {
        authority.assert_current()?;
        crate::create_root(&root)?;
        crate::canonical(&helper)?;
        let unavailable = unavailable
            .into_iter()
            .map(|mut u| {
                if let Ok(path) = std::fs::canonicalize(&u.repository_path) {
                    u.repository_path = path;
                }
                u
            })
            .collect();
        Ok(Self(Arc::new(Inner {
            store,
            root,
            helper,
            authority,
            connections,
            unavailable,
            operations: Operations::default(),
        })))
    }
    /// Weak callbacks leave transport lifetime under the owning workflow service.
    pub fn install_observers(&self, work: &Repository<S>) {
        let weak = Arc::downgrade(&self.0);
        work.set_target_observer(move |work| {
            let inner = weak.upgrade();
            async move {
                PublicationRemote(
                    inner
                        .ok_or_else(|| conflict("The configured remote target is unavailable."))?,
                )
                .observe_target(work)
                .await
            }
        });
        let weak = Arc::downgrade(&self.0);
        work.set_continuation_observer(move |id| {
            let inner = weak.upgrade();
            async move {
                PublicationRemote(
                    inner.ok_or_else(|| conflict("Continuation verification is unavailable."))?,
                )
                .verify_cancelled_continuation(&id)
                .await
            }
        });
    }
    pub(crate) fn store<T>(
        &self,
        f: impl FnOnce(&crewroom_core::store::Store) -> Result<T>,
    ) -> Result<T> {
        self.0.authority.assert_current()?;
        self.0.store.with_store(f)
    }
    pub(crate) fn connection(&self, path: &str) -> Option<Connection> {
        self.0
            .connections
            .iter()
            .find(|c| c.path == Path::new(path))
            .cloned()
    }
    /// The unavailable-provider report for a configured repository whose
    /// publish remote no provider implements.
    pub fn unavailable(&self, path: &str) -> Option<UnavailablePublication> {
        if self.connection(path).is_some() {
            return None;
        }
        self.0
            .unavailable
            .iter()
            .find(|u| u.repository_path == Path::new(path))
            .cloned()
    }
    pub(crate) fn matched(&self, record: &Value) -> Option<Connection> {
        self.connection(record["repositoryPath"].as_str()?)
            .filter(|c| stringify(&c.repository) == stringify(&record["repository"]))
    }
    pub(crate) fn lane(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.0.operations.lane(key)
    }
    pub(crate) async fn operation<T, F, Fut>(&self, run: &str, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Ticket) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.0.authority.assert_current()?;
        self.0.operations.run(run, None, f).await
    }
    pub(crate) async fn ordered<T, F, Fut>(&self, run: &str, key: &str, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Ticket) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.0.authority.assert_current()?;
        self.0.operations.run(run, Some(key), f).await
    }
    pub(crate) fn register<T, F, Fut>(
        &self,
        run: &str,
        key: &str,
        f: F,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<T>>>
    where
        T: Send + 'static,
        F: FnOnce(Ticket) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.0.authority.assert_current()?;
        self.0.operations.start(run, Some(key), f)
    }
    pub(crate) async fn git<T, F, Fut>(
        &self,
        ticket: &Ticket,
        run: &str,
        suffix: &str,
        target: &str,
        connection: &Connection,
        operation: F,
    ) -> Result<T>
    where
        F: FnOnce(PublicationGit) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let root = self.0.root.join(format!("{run}{suffix}"));
        let lane = self.lane(
            root.to_str()
                .ok_or_else(|| conflict("Invalid transport path."))?,
        );
        let _guard = lane.lock().await;
        let git = ticket.attach(|| {
            PublicationGit::new(
                root,
                connection.remote()?,
                vec![target.into(), "main".into(), "master".into()],
                self.0.helper.clone(),
                self.0.authority.clone(),
            )
        })?;
        let result = operation(git.clone()).await;
        git.close().await?;
        ticket.detached();
        result
    }
    pub async fn observe_target(&self, work: Value) -> Result<Option<String>> {
        let id = text(&work, "runId")?.to_owned();
        if !crewroom_core::store::valid_uuid(&id) {
            return Err(Error::Invalid("Invalid target observer run UUID.".into()));
        }
        let this = self.clone();
        self.operation(&id.clone(),move |ticket| async move {
            let connection = this.connection(text(&work["repository"],"path")?).ok_or_else(|| conflict("The configured remote target is unavailable. Retain the branch for inspection."))?;
            let target = text(&work["repository"],"targetBranch")?;
            let result = this.git(&ticket,&id,".observe",target,&connection,|git| async move { git.reference(target).await }).await;
            ticket.check()?;
            result.map_err(|_| conflict("The remote target observation is unavailable. Retain the branch for inspection."))
        }).await
    }
    pub async fn fetch_target(
        &self,
        run: &str,
        request_id: &str,
        supersedes: Option<Value>,
    ) -> Result<Option<String>> {
        let (run, key) = (run.to_owned(), request_id.to_owned());
        for id in [&run, &key] {
            if !crewroom_core::store::valid_uuid(id) {
                return Err(Error::Invalid(
                    "Target refresh requires valid UUIDs.".into(),
                ));
            }
        }
        let this = self.clone();
        self.operation(&run.clone(), move |ticket| async move {
            this.fetch_current(ticket, run, key, supersedes).await
        })
        .await
    }
    pub(crate) async fn fetch_current(
        &self,
        ticket: Ticket,
        run: String,
        key: String,
        supersedes: Option<Value>,
    ) -> Result<Option<String>> {
        ticket.check()?;
        let owned = self.store(|s| s.repository_work(&run))?;
        let Some(owned) = owned else { return Ok(None) };
        let Some(connection) = self.connection(text(&owned["repository"], "path")?) else {
            if retained_target(&owned) {
                return Err(conflict(
                    "The configured remote target is unavailable. Do not verify a retained snapshot as the live target.",
                ));
            }
            return Ok(None);
        };
        let (current, proof) = self.store(|s| {
            Ok((
                s.preflight_target_refresh(&run, supersedes.as_ref())?,
                fetch_proof(s, &run)?,
            ))
        })?;
        if array(&current["refreshes"])
            .iter()
            .any(|r| r["requestId"] == key)
        {
            return Err(conflict(
                "A recorded target request cannot be fetched again. Replay its pinned receipt.",
            ));
        }
        let destination = format!("refs/crewroom/{run}/refresh/{key}/target");
        let input = Target {
            directory: connection.path.clone(),
            branch: text(&owned["repository"], "targetBranch")?.into(),
            destination: destination.clone(),
        };
        let target = input.branch.clone();
        let guard_ticket = ticket.clone();
        let guard_store = self.clone();
        let guard_run = run.clone();
        self.git(
            &ticket,
            &run,
            ".target",
            &target,
            &connection,
            move |git| async move {
                git.fetch_target(input, move |launch| {
                    guard_ticket.with_current(|| {
                        guard_store.store(|s| {
                            let now =
                                s.preflight_target_refresh(&guard_run, supersedes.as_ref())?;
                            if now != current
                                || fetch_proof(s, &guard_run)? != proof
                                || array(&now["refreshes"])
                                    .iter()
                                    .any(|r| r["requestId"] == key)
                            {
                                return Err(conflict(
                                    "Target admission changed before fetching its snapshot.",
                                ));
                            }
                            launch()
                        })
                    })
                })
                .await
            },
        )
        .await?;
        ticket.check()?;
        Ok(Some(destination))
    }
    pub async fn verify_cancelled_continuation(&self, run: &str) -> Result<()> {
        let id = run.to_owned();
        let this = self.clone();
        self.operation(run,move |ticket|async move {
            let Some(record)=this.store(|s|s.continuation_publication(&id))? else {return Ok(())};
            let connection=this.matched(&record).ok_or_else(||conflict("Continuation held: the recorded PR cannot be verified."))?;
            let pr_id=record["pr"]["id"].as_i64().ok_or_else(||conflict("Continuation held: the recorded PR cannot be verified."))?;
            let client=connection.host;
            client.verify_repository().await.map_err(|_|conflict("Continuation held: live PR verification is unavailable. Retry when the provider is available."))?;
            ticket.check()?;
            let pr=client.pull_request(pr_id).await.map_err(|_|conflict("Continuation held: live PR verification is unavailable. Retry when the provider is available."))?;
            ticket.with_current(||this.store(|s| {
                if s.continuation_publication(&id)?.as_ref()!=Some(&record) {return Err(conflict("Continuation held: the retained publication changed during verification."));}
                if pr["state"]!="OPEN" || !pr["description"].as_str().is_some_and(|s|s.contains(&format!("Crewroom publication {}",record["id"].as_str().unwrap_or(""))))
                    || pr["source"]["branch"]["name"]!=record["branch"] || pr["destination"]["branch"]["name"]!=record["target"]
                    || pr["source"]["repository"]["uuid"]!=record["repository"]["uuid"] || pr["destination"]["repository"]["uuid"]!=record["repository"]["uuid"]
                    || pr["source"]["commit"]["hash"]!=record["revision"]["head"] {
                    return Err(conflict("Continuation held: the PR is closed, merged or its branch identity changed."));
                }
                Ok(())
            }))
        }).await
    }
    pub async fn stop_run(&self, run: &str) -> Result<()> {
        self.0.operations.stop(Some(run)).await
    }
    pub async fn close(&self) -> Result<()> {
        self.0.operations.stop(None).await
    }
}
pub(crate) fn retained_target(work: &Value) -> bool {
    work.get("continuationTargetRef")
        .unwrap_or(&work["targetRef"])
        .as_str()
        .is_some_and(|s| s.starts_with("refs/crewroom/"))
        || array(&work["refreshes"]).iter().any(|r| {
            r["targetRef"]
                .as_str()
                .is_some_and(|s| s.starts_with("refs/crewroom/"))
        })
}
fn fetch_proof(s: &crewroom_core::store::Store, run: &str) -> Result<Value> {
    let value = s.get_run(run)?;
    if value["desiredState"] == "cancelled" {
        return Err(conflict("A cancelled run cannot fetch a new target."));
    }
    Ok(
        json!({"controlRevision":value["controlRevision"],"desiredState":value["desiredState"],"sourceConflict":value["sourceConflict"],"taskSnapshot":value["taskSnapshot"],"coding":s.coding_record(run)?}),
    )
}
