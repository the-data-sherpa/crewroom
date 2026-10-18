//! Native managed worktrees and source-scoped tools. Revision evidence is a
//! separate service; a successful read/edit is never a passing check or review.
mod claims;
mod completion;
mod edits;
pub(crate) mod files;
mod git;
mod retained;
mod shutdown;
mod source;
mod target;
use crate::{conflict, messaging::StoreAccess};
use crewroom_core::{
    Error, Result, authority::Authority, requirements_hash, serialized::stringify, store::Store,
};
use files::hash;
use serde_json::{Value, json};
use std::os::unix::fs::MetadataExt;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Mutex as AsyncMutex;
pub(crate) fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .ok_or_else(|| conflict(&format!("Invalid repository field: {k}.")))
}
pub(crate) fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub(crate) fn json_hash(v: &Value) -> String {
    hash(stringify(v))
}
pub(crate) fn request(value: &Value, keys: &[&str]) -> Result<()> {
    if value
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !keys.contains(&k.as_str())))
    {
        return Err(Error::Invalid(
            "Unexpected repository request fields.".into(),
        ));
    }
    Ok(())
}
pub(crate) fn limited<'a>(v: &'a Value, key: &str, min: usize, max: usize) -> Result<&'a str> {
    let s = text(v, key)?;
    if !(min..=max).contains(&s.chars().count()) {
        return Err(Error::Invalid(format!("Invalid {key} length.")));
    }
    Ok(s)
}
struct Inner {
    root: PathBuf,
    git: git::Git,
    operations: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    target_observer: std::sync::RwLock<Option<target::TargetObserver>>,
    continuation_observer: std::sync::RwLock<Option<retained::ContinuationObserver>>,
}
#[derive(Clone)]
pub struct Repository<S: StoreAccess> {
    store: S,
    inner: Arc<Inner>,
}
impl<S: StoreAccess> Repository<S> {
    pub fn new(
        store: S,
        root: PathBuf,
        helper: PathBuf,
        authority: Arc<Authority>,
    ) -> Result<Self> {
        crate::create_root(&root)?;
        let git = git::Git::new(&root, helper, authority)?;
        Ok(Self {
            store,
            inner: Arc::new(Inner {
                root,
                git,
                operations: Mutex::new(HashMap::new()),
                target_observer: std::sync::RwLock::new(None),
                continuation_observer: std::sync::RwLock::new(None),
            }),
        })
    }
    pub(crate) fn store<T>(&self, op: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        self.inner.git.authority.assert_current()?;
        self.store.with_store(op)
    }
    pub(crate) fn lock(&self, id: &str) -> Arc<AsyncMutex<()>> {
        let mut operations = self.inner.operations.lock().unwrap();
        operations.retain(|_, v| v.strong_count() > 0);
        if let Some(lock) = operations.get(id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        operations.insert(id.into(), Arc::downgrade(&lock));
        lock
    }
    fn owned(&self, id: &str) -> Result<Value> {
        self.store(|s| s.repository_work(id))?
            .ok_or_else(|| Error::NotFound("Repository work has not been prepared.".into()))
    }
    fn path<'a>(&self, work: &'a Value) -> Result<&'a Path> {
        Ok(Path::new(text(work, "path")?))
    }
    async fn common(&self, checkout: &Path) -> Result<PathBuf> {
        Ok(fs::canonicalize(
            self.inner
                .git
                .run(
                    checkout,
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                )
                .await?,
        )?)
    }
    async fn branches(&self, checkout: &Path) -> Result<Vec<Value>> {
        Ok(self
            .inner
            .git
            .metadata(checkout, &["worktree", "list", "--porcelain", "-z"])
            .await?
            .split("\0\0")
            .filter(|r| !r.is_empty())
            .map(|r| {
                let mut v = json!({});
                for line in r.split('\0').filter(|s| !s.is_empty()) {
                    if let Some((k, s)) = line.split_once(' ') {
                        v[k] = json!(s);
                    } else {
                        v[line] = json!(true);
                    }
                }
                v
            })
            .collect())
    }
    fn metadata(&self, work: &Value) -> Result<String> {
        let path = self.path(work)?;
        crate::canonical(path)
            .map_err(|_| conflict("Owned worktree path is unavailable or changed."))?;
        let git = path.join(".git");
        let m = fs::symlink_metadata(&git)?;
        if !m.is_file() || m.file_type().is_symlink() {
            return Err(conflict("Owned Git metadata changed."));
        }
        let body = String::from_utf8(crate::read(&git)?)
            .map_err(|_| conflict("Owned Git metadata changed."))?;
        if work.get("gitFile").is_some_and(|v| v != &body) {
            return Err(conflict("Owned Git metadata changed."));
        }
        let target = body
            .strip_prefix("gitdir: ")
            .map(|s| s.strip_suffix('\n').unwrap_or(s))
            .ok_or_else(|| conflict("Owned Git metadata changed."))?;
        if target.contains(['\n', '\r']) {
            return Err(conflict("Owned Git metadata changed."));
        }
        let target = path.join(target);
        let parent = Path::new(text(work, "commonDir")?).join("worktrees");
        if !target.starts_with(&parent) || target == parent || fs::canonicalize(&target)? != target
        {
            return Err(conflict(
                "Owned Git metadata is outside its reserved repository.",
            ));
        }
        Ok(body)
    }
    async fn validate(&self, work: &Value) -> Result<String> {
        self.inner.git.reconcile(text(work, "runId")?).await?;
        let body = self.metadata(work)?;
        let path = self.path(work)?;
        let (branches, common) = tokio::try_join!(
            self.branches(Path::new(text(&work["repository"], "path")?)),
            self.common(path)
        )?;
        if !branches.iter().any(|b| {
            b["worktree"] == text(work, "path").unwrap_or("")
                && b["branch"] == format!("refs/heads/{}", text(work, "branch").unwrap_or(""))
        }) || common != Path::new(text(work, "commonDir")?)
        {
            return Err(conflict("Worktree branch or repository ownership changed."));
        }
        self.metadata(work)?;
        Ok(body)
    }
    async fn files(&self, work: &Value) -> Result<Vec<Value>> {
        let root = self.path(work)?;
        let raw = self
            .inner
            .git
            .metadata(
                root,
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ],
            )
            .await?;
        let mut names = raw
            .split('\0')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        names.dedup();
        let mut files = Vec::new();
        for name in names {
            files::name(name)?;
            let path = root.join(name);
            let mut parent = path.parent().unwrap();
            while parent != root {
                if fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(conflict(
                        "A repository parent directory is a symbolic link.",
                    ));
                }
                parent = parent
                    .parent()
                    .ok_or_else(|| conflict("Repository path escaped the owned worktree."))?;
            }
            let m = match tokio::fs::symlink_metadata(&path).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    files.push(json!({"path":name,"mode":0,"hash":"deleted"}));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let digest = if m.file_type().is_symlink() {
                hash(
                    tokio::fs::read_link(&path)
                        .await?
                        .as_os_str()
                        .as_encoded_bytes(),
                )
            } else {
                if !m.is_file() || m.len() > 50 * 1024 * 1024 {
                    return Err(conflict(
                        "Repository observation requires regular files smaller than 50 MB; submodules need explicit support.",
                    ));
                }
                let root = root.to_owned();
                let name = name.to_owned();
                let (bytes, _) = tokio::task::spawn_blocking(move || {
                    files::regular_bytes(&root, &name, 50 * 1024 * 1024)
                })
                .await
                .map_err(|_| conflict("Source observation was interrupted."))??;
                hash(bytes)
            };
            files.push(json!({"path":name,"mode":m.mode(),"hash":digest}));
        }
        Ok(files)
    }
    async fn instructions(&self, work: &Value, files: &[Value]) -> Result<Value> {
        let mut instructions = Vec::new();
        for file in files {
            let name = text(file, "path")?;
            if ![Some("AGENTS.md"), Some("CLAUDE.md")]
                .contains(&Path::new(name).file_name().and_then(|v| v.to_str()))
                || file["hash"] == "deleted"
            {
                continue;
            }
            let root = self.path(work)?.to_owned();
            let path = name.to_owned();
            let body = tokio::task::spawn_blocking(move || files::read_text(&root, &path, 400000))
                .await
                .map_err(|_| conflict("Instruction read interrupted."))??;
            if body.encode_utf16().count() > 100000 {
                return Err(conflict("Project instructions exceed the admission limit."));
            }
            if hash(&body) != file["hash"] {
                return Err(conflict("Project instructions changed during observation."));
            }
            instructions.push(json!({"path":name,"body":body,"sha256":file["hash"]}));
        }
        Ok(json!(instructions))
    }
    pub(crate) fn current_requirements(s: &Store, work: &Value) -> Result<String> {
        let run = s.get_run(text(work, "runId")?)?;
        Ok(requirements_hash(
            &s.get_task(text(&run, "taskId")?)?,
            (!array(&run["adoptions"]).is_empty())
                .then(|| run["taskRevision"].as_u64())
                .flatten(),
        ))
    }
    async fn revision(&self, work: &Value) -> Result<Value> {
        let root = self.path(work)?;
        let (base, head, tree, porcelain, files) = tokio::try_join!(
            self.inner.git.reference(
                Path::new(text(&work["repository"], "path")?),
                text(work, "targetRef")?
            ),
            self.inner.git.reference(root, "HEAD"),
            self.inner.git.run(root, &["rev-parse", "HEAD^{tree}"]),
            self.inner.git.run(
                root,
                &[
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all",
                    "--ignore-submodules=none"
                ]
            ),
            self.files(work)
        )?;
        let instructions = self.instructions(work, &files).await?;
        self.metadata(work)?;
        let requirements = self.store(|s| Self::current_requirements(s, work))?;
        Ok(
            json!({"base":base,"head":head,"tree":tree,"worktreeHash":json_hash(&json!(files)),"dirty":!porcelain.is_empty(),"requirementsHash":requirements,"instructionsHash":json_hash(&instructions)}),
        )
    }
    pub async fn status(&self, id: &str) -> Result<Value> {
        let work = self.owned(id)?;
        self.validate(&work).await?;
        let mut revision = self.revision(&work).await?;
        let includes = self
            .inner
            .git
            .run(
                self.path(&work)?,
                &[
                    "merge-base",
                    "--is-ancestor",
                    text(&work, "base")?,
                    text(&revision, "head")?,
                ],
            )
            .await
            .is_ok();
        self.metadata(&work)?;
        self.store(|s|{revision["requirementsHash"]=json!(Self::current_requirements(s,&work)?);let prior=array(&work["checkpoints"]).last().map(|p|&p["revision"]);
            let drift=json!({"base":revision["base"]!=work["base"] || !includes,"head":prior.is_some_and(|p|p["head"]!=revision["head"]),"worktree":prior.is_some_and(|p|p["worktreeHash"]!=revision["worktreeHash"]),"requirements":revision["requirementsHash"]!=work["requirementsHash"],"instructions":revision["instructionsHash"]!=json_hash(&work["instructions"])});
            let mut status=work.clone();status["revision"]=revision;status["drift"]=drift;Ok(status)
        })
    }
    async fn checkpoint(&self, work: &Value, reason: &str, attempt: Option<&str>) -> Result<Value> {
        self.validate(work).await?;
        let revision = self.revision(work).await?;
        let mut value = json!({"id":uuid::Uuid::new_v4().to_string(),"at":crewroom_core::timestamp(crewroom_core::now_ms()),"reason":reason});
        if let Some(id) = attempt {
            value["attemptId"] = json!(id);
        }
        value["revision"] = revision;
        Ok(value)
    }
    pub async fn dependency_base_hold(
        &self,
        id: &str,
        prepared: Option<&str>,
    ) -> Result<Option<String>> {
        let (run, dependencies, repository) = self.store(|s| {
            let run = s.get_run(id)?;
            let dependencies = s.dependency_merges(&run)?;
            let repository = array(&s.snapshot()?["repositories"])
                .iter()
                .find(|r| r["id"] == run["taskSnapshot"]["repositoryId"])
                .cloned();
            Ok((run, dependencies, repository))
        })?;
        let _ = run;
        if array(&dependencies).is_empty() {
            return Ok(None);
        }
        let Some(repository) = repository else {
            return Ok(Some("Selected repository unavailable".into()));
        };
        let target = text(&repository, "targetBranch")?;
        let checkout = match fs::canonicalize(text(&repository, "path")?) {
            Ok(v) => v,
            Err(_) => {
                return Ok(Some(format!(
                    "Update target branch {target} in the checkout: repository unavailable"
                )));
            }
        };
        let base = if let Some(v) = prepared {
            v.into()
        } else {
            match self
                .inner
                .git
                .reference(&checkout, &format!("refs/heads/{target}"))
                .await
            {
                Ok(v) => v,
                Err(_) => match self
                    .inner
                    .git
                    .reference(&checkout, &format!("refs/remotes/{target}"))
                    .await
                {
                    Ok(v) => v,
                    Err(_) => {
                        return Ok(Some(format!(
                            "Update target branch {target} in the checkout: target unavailable"
                        )));
                    }
                },
            }
        };
        for dependency in array(&dependencies) {
            let commit = text(dependency, "commit")?;
            if self
                .inner
                .git
                .run(&checkout, &["merge-base", "--is-ancestor", commit, &base])
                .await
                .is_err()
            {
                return Ok(Some(format!(
                    "{}: verified merge {} is not in prepared base {target}; update the target branch in this checkout",
                    text(dependency, "key")?,
                    &commit[..commit.len().min(12)]
                )));
            }
        }
        Ok(None)
    }
    pub async fn prepare(&self, id: &str, input: &Value) -> Result<Value> {
        let link = self.store(|s| Ok(s.get_run(id)?.get("continuedFrom").cloned()))?;
        let mut ids = vec![id.to_owned()];
        if let Some(link) = &link {
            let source = text(link, "runId")?;
            if source == id {
                return Err(conflict(
                    "Continuation held: source lineage contains a cycle.",
                ));
            }
            ids.push(source.to_owned());
        }
        ids.sort();
        let locks = ids.iter().map(|id| self.lock(id)).collect::<Vec<_>>();
        let mut guards = Vec::new();
        for lock in &locks {
            guards.push(lock.lock().await);
        }
        if self.store(|s| Ok(s.get_run(id)?.get("continuedFrom").cloned()))? != link {
            return Err(conflict(
                "Continuation held: source identity changed during admission.",
            ));
        }
        self.prepare_unlocked(id, input).await
    }
    async fn prepare_unlocked(&self, id: &str, input: &Value) -> Result<Value> {
        let mode = text(input, "mode")?;
        match mode {
            "new" => request(input, &["mode"])?,
            "continue" => {
                request(input, &["mode", "branch", "expectedHead"])?;
                if limited(input, "branch", 1, 200)?.starts_with('-') {
                    return Err(conflict("Use a branch name, not a Git option."));
                }
                let head = text(input, "expectedHead")?;
                if !(40..=64).contains(&head.len())
                    || !head
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(conflict("Invalid expectedHead."));
                }
            }
            _ => return Err(conflict("Invalid repository selection.")),
        };
        let run = self.store(|s| s.get_run(id))?;
        if array(&self.store(|s| s.runtime(id))?["assignments"]).is_empty() {
            let preflight = self.store(|s| {
                if let Some(reason) = s.implementation_plan_hold(&run)? {
                    return Err(conflict(&reason));
                }
                let task = s.get_task(text(&run, "taskId")?)?;
                s.prepare_plan_source(text(&task, "id")?, task["revision"].as_u64().unwrap())
            })?;
            let pinned = tokio::task::spawn_blocking(move || preflight.validate())
                .await
                .map_err(|_| conflict("Plan source validation interrupted."))??;
            self.store(|s| {
                s.save_plan_source(&pinned)?;
                if let Some(reason) = s.implementation_plan_hold(&s.get_run(id)?)? {
                    return Err(conflict(&reason));
                }
                Ok(())
            })?;
        }
        self.inner.git.reconcile(id).await?;
        if run["purpose"].as_str().unwrap_or("coding") != "coding" {
            return Err(conflict("Refinement runs cannot prepare repository work."));
        }
        if run.get("continuedFrom").is_some() {
            return self.prepare_cancelled_unlocked(id, input, &run).await;
        }
        let mut work = if let Some(work) = self.store(|s| s.repository_work(id))? {
            if work["mode"] != mode
                || mode == "continue"
                    && (work["branch"] != input["branch"]
                        || work["startHead"] != input["expectedHead"])
            {
                return Err(conflict(
                    "Run already has a different repository selection.",
                ));
            }
            if work["state"] == "ready" {
                self.validate(&work).await?;
                return Ok(work);
            }
            work
        } else {
            let mut repo = self
                .store(|s| {
                    Ok(array(&s.snapshot()?["repositories"])
                        .iter()
                        .find(|r| r["id"] == run["taskSnapshot"]["repositoryId"])
                        .cloned())
                })?
                .ok_or_else(|| Error::NotFound("Selected repository is unavailable.".into()))?;
            let checkout=fs::canonicalize(text(&repo,"path")?).map_err(|_|conflict("Selected repository checkout is unavailable. Restore access, then Save launch details to retry."))?;
            if self
                .inner
                .git
                .run(&checkout, &["rev-parse", "--show-toplevel"])
                .await?
                != checkout.to_string_lossy()
            {
                return Err(conflict("Select the repository root checkout."));
            }
            repo["path"] = json!(checkout);
            let common = self.common(&checkout).await?;
            let target = text(&repo, "targetBranch")?;
            self.inner
                .git
                .run(
                    &checkout,
                    &["check-ref-format", &format!("refs/heads/{target}")],
                )
                .await?;
            let mut target_ref = format!("refs/heads/{target}");
            if self
                .inner
                .git
                .reference(&checkout, &target_ref)
                .await
                .is_err()
            {
                target_ref = format!("refs/remotes/{target}");
            }
            let base = self.inner.git.reference(&checkout, &target_ref).await?;
            if let Some(hold) = self.dependency_base_hold(id, Some(&base)).await? {
                return Err(conflict(&hold));
            }
            let branch = if mode == "new" {
                format!(
                    "{}{id}",
                    repo["branchPrefix"].as_str().unwrap_or("codex/crewroom/")
                )
            } else {
                text(input, "branch")?.into()
            };
            self.inner
                .git
                .run(
                    &checkout,
                    &["check-ref-format", &format!("refs/heads/{branch}")],
                )
                .await?;
            if self
                .branches(&checkout)
                .await?
                .iter()
                .any(|b| b["branch"] == format!("refs/heads/{branch}"))
            {
                return Err(conflict(
                    "Selected branch is already checked out. Inspect and reconcile its owner, then Save launch details to retry.",
                ));
            }
            let start = if mode == "continue" {
                let actual = self
                    .inner
                    .git
                    .reference(&checkout, &format!("refs/heads/{branch}"))
                    .await?;
                if actual != text(input, "expectedHead")? {
                    return Err(conflict(
                        "Selected branch head changed. Refresh before continuing.",
                    ));
                }
                actual
            } else {
                if self
                    .inner
                    .git
                    .reference(&checkout, &format!("refs/heads/{branch}"))
                    .await
                    .is_ok()
                {
                    return Err(conflict(
                        "Generated branch already exists without owned state. Inspect and reconcile branch ownership, then Save launch details to retry.",
                    ));
                }
                base.clone()
            };
            let destination = self.inner.root.join(id);
            if fs::symlink_metadata(&destination).is_ok() {
                return Err(conflict(
                    "Worktree destination already exists without owned state. Inspect and reconcile the destination, then Save launch details to retry.",
                ));
            }
            let requirements = requirements_hash(
                &run["taskSnapshot"],
                (!array(&run["adoptions"]).is_empty())
                    .then(|| run["taskRevision"].as_u64())
                    .flatten(),
            );
            self.store(|s|s.reserve_repository_work(&json!({"runId":id,"repository":repo,"commonDir":common,"path":destination,"branch":branch,"targetRef":target_ref,"mode":mode,"base":base,"startHead":start,"requirementsHash":requirements,"state":"preparing","instructions":[],"checkpoints":[]})))?
        };
        let checkout = Path::new(text(&work["repository"], "path")?);
        let pin = format!("refs/crewroom/{id}/start");
        let start = text(&work, "startHead")?;
        match self.inner.git.reference(checkout, &pin).await {
            Ok(value) if value != start => {
                return Err(conflict("Owned admission reference changed."));
            }
            Ok(_) => (),
            Err(_) => {
                self.inner
                    .git
                    .mutate(
                        id,
                        checkout,
                        &["update-ref", &pin, start, &"0".repeat(start.len())],
                        |launch| {
                            self.store(|s| {
                                Self::preparing_store(s, &work)?;
                                launch()
                            })
                        },
                    )
                    .await?;
            }
        };
        if fs::symlink_metadata(self.path(&work)?).is_err() {
            let branch = text(&work, "branch")?;
            if self
                .branches(checkout)
                .await?
                .iter()
                .any(|b| b["branch"] == format!("refs/heads/{branch}"))
            {
                return Err(conflict("Branch became checked out during admission."));
            }
            if mode == "continue"
                && self
                    .inner
                    .git
                    .reference(checkout, &format!("refs/heads/{branch}"))
                    .await?
                    != start
            {
                return Err(conflict("Branch head changed during admission."));
            }
            let path = text(&work, "path")?;
            let args = if mode == "new" {
                vec!["worktree", "add", "-b", branch, path, start]
            } else {
                vec!["worktree", "add", path, branch]
            };
            self.inner
                .git
                .mutate(id, checkout, &args, |launch| {
                    self.store(|s| {
                        Self::preparing_store(s, &work)?;
                        launch()
                    })
                })
                .await?;
        }
        let git_file = self.validate(&work).await?;
        if self.inner.git.reference(self.path(&work)?, "HEAD").await? != start {
            return Err(conflict(
                "Incomplete admission has unexpected commits; inspect before adopting.",
            ));
        }
        let files = self.files(&work).await?;
        let instructions = self.instructions(&work, &files).await?;
        let checkpoint = self.checkpoint(&work, "prepared", None).await?;
        work = self.store(|s| {
            Self::preparing_store(s, &work)?;
            s.confirm_repository_work(id, &git_file, &instructions, &checkpoint)
        })?;
        Ok(work)
    }
    fn preparing_store(s: &Store, work: &Value) -> Result<()> {
        let id = text(work, "runId")?;
        let current = s
            .repository_work(id)?
            .ok_or_else(|| conflict("Repository reservation disappeared."))?;
        let run = s.get_run(id)?;
        if current != *work
            || run["status"] != "awaiting_runner"
            || ["paused", "cancelled"].contains(&run["desiredState"].as_str().unwrap_or(""))
            || Self::current_requirements(s, work)? != text(work, "requirementsHash")?
        {
            return Err(conflict(
                "Repository preparation changed before Git dispatch.",
            ));
        }
        crate::canonical(Path::new(text(&work["repository"], "path")?))?;
        Ok(())
    }
}
