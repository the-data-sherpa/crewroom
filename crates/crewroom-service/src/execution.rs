//! Private daemon composition. No credential or runner configuration enters an API projection.
use crate::scheduler::Scheduler;
use crate::{Access, array};
use crewroom_core::{Error, Result, authority::Authority};
use crewroom_runtime::{
    evidence::{Checks, Evidence, Research},
    github_delivery::GitHubDelivery,
    jira_delivery::JiraDelivery,
    jira_sync::JiraSync,
    messaging::StoreAccess,
    model_catalog::{billing, store::Catalog, unconfigured},
    providers::{
        bitbucket::BitbucketClient,
        config,
        github_pulls::GitHubPullClient,
        jira::JiraClient,
        remote::{PublicationProvider, UnavailablePublication},
    },
    publication::Publication,
    publication_remote::{Connection, PublicationRemote, PullRequestHost},
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
    /// Where `runtime` came from, and why it is absent when it failed to load
    /// (FOR-83). Embedding programs that pass `runtime` directly use the default.
    pub runner: crate::runner::RunnerLoad,
}
impl ServiceOptions {
    /// Startup options. A broken `CREWROOM_RUNTIME_CONFIG` fails startup, as
    /// before; a broken Settings → Runner file leaves the runner off with its
    /// reason shown.
    pub fn from_environment(data: &Path) -> anyhow::Result<Self> {
        Self::load(data, true)
    }
    /// Options for a requested reload: any runner configuration problem leaves
    /// the runner off with its reason shown, so the service stays reachable.
    pub fn for_reload(data: &Path) -> anyhow::Result<Self> {
        Self::load(data, false)
    }
    /// The same options with the runner off, recording why.
    pub fn without_runner(mut self, error: String) -> Self {
        self.runtime = None;
        self.runner.error = Some(error);
        self
    }
    fn load(data: &Path, strict: bool) -> anyhow::Result<Self> {
        let executable = std::env::current_exe()?;
        let bin = executable
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing daemon installation directory"))?;
        let (runner, runtime) = crate::runner::load(data, strict)?;
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
                let host: PullRequestHost = match c.provider {
                    PublicationProvider::Bitbucket => {
                        BitbucketClient::new(&c.repository, c.credentials, None)
                            .map_err(std::io::Error::other)?
                            .into()
                    }
                    PublicationProvider::GitHub => {
                        GitHubPullClient::new(&c.repository, c.credentials, None)
                            .map_err(std::io::Error::other)?
                            .into()
                    }
                };
                Connection::new(&c.repository_path, &c.repository, host, &c.policy, None)
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
            runner,
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
    /// GitHub issue delivery for GitHub-sourced tickets (FOR-48).
    pub github: GitHubDelivery<Access>,
    /// Per-project Jira refresh (FOR-53). Always present: without a Jira
    /// connection each project's refresh records that it is not connected.
    pub sync: JiraSync<Access>,
    pub target: TargetRefresh<Access>,
    pub workflow: Option<Workflow<Access>>,
    /// Discovery-only model catalog, present exactly when no runner is configured.
    pub discovery: Option<Catalog>,
    /// Native login locations for billing detection without a runner.
    pub billing: billing::Sources,
    pub scheduler: Option<Scheduler>,
    pub checks: bool,
    pub operations: RunTasks,
    pub ready: AtomicBool,
    pub publication_paths: Vec<PathBuf>,
    pub publication_unavailable: Vec<UnavailablePublication>,
    changes: watch::Sender<u64>,
}
impl Execution {
    #[allow(clippy::too_many_arguments)] // The service-owned GitHub token route stays explicit.
    pub fn new(
        store: Access,
        authority: Arc<Authority>,
        data: &Path,
        roles: &Path,
        backend: &str,
        options: ServiceOptions,
        changes: watch::Sender<u64>,
        github: &Arc<crate::github::GitHub>,
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
        let sync = match options.jira.clone() {
            Some(c) => JiraSync::new(store.clone(), c),
            None => JiraSync::unconnected(store.clone()),
        };
        // A saved filter left in the connection file was the workspace-level
        // refresh setting; it moves into the projects that confirmed its site,
        // or is reported unused (FOR-53).
        sync.offer_workspace_setting()?;
        let jira = options.jira.map(|c| {
            JiraDelivery::new(
                store.clone(),
                c,
                Some(evidence.clone()),
                Some(research.clone()),
            )
        });
        // Always present: a GitHub-sourced ticket's updates are queued even while
        // GitHub is disconnected and stay undelivered with that reason.
        let github_delivery = GitHubDelivery::new(
            store.clone(),
            github.delivery_credentials(),
            github.fixture_origin(),
            Some(evidence.clone()),
            Some(research.clone()),
        );
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
        // Without a runner, installed harnesses still list their models so the
        // operator can record intent. The runner re-checks before admission.
        let discovery = if options.runtime.is_none() {
            let push = changes.clone();
            Some(Catalog::open_with_login(
                &data.join("model-catalog"),
                unconfigured::sources(
                    &std::fs::canonicalize(&options.helper)?,
                    // The service's own PATH; none means no harness is found.
                    options.environment["PATH"].as_str().unwrap_or(""),
                ),
                // FOR-73: the operator's OMP login in the service's HOME.
                crewroom_runtime::model_catalog::login::home(&options.environment),
                move || {
                    push.send_modify(|v| {
                        *v = v.saturating_add(1);
                    });
                },
            )?)
        } else {
            None
        };
        let billing = billing::Sources::native(&options.environment);
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
                        github: Some(github_delivery.clone()),
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
            github: github_delivery,
            sync,
            target,
            workflow,
            discovery,
            billing,
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
        if let Some(c) = &self.discovery
            && let Err(e) = c.close().await
        {
            failures.push(e.to_string());
        }
        if let Err(e) = self.sync.close().await {
            failures.push(e.to_string());
        }
        if let Some(j) = &self.jira
            && let Err(e) = j.close().await
        {
            failures.push(e.to_string());
        }
        if let Err(e) = self.github.close().await {
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
