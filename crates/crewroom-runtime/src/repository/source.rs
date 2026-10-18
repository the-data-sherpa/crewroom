use super::*;
impl<S: StoreAccess> Repository<S> {
    async fn tool_source(
        &self,
        id: &str,
    ) -> Result<(Value, Option<target::resolution::ConflictSource>)> {
        if self.active_conflict(id)?.is_some() {
            let source = self.conflict_work(id, None, false).await?;
            return Ok((source.work.clone(), Some(source)));
        }
        self.no_conflict(id)?;
        let work = self.owned(id)?;
        self.validate(&work).await?;
        Ok((work, None))
    }
    fn tool_source_unchanged(
        &self,
        s: &Store,
        work: &Value,
        scope: Option<&target::resolution::ConflictSource>,
    ) -> Result<()> {
        if let Some(scope) = scope {
            Self::conflict_unchanged(s, scope)?;
        } else if s.repository_work(text(work, "runId")?)?.as_ref() != Some(work) {
            return Err(conflict("Repository source changed during observation."));
        }
        self.metadata(work)?;
        Ok(())
    }
    /// Ad-hoc output is observation only; configured check evidence uses N4's
    /// separate immutable snapshot and receipt protocol.
    pub async fn prepare_shell(&self, attempt_id: &str, input: &Value) -> Result<Value> {
        request(input, &["command", "timeoutMs"])?;
        let command = limited(input, "command", 1, 4000)?;
        let timeout = input
            .get("timeoutMs")
            .map(|v| {
                v.as_f64()
                    .filter(|n| n.fract() == 0.0 && (1000.0..=600000.0).contains(n))
                    .ok_or_else(|| Error::Invalid("Invalid command timeout.".into()))
            })
            .transpose()?
            .unwrap_or(120000.0) as u64;
        let run = self.store(|s| s.get_attempt(attempt_id))?;
        let run_id = text(&run, "runId")?;
        let (work, scope) = self.tool_source(run_id).await?;
        self.store(|s| {
            self.tool_source_unchanged(s,&work,scope.as_ref())?;
            let attempt = s.get_attempt(attempt_id)?;
            let run = s.get_run(run_id)?;
            let runtime = s.runtime(run_id)?;
            let assignment = array(&runtime["assignments"]).iter().find(|a| a["currentAttemptId"] == attempt_id && a.get("retiredAt").is_none())
                .ok_or_else(|| conflict("Only an active specialist may run commands."))?;
            if assignment["coordinator"] == true { return Err(conflict("The coordinator delegates commands to specialists.")); }
            if attempt["ownership"] != "held" || !["idle", "executing"].contains(&attempt["state"].as_str().unwrap_or("")) || run["status"] != "running" || run["desiredState"] != "running"
                { return Err(conflict("Only an active specialist may run commands.")); }
            let record = s.coding_record(run_id)?.unwrap_or(Value::Null);
            let definitions = array(&record["requiredChecks"]);
            let environment = definitions.iter().find(|e| e["required"] == true).or_else(|| definitions.first())
                .ok_or_else(|| conflict("Ad-hoc commands need a configured, contained check environment."))?;
            self.metadata(&work)?;
            let mut spec = json!({"image":environment["image"],"workspace":work["path"],"command":"/bin/sh","args":["-c",command],"timeoutMs":timeout,"readOnlyWorkspace":true});
            for key in ["memoryMiB", "tmpExecutable"] { if let Some(v) = environment.get(key) { spec[key] = v.clone(); } }
            Ok(json!({"spec":spec}))
        })
    }
    pub(crate) fn no_conflict(&self, id: &str) -> Result<()> {
        let work = self.owned(id)?;
        if array(&work["refreshes"]).iter().any(|r| {
            r["state"] == "rebasing"
                || r["resolutionWorkspace"]["legacyOwnershipUnknown"] == true
                || ["preparing", "continuing", "promoting"]
                    .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
                || r["state"] == "conflicted" && r.get("supersededBy").is_none()
        }) {
            return Err(conflict(
                "Git ownership is unverified; retained conflict requires native target-recovery verification.",
            ));
        }
        Ok(())
    }
    pub async fn list_files(&self, id: &str) -> Result<Value> {
        let (work, scope) = self.tool_source(id).await?;
        let files = self.files(&work).await?;
        self.store(|s| self.tool_source_unchanged(s, &work, scope.as_ref()))?;
        Ok(json!(files))
    }
    pub async fn read_file(&self, id: &str, name: &str) -> Result<Value> {
        let (work, scope) = self.tool_source(id).await?;
        files::scoped(self.path(&work)?, name)?;
        if !self
            .files(&work)
            .await?
            .iter()
            .any(|f| f["path"] == name && f["hash"] != "deleted")
        {
            return Err(Error::NotFound(
                "File is absent or ignored by the repository.".into(),
            ));
        }
        let root = self.path(&work)?.to_owned();
        let path = name.to_owned();
        let content =
            tokio::task::spawn_blocking(move || files::read_text(&root, &path, 1024 * 1024))
                .await
                .map_err(|_| conflict("File read interrupted."))??;
        self.store(|s| self.tool_source_unchanged(s, &work, scope.as_ref()))?;
        Ok(json!({"path":name,"content":content,"sha256":hash(&content)}))
    }
    async fn pinned_tree(&self, id: &str, revision: &Value) -> Result<Value> {
        let work = self.owned(id)?;
        self.validate(&work).await?;
        let head = Self::commit_id(text(revision, "head")?)?;
        if self
            .inner
            .git
            .run(
                self.path(&work)?,
                &["rev-parse", &format!("{head}^{{tree}}")],
            )
            .await?
            != text(revision, "tree")?
        {
            return Err(conflict("Candidate tree identity changed."));
        }
        Ok(work)
    }
    fn commit_id(v: &str) -> Result<&str> {
        if !(40..=64).contains(&v.len())
            || !v
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(conflict("Invalid pinned commit identity."));
        }
        Ok(v)
    }
    async fn blob(&self, path: &Path, head: &str, name: &str) -> Result<String> {
        files::name(name)?;
        Self::commit_id(head)?;
        let object = format!("{head}:{name}");
        let type_args = ["cat-file", "-t", &object];
        let size_args = ["cat-file", "-s", &object];
        let (kind, size) = tokio::try_join!(
            self.inner.git.run(path, &type_args),
            self.inner.git.run(path, &size_args)
        )?;
        if kind != "blob" || size.parse::<u64>().ok().is_none_or(|n| n > 1024 * 1024) {
            return Err(conflict("Read one bounded text file from the candidate."));
        }
        Ok(self
            .inner
            .git
            .raw(path, &["cat-file", "blob", &object])
            .await?)
    }
    pub async fn read_revision(&self, id: &str, revision: &Value, name: &str) -> Result<Value> {
        files::name(name)?;
        let work = self.pinned_tree(id, revision).await?;
        let content = self
            .blob(self.path(&work)?, text(revision, "head")?, name)
            .await?;
        Ok(
            json!({"path":name,"content":content,"sha256":hash(&content),"head":revision["head"],"tree":revision["tree"]}),
        )
    }
    pub async fn diff_revision(&self, id: &str, revision: &Value) -> Result<Value> {
        Self::commit_id(text(revision, "base")?)?;
        let work = self.pinned_tree(id, revision).await?;
        let patch = self
            .inner
            .git
            .raw(
                self.path(&work)?,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--no-color",
                    text(revision, "base")?,
                    text(revision, "head")?,
                    "--",
                ],
            )
            .await?;
        if patch.len() > 1024 * 1024 {
            return Err(conflict(
                "The patch exceeds the 1 MB preview limit. Inspect the retained branch locally.",
            ));
        }
        Ok(
            json!({"base":revision["base"],"head":revision["head"],"tree":revision["tree"],"patch":patch}),
        )
    }
    async fn grep(
        &self,
        root: &Path,
        input: &Value,
        head: Option<&str>,
        research: bool,
    ) -> Result<Value> {
        request(input, &["query", "regex", "ignoreCase", "path"])?;
        let query = limited(input, "query", 1, 500)?;
        let flag = |k: &str| -> Result<bool> {
            if let Some(v) = input.get(k) {
                v.as_bool()
                    .ok_or_else(|| Error::Invalid(format!("Invalid {k}.")))
            } else {
                Ok(false)
            }
        };
        let regex = flag("regex")?;
        let ignore = flag("ignoreCase")?;
        let path = input
            .get("path")
            .map(|_| limited(input, "path", 0, 1000))
            .transpose()?;
        if let Some(path) = path.filter(|v| !v.is_empty()) {
            if path.starts_with(':') {
                return Err(conflict("Search paths are plain repository paths."));
            }
            files::name(path.strip_suffix('/').unwrap_or(path))?;
        }
        let mut args = vec![
            "grep",
            "-z",
            "-n",
            "--no-column",
            "--full-name",
            "-I",
            "--no-color",
            "--max-count=101",
            if regex { "-E" } else { "-F" },
        ];
        if ignore {
            args.push("-i");
        }
        if head.is_none() {
            args.push("--untracked");
        }
        args.extend(["-e", query]);
        if let Some(head) = head {
            Self::commit_id(head)?;
            args.push(head);
        }
        args.push("--");
        let pathspec = path
            .filter(|v| !v.is_empty())
            .map(|p| format!(":(literal){p}"));
        if let Some(p) = &pathspec {
            args.push(p);
        }
        let output = match self.inner.git.raw(root, &args).await {
            Ok(v) => v,
            Err(e) if e.code == Some(1) => String::new(),
            Err(_) => return Err(conflict("Search failed. Narrow the query or path.")),
        };
        let lines = output.lines().filter(|s| !s.is_empty()).collect::<Vec<_>>();
        let mut matches = Vec::new();
        let prefix = head.map(|h| format!("{h}:"));
        for line in lines.iter().take(100) {
            let parts = line.splitn(3, '\0').collect::<Vec<_>>();
            if parts.len() != 3 {
                return Err(conflict("Invalid Git search output."));
            }
            let path = prefix
                .as_ref()
                .and_then(|p| parts[0].strip_prefix(p))
                .unwrap_or(parts[0]);
            let line = parts[1]
                .parse::<u64>()
                .map_err(|_| conflict("Invalid Git search output."))?;
            let mut units = 0;
            let excerpt = parts[2]
                .chars()
                .take_while(|c| {
                    units += c.len_utf16();
                    units <= 300
                })
                .collect::<String>();
            matches.push(json!({"path":path,"line":line,"text":excerpt}));
        }
        let mut value = json!({"query":query});
        if !research {
            value["regex"] = json!(regex);
        }
        if let Some(path) = path {
            value["path"] = json!(path);
        }
        if let Some(head) = head {
            value["revision"] = json!(head);
        }
        value["matches"] = json!(matches);
        value["truncated"] = json!(lines.len() > 100);
        Ok(value)
    }
    pub async fn search(&self, id: &str, input: &Value, revision: Option<&Value>) -> Result<Value> {
        let (work, scope) = if let Some(revision) = revision {
            (self.pinned_tree(id, revision).await?, None)
        } else {
            self.tool_source(id).await?
        };
        if revision.is_none() {
            self.files(&work).await?;
        }
        let result = self
            .grep(
                self.path(&work)?,
                input,
                revision.and_then(|v| v["head"].as_str()),
                false,
            )
            .await?;
        if revision.is_none() {
            self.files(&work).await?;
            self.store(|s| self.tool_source_unchanged(s, &work, scope.as_ref()))?;
        }
        Ok(result)
    }
    fn research_source(&self, id: &str) -> Result<Value> {
        self.store(|s| {
            let run = s.get_run(id)?;
            let source = if run["intakeId"].is_string() && run["purpose"] == "grill" {
                s.intake_source_for_run(id)?
            } else {
                run.get("researchSource")
                    .filter(|v| v.is_object())
                    .cloned()
                    .ok_or_else(|| conflict("This research run has no pinned repository."))?
            };
            let snapshot = s.snapshot()?;
            let configured = array(&snapshot["repositories"])
                .iter()
                .find(|r| r["id"] == source["repositoryId"])
                .ok_or_else(|| conflict("The pinned repository checkout is unavailable."))?;
            if fs::canonicalize(text(configured, "path")?)? != Path::new(text(&source, "path")?) {
                return Err(conflict("The pinned repository checkout is unavailable."));
            }
            Self::commit_id(text(&source, "commit")?)?;
            Ok(source.clone())
        })
    }
    pub async fn read_research(&self, id: &str, name: &str) -> Result<Value> {
        files::name(name)?;
        let source = self.research_source(id)?;
        let path = Path::new(text(&source, "path")?);
        let commit = text(&source, "commit")?;
        let object = format!("{commit}:{name}");
        let entry = self
            .inner
            .git
            .run(path, &["ls-tree", commit, "--", name])
            .await?;
        // `ls-tree` succeeds with no entry for an absent path; report it as absent, not as a non-regular file.
        if entry.trim().is_empty() {
            return Err(Error::NotFound(format!(
                "Pinned source path {name} does not exist at {commit}."
            )));
        }
        if !entry.starts_with("100644 blob ") && !entry.starts_with("100755 blob ") {
            return Err(conflict("Pinned source must be a regular text file."));
        }
        let type_args = ["cat-file", "-t", &object];
        let size_args = ["cat-file", "-s", &object];
        let (kind, size) = tokio::try_join!(
            self.inner.git.run(path, &type_args),
            self.inner.git.run(path, &size_args)
        )
        .map_err(|_| {
            Error::NotFound(format!(
                "Pinned source path {name} does not exist at {commit}."
            ))
        })?;
        if kind != "blob" || size.parse::<u64>().ok().is_none_or(|n| n > 1024 * 1024) {
            return Err(conflict(
                "Pinned source must be a text blob smaller than 1 MB.",
            ));
        }
        let content = self
            .inner
            .git
            .raw(path, &["cat-file", "blob", &object])
            .await?;
        if content.contains('\0') {
            return Err(conflict("Pinned source must be text, not binary."));
        }
        if self.research_source(id)? != source {
            return Err(conflict("Research source changed during observation."));
        }
        Ok(json!({"path":name,"content":content,"sha256":hash(&content),"commit":source["commit"]}))
    }
    pub async fn search_research(&self, id: &str, input: &Value) -> Result<Value> {
        let source = self.research_source(id)?;
        let result = self
            .grep(
                Path::new(text(&source, "path")?),
                input,
                Some(text(&source, "commit")?),
                true,
            )
            .await?;
        if self.research_source(id)? != source {
            return Err(conflict("Pinned source changed during search."));
        }
        Ok(result)
    }
    pub async fn pin_research(&self, repository: &Value) -> Result<Value> {
        let checkout = fs::canonicalize(text(repository, "path")?)?;
        if self
            .inner
            .git
            .run(&checkout, &["rev-parse", "--show-toplevel"])
            .await?
            != checkout.to_string_lossy()
        {
            return Err(conflict("Select the repository root checkout."));
        }
        let target = text(repository, "targetBranch")?;
        self.inner
            .git
            .run(
                &checkout,
                &["check-ref-format", &format!("refs/heads/{target}")],
            )
            .await?;
        let commit = match self
            .inner
            .git
            .reference(&checkout, &format!("refs/heads/{target}"))
            .await
        {
            Ok(v) => v,
            Err(_) => {
                self.inner
                    .git
                    .reference(&checkout, &format!("refs/remotes/{target}"))
                    .await?
            }
        };
        Ok(
            json!({"repositoryId":repository["id"],"repositoryRevision":repository.get("revision").cloned().unwrap_or(json!(1)),"path":checkout,"commit":commit}),
        )
    }
}
