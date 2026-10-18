//! Private daemon composition. No credential or runner configuration enters an API projection.
use crate::scheduler::Scheduler;
use crate::{Access, array};
use crewroom_core::{Error, Result, authority::Authority};
use crewroom_runtime::{
    evidence::{Checks, Evidence, Research},
    jira_delivery::JiraDelivery,
    jira_sync::JiraSync,
    messaging::StoreAccess,
    providers::{
        bitbucket::BitbucketClient, config, jira::JiraClient, remote::UnavailablePublication,
    },
    publication::Publication,
    publication_remote::{Connection, PublicationRemote},
    repository::Repository,
    supervisor::{
        tasks::RunTasks,
        workflow::{Services, Workflow, WorkflowOptions, configuration::RuntimeConfig},
    },
    target_refresh::TargetRefresh,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::watch;

pub struct ServiceOptions {
    pub runtime: Option<Value>,
    pub helper: PathBuf,
    pub host: PathBuf,
    pub tmux: PathBuf,
    pub jira: Option<JiraClient>,
    pub publication: Vec<Connection>,
    /// Configured repositories whose publish remote has no publication provider.
    pub publication_unavailable: Vec<UnavailablePublication>,
    pub environment: Value,
    /// GitHub API origin and token override; `None` uses github.com and
    /// `CREWROOM_GITHUB_TOKEN`.
    pub github: Option<crate::GitHubOptions>,
}
impl ServiceOptions {
    pub fn from_environment(data: &Path) -> anyhow::Result<Self> {
        let executable = std::env::current_exe()?;
        let bin = executable
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing daemon installation directory"))?;
        let runtime = std::env::var_os("CREWROOM_RUNTIME_CONFIG")
            .map(|path| -> anyhow::Result<Value> {
                use std::os::unix::fs::MetadataExt;
                let meta = std::fs::symlink_metadata(&path)?;
                // SAFETY: geteuid has no preconditions.
                anyhow::ensure!(
                    meta.is_file()
                        && !meta.file_type().is_symlink()
                        && meta.uid() == unsafe { libc::geteuid() }
                        && meta.mode() & 0o077 == 0,
                    "Runtime configuration must be a private file owned by this user."
                );
                Ok(serde_json::from_slice(&std::fs::read(path)?)?)
            })
            .transpose()?;
        let secrets = crewroom_runtime::providers::secrets::SecretStore::at(data);
        let jira = config::load_jira_connection(&data.join("jira.json"), secrets.clone())?
            .map(|c| JiraClient::new(&c.source, c.credentials, None))
            .transpose()
            .map_err(std::io::Error::other)?;
        let configured = config::load_publication_config(&data.join("bitbucket.json"), secrets)?;
        let publication = configured
            .connections
            .into_iter()
            .map(|c| -> Result<Connection> {
                let client = BitbucketClient::new(&c.repository, c.credentials, None)
                    .map_err(std::io::Error::other)?;
                Connection::new(&c.repository_path, &c.repository, client, &c.policy, None)
            })
            .collect::<Result<Vec<_>>>()?;
        let tmux = std::env::var_os("CREWROOM_TMUX")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                    .map(|p| p.join("tmux"))
                    .find(|p| p.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("/usr/bin/tmux"));
        Ok(Self {
            runtime,
            helper: bin.join("crewroom-process"),
            host: bin.join("crewroom-host"),
            tmux,
            jira,
            publication,
            publication_unavailable: configured.unavailable,
            environment: json!(std::env::vars().collect::<std::collections::BTreeMap<_, _>>()),
            github: None,
        })
    }
}
pub(crate) struct Execution {
    pub store: Access,
    pub work: Repository<Access>,
    pub evidence: Evidence<Access>,
    pub research: Research<Access>,
    pub publication: Publication<Access>,
    pub jira: Option<JiraDelivery<Access>>,
    pub sync: Option<JiraSync<Access>>,
    pub target: TargetRefresh<Access>,
    pub workflow: Option<Workflow<Access>>,
    pub scheduler: Option<Scheduler>,
    pub checks: bool,
    pub operations: RunTasks,
    pub ready: AtomicBool,
    pub publication_paths: Vec<PathBuf>,
    pub publication_unavailable: Vec<UnavailablePublication>,
    changes: watch::Sender<u64>,
}
impl Execution {
    pub fn new(
        store: Access,
        authority: Arc<Authority>,
        data: &Path,
        roles: &Path,
        backend: &str,
        options: ServiceOptions,
        changes: watch::Sender<u64>,
    ) -> Result<Arc<Self>> {
        let work = Repository::new(
            store.clone(),
            data.join("worktrees"),
            std::fs::canonicalize(&options.helper)?,
            authority.clone(),
        )?;
        let evidence = Evidence::new(work.clone(), data.join("artifacts"))?;
        let research = Research::new(evidence.clone());
        let publication_paths = options
            .publication
            .iter()
            .map(|c| c.repository_path().to_owned())
            .collect();
        let publication_unavailable = options
            .publication_unavailable
            .iter()
            .map(|u| {
                let mut u = u.clone();
                if let Ok(path) = std::fs::canonicalize(&u.repository_path) {
                    u.repository_path = path;
                }
                u
            })
            .collect();
        let remote = PublicationRemote::with_unavailable(
            store.clone(),
            data.join("publication"),
            std::fs::canonicalize(&options.helper)?,
            authority.clone(),
            options.publication,
            options.publication_unavailable,
        )?;
        remote.install_observers(&work);
        let target = TargetRefresh::new(work.clone(), evidence.clone(), remote.clone());
        let publication = Publication::new(remote, work.clone(), evidence.clone());
        let sync = options
            .jira
            .clone()
            .map(|c| JiraSync::new(store.clone(), c));
        let jira = options.jira.map(|c| {
            JiraDelivery::new(
                store.clone(),
                c,
                Some(evidence.clone()),
                Some(research.clone()),
            )
        });
        let checks = options
            .runtime
            .as_ref()
            .and_then(|v| {
                v["commandEngine"]["socket"].as_str().map(|socket| {
                    (
                        v["root"].as_str().unwrap_or("").to_owned(),
                        socket.to_owned(),
                    )
                })
            })
            .map(|(root, socket)| Checks::new(evidence.clone(), root.into(), socket.into()))
            .transpose()?;
        let callback = Arc::new(OnceLock::<Weak<Self>>::new());
        let link = callback.clone();
        let workflow = options
            .runtime
            .map(|runtime| {
                Workflow::new(
                    store.clone(),
                    authority,
                    WorkflowOptions {
                        runtime: RuntimeConfig::parse(&runtime)?,
                        helper: std::fs::canonicalize(options.helper)?,
                        host: std::fs::canonicalize(options.host)?,
                        tmux: std::fs::canonicalize(options.tmux)?,
                        backend: backend.into(),
                        roles: roles.into(),
                        environment: options.environment,
                    },
                    Services {
                        repository: work.clone(),
                        evidence: evidence.clone(),
                        research: research.clone(),
                        checks: checks.clone(),
                        publication: Some(publication.clone()),
                        jira: jira.clone(),
                    },
                    move |_| {
                        if let Some(execution) = link.get().and_then(Weak::upgrade) {
                            execution.changed();
                        }
                    },
                )
            })
            .transpose()?;
        let push = changes.clone();
        let scheduler = workflow.as_ref().map(|w| {
            Scheduler::new(
                store.clone(),
                work.clone(),
                evidence.clone(),
                w.clone(),
                move || {
                    push.send_modify(|v| {
                        *v = v.saturating_add(1);
                    });
                },
            )
        });
        let result = Arc::new(Self {
            store,
            work,
            evidence,
            research,
            publication,
            jira,
            sync,
            target,
            workflow,
            scheduler,
            checks: checks.is_some(),
            operations: RunTasks::default(),
            ready: AtomicBool::new(false),
            publication_paths,
            publication_unavailable,
            changes,
        });
        let _ = callback.set(Arc::downgrade(&result));
        Ok(result)
    }
    pub fn changed(&self) {
        self.changes.send_modify(|v| {
            *v = v.saturating_add(1);
        });
        if self.ready.load(Ordering::SeqCst)
            && let Some(s) = &self.scheduler
        {
            s.kick();
        }
    }
    pub fn notify_all(&self) -> Result<()> {
        if let Some(workflow) = &self.workflow {
            for run in array(&self.store.with_store(|s| s.snapshot())?["runs"]) {
                if let Some(id) = run["id"].as_str() {
                    workflow.notify(id);
                }
            }
        }
        self.changed();
        Ok(())
    }
    pub async fn initialize(&self) -> Result<()> {
        self.target.reconcile().await?;
        if let Some(workflow) = &self.workflow {
            workflow
                .initialize()?
                .wait()
                .await
                .map_err(|e| Error::Conflict(e.to_string()))?;
        }
        self.ready.store(true, Ordering::SeqCst);
        self.changed();
        Ok(())
    }
    pub fn fence(&self) {
        self.ready.store(false, Ordering::SeqCst);
        self.operations.fence();
        if let Some(s) = &self.scheduler {
            s.fence();
        }
    }
    pub async fn close(&self) -> Result<()> {
        self.fence();
        let mut failures = Vec::new();
        if let Some(w) = &self.workflow {
            match w.close() {
                Ok(t) => match t.wait().await {
                    Ok(v) if v["stopped"] == true => (),
                    Ok(_) => failures.push("Workflow ownership remains unresolved.".into()),
                    Err(e) => failures.push(e.to_string()),
                },
                Err(e) => failures.push(e.to_string()),
            }
        }
        if let Some(s) = &self.scheduler {
            s.drain().await;
        }
        if let Some(j) = &self.sync
            && let Err(e) = j.close().await
        {
            failures.push(e.to_string());
        }
        if let Some(j) = &self.jira
            && let Err(e) = j.close().await
        {
            failures.push(e.to_string());
        }
        if let Err(e) = self.publication.close().await {
            failures.push(e.to_string());
        }
        match self.work.close() {
            Ok(t) => match t.wait().await {
                Ok(v) if v["stopped"] == true => (),
                Ok(_) => failures.push("Repository ownership remains unresolved.".into()),
                Err(e) => failures.push(e.to_string()),
            },
            Err(e) => failures.push(e.to_string()),
        }
        self.operations.drain().await;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Conflict(failures.join(" ")))
        }
    }
}
