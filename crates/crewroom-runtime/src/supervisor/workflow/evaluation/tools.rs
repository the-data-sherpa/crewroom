use super::super::tools::Caller;
use super::*;
use crate::repository::request;

impl<S: StoreAccess> Workflow<S> {
    pub(super) fn evaluation_source(&self, run: &str) -> Result<Value> {
        let root = self.evaluation_root(run)?.join("workspace");
        if !root.exists() {
            return Ok(json!({"sha256":digest(&json!([])),"files":[]}));
        }
        fn visit(root: &Path, dir: &Path, entries: &mut Vec<Value>) -> Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let meta = entry.file_type()?;
                if meta.is_symlink() {
                    return Err(conflict("Evaluation source contains a link."));
                }
                if meta.is_dir() {
                    visit(root, &path, entries)?;
                } else {
                    let name = path
                        .strip_prefix(root)
                        .map_err(|_| conflict("Source escaped workspace."))?
                        .to_str()
                        .ok_or_else(|| conflict("Invalid source path."))?;
                    let (bytes, _) = files::regular_bytes(root, name, 1_000_000)?;
                    entries.push(json!({"path":name,"sha256":files::hash(&bytes)}));
                    if entries.len() > 200 {
                        return Err(conflict("Evaluation source file limit exceeded."));
                    }
                }
            }
            Ok(())
        }
        let mut entries = Vec::new();
        visit(&root, &root, &mut entries)?;
        entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Ok(json!({"sha256":digest(&json!(entries)),"files":entries}))
    }
    fn evaluation_replay(&self, c: &Caller, kind: &str) -> Result<Option<Value>> {
        let link = self.evaluation_link(text(&c.run, "id")?)?;
        let key = format!("{}:{}", c.attempt, c.key);
        if let Some(event) = array(&link["cell"]["events"])
            .iter()
            .find(|e| e["requestId"] == key)
        {
            if event["type"] != kind || event["payload"]["arguments"] != c.args {
                return Err(conflict("Tool replay key belongs to different inputs."));
            }
            return Ok(Some(event["payload"]["receipt"].clone()));
        }
        Ok(None)
    }
    fn evaluation_tool_event(&self, c: &Caller, kind: &str, receipt: Value) -> Result<Value> {
        let run = text(&c.run, "id")?;
        let link = self.evaluation_link(run)?;
        self.store(|s|s.append_comparison_event(text(&link["cell"],"id")?,&json!({"requestId":format!("{}:{}",c.attempt,c.key),"type":kind,"attemptId":c.attempt,"payload":{"arguments":c.args,"receipt":receipt}})))?;
        Ok(receipt)
    }
    pub(in crate::supervisor::workflow) async fn evaluation_tool(
        &self,
        route: &str,
        c: &Caller,
    ) -> Result<Value> {
        let run = text(&c.run, "id")?;
        let link = self.evaluation_link(run)?;
        let case = Self::evaluation_case(&link)?;
        let root = self.evaluation_root(run)?;
        let workspace = root.join("workspace");
        let mutation = match route {
            "evaluation/write" | "evaluation/edit" => Some("edit"),
            "evaluation/check" => Some("check"),
            "evaluation/handoff" => Some("handoff"),
            "evaluation/playbook-step" => Some("playbook"),
            _ => None,
        };
        if let Some(kind) = mutation {
            if let Some(receipt) = self.evaluation_replay(c, kind)? {
                return Ok(if kind == "check" {
                    check_view(receipt)
                } else {
                    receipt
                });
            }
            if link["cell"]["result"].is_object()
                || array(&link["cell"]["events"])
                    .iter()
                    .any(|event| matches!(event["type"].as_str(), Some("handoff" | "finalization")))
            {
                return Err(conflict(
                    "The worker's final handoff sealed this evaluation; no new work is accepted.",
                ));
            }
        }
        match route {
            "evaluation/read" => {
                request(&c.args, &["path"])?;
                let public_checks = array(&case["checkFiles"]).iter().collect::<Vec<_>>();
                if let Some(path) = c.args["path"].as_str() {
                    if let Some(name) = path.strip_prefix(".checks/") {
                        let file = public_checks
                            .iter()
                            .find(|file| file["path"] == name)
                            .ok_or_else(|| conflict("Unknown protected check source."))?;
                        return Ok(
                            json!({"path":path,"content":file["content"],"hash":file["sha256"],"readOnly":true}),
                        );
                    }
                    let content = files::read_text(&workspace, path, 1_000_000)?;
                    Ok(json!({"path":path,"content":content,"hash":files::hash(&content)}))
                } else {
                    Ok(
                        json!({"files":self.evaluation_source(run)?["files"],"requiredChecks":case["requiredChecks"],"checkFiles":public_checks.iter().map(|file|json!({"path":format!(".checks/{}",file["path"].as_str().unwrap_or("")),"hash":file["sha256"],"readOnly":true})).collect::<Vec<_>>(),"priorChecks":array(&link["cell"]["events"]).iter().filter(|e|e["payload"]["prior"]==true).map(|e|check_view(e["payload"]["receipt"].clone())).collect::<Vec<_>>()}),
                    )
                }
            }
            "evaluation/search" => {
                request(&c.args, &["query", "regex", "ignoreCase", "path"])?;
                if c.args["regex"] == true {
                    return Err(conflict("Case search supports literal text."));
                }
                let q = text(&c.args, "query")?;
                if q.is_empty() || q.len() > 1000 {
                    return Err(conflict("Search query is empty or too long."));
                }
                let fold = c.args["ignoreCase"] == true;
                let q = if fold { q.to_lowercase() } else { q.into() };
                let mut found = Vec::new();
                for file in array(&self.evaluation_source(run)?["files"]) {
                    let path = text(file, "path")?;
                    if c.args["path"].as_str().is_some_and(|p| p != path) {
                        continue;
                    }
                    for (line, body) in files::read_text(&workspace, path, 1_000_000)?
                        .lines()
                        .enumerate()
                    {
                        if (if fold {
                            body.to_lowercase()
                        } else {
                            body.into()
                        })
                        .contains(&q)
                        {
                            found.push(json!({"path":path,"line":line+1,"text":body.chars().take(300).collect::<String>()}));
                            if found.len() == 100 {
                                return Ok(json!(found));
                            }
                        }
                    }
                }
                Ok(json!(found))
            }
            "evaluation/write" | "evaluation/edit" => {
                if c.args["path"]
                    .as_str()
                    .is_some_and(|p| p == ".checks" || p.starts_with(".checks/"))
                {
                    return Err(conflict("Protected check sources are read-only."));
                }
                if c.assignment["coordinator"] == true {
                    return Err(conflict("Only the fixed specialist can edit."));
                }
                let path = text(&c.args, "path")?;
                let intent = root.join(format!(
                    "edit-{}.json",
                    files::hash(format!("{}:{}", c.attempt, c.key))
                ));
                if intent.exists() {
                    let saved: Value = serde_json::from_slice(&crate::read(&intent)?)?;
                    if saved["arguments"] != c.args {
                        return Err(conflict("Edit replay changed."));
                    }
                    if files::read_text(&workspace, path, 1_000_000)
                        .is_ok_and(|v| files::hash(v) == saved["sha256"])
                    {
                        return self.evaluation_tool_event(
                            c,
                            "edit",
                            json!({"path":path,"hash":saved["sha256"]}),
                        );
                    }
                }
                let content = if route == "evaluation/write" {
                    request(&c.args, &["path", "expectedHash", "content"])?;
                    text(&c.args, "content")?.to_owned()
                } else {
                    request(&c.args, &["path", "expectedHash", "replacements"])?;
                    let mut content = files::read_text(&workspace, path, 1_000_000)?;
                    for r in array(&c.args["replacements"]) {
                        let old = text(r, "oldText")?;
                        let new = text(r, "newText")?;
                        if old.is_empty() || content.matches(old).count() != 1 {
                            return Err(conflict("Patch must match exactly one source span."));
                        }
                        content = content.replacen(old, new, 1);
                    }
                    content
                };
                if content.len() > 1_000_000 {
                    return Err(conflict("Source file limit exceeded."));
                }
                // Durable intent precedes the atomic file replacement. A reply loss
                // reuses the resulting hash, while a partial application is held.
                let wanted = json!({"path":path,"expectedHash":c.args["expectedHash"],"sha256":files::hash(&content),"arguments":c.args});
                if intent.exists() {
                    let saved: Value = serde_json::from_slice(&crate::read(&intent)?)?;
                    if saved != wanted {
                        return Err(conflict("Edit replay changed."));
                    }
                    if files::read_text(&workspace, path, 1_000_000)
                        .is_ok_and(|v| files::hash(v) == wanted["sha256"])
                    {
                        return self.evaluation_tool_event(
                            c,
                            "edit",
                            json!({"path":path,"hash":wanted["sha256"]}),
                        );
                    }
                } else {
                    crate::durable_new(&intent, stringify(&wanted).as_bytes())?;
                }
                let hash = files::replace(
                    &workspace,
                    &root,
                    path,
                    c.args["expectedHash"].as_str(),
                    &content,
                )?;
                self.evaluation_tool_event(c, "edit", json!({"path":path,"hash":hash}))
            }
            "evaluation/check" => {
                request(&c.args, &["checkId"])?;
                let receipt = self
                    .evaluation_check(run, &c.attempt, &c.key, text(&c.args, "checkId")?, false)
                    .await?;
                self.evaluation_tool_event(c, "check", receipt)
                    .map(check_view)
            }
            "evaluation/handoff" => {
                request(&c.args, &["status", "summary"])?;
                if !matches!(c.args["status"].as_str(), Some("complete" | "incomplete"))
                    || text(&c.args, "summary")?.trim().is_empty()
                {
                    return Err(conflict(
                        "Handoff needs complete or incomplete and an explanation.",
                    ));
                }
                let receipt = json!({"status":c.args["status"],"summary":c.args["summary"],"source":self.evaluation_source(run)?});
                let event = self.store(|store| {
                    store.record_comparison_handoff(text(&link["cell"], "id")?, &json!({
                        "requestId":format!("{}:{}",c.attempt,c.key), "type":"handoff", "attemptId":c.attempt,
                        "payload":{"arguments":c.args,"receipt":receipt}
                    }))
                })?;
                Ok(event["payload"]["receipt"].clone())
            }
            "evaluation/playbook" => {
                request(&c.args, &[])?;
                Ok(
                    json!({"selectionVersion":1,"playbook":c.assignment["brief"]["playbook"],"progress":array(&link["cell"]["events"]).iter().filter(|e|e["type"]=="playbook").collect::<Vec<_>>()}),
                )
            }
            "evaluation/playbook-step" => {
                request(&c.args, &["version", "stepId", "state", "reason"])?;
                if c.args["version"] != 1
                    || !array(&c.assignment["brief"]["playbook"]["steps"])
                        .iter()
                        .any(|s| s["id"] == c.args["stepId"])
                    || !matches!(
                        c.args["state"].as_str(),
                        Some("pending" | "active" | "done" | "skipped")
                    )
                    || c.args["state"] == "skipped"
                        && c.args["reason"]
                            .as_str()
                            .is_none_or(|s| s.trim().is_empty())
                {
                    return Err(conflict("Pinned playbook step or progress is invalid."));
                }
                self.evaluation_tool_event(c, "playbook", c.args.clone())
            }
            _ => Err(conflict("Evaluation capability denied.")),
        }
    }
    pub(super) async fn evaluation_check(
        &self,
        run: &str,
        attempt: &str,
        key: &str,
        check_id: &str,
        oracle: bool,
    ) -> Result<Value> {
        let link = self.evaluation_link(run)?;
        let case = Self::evaluation_case(&link)?;
        let d = array(&case["requiredChecks"])
            .iter()
            .find(|d| d["id"] == check_id)
            .ok_or_else(|| conflict("Unknown configured check."))?;
        let root = self.evaluation_root(run)?;
        let source = self.evaluation_source(run)?;
        let job_root = self
            .0
            .config
            .root
            .join(attempt)
            .join("tools")
            .join(files::hash(key));
        private_directory(&job_root)?;
        let saved = job_root.join("evaluation-check.json");
        let snapshot = job_root.join("source");
        let operation = if saved.exists() {
            serde_json::from_slice::<Value>(&crate::read(&saved)?)?
        } else {
            private_directory(&snapshot)?;
            for file in array(&source["files"]) {
                let name = text(file, "path")?;
                let content = files::read_text(&root.join("workspace"), name, 1_000_000)?;
                if files::hash(&content) != file["sha256"] {
                    return Err(conflict("Source changed during check snapshot."));
                }
                files::replace(&snapshot, &job_root, name, None, &content)?;
            }
            let value = json!({"source":source,"definition":d,"checkId":check_id,"oracle":oracle});
            crate::durable_new(&saved, stringify(&value).as_bytes())?;
            value
        };
        if operation["definition"] != *d || operation["oracle"] != oracle {
            return Err(conflict("Check replay belongs to different inputs."));
        }
        let engine = self
            .0
            .config
            .engine
            .as_ref()
            .ok_or_else(|| conflict("Contained checks unavailable."))?;
        let job = ContainerCommand::new(job_root.clone(), Path::new(text(engine, "socket")?))?;
        let mut script = text(d, "script")?.to_owned();
        if !script.ends_with(".cjs") {
            script.push_str(".cjs");
        }
        if script.contains('/') || script.contains("..") {
            return Err(conflict("Frozen check script is invalid."));
        }
        let current = self.store(|s| s.account_time(run))?;
        let timeout = if !oracle && attempt != run {
            let remaining = current["activeLimitMs"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(current["activeMs"].as_u64().unwrap_or(0));
            if remaining == 0 || current["desiredState"] != "running" {
                return Err(conflict("Evaluation active allowance exhausted or paused."));
            }
            remaining.min(45000)
        } else {
            45000
        };
        let spec = json!({"image":link["manifest"]["execution"]["image"],"command":"/usr/local/bin/node","args":[format!("/opt/crewroom-checks/{script}")],"timeoutMs":timeout,"memoryMiB":1024,"tmpExecutable":true,"readOnlyWorkspace":true,"workspace":snapshot});
        if !job_root.join("container.json").exists() {
            job.start(spec.clone()).await?;
        }
        job.wait().await?;
        let actual = job.receipt().await?;
        for k in ["image", "command", "args", "workspace", "readOnlyWorkspace"] {
            if actual["spec"][k] != spec[k] {
                return Err(conflict("Contained check receipt identity mismatch."));
            }
        }
        let source_intact = array(&operation["source"]["files"]).iter().all(|f| {
            files::regular_bytes(&snapshot, f["path"].as_str().unwrap_or(""), 1_000_000)
                .is_ok_and(|(b, _)| files::hash(b) == f["sha256"])
        });
        let output = text(&actual, "output")?;
        let artifact_path = job_root.join("output-artifact.json");
        let artifact = if artifact_path.exists() {
            serde_json::from_slice::<Value>(&crate::read(&artifact_path)?)?
        } else {
            let a = self.0.services.evidence.artifact(output)?;
            crate::durable_new(&artifact_path, stringify(&a).as_bytes())?;
            a
        };
        let verified = self.0.services.evidence.read_artifact(&artifact)?;
        if verified != output {
            return Err(conflict("Check artifact differs from actual receipt."));
        }
        job.remove().await?;
        if job.inspect().await["state"] != "stopped" {
            return Err(conflict("Check container cleanup is unresolved."));
        }
        Ok(
            json!({"checkId":check_id,"definitionHash":d["definitionHash"],"sourceManifestHash":operation["source"]["sha256"],"started":actual["started"],"completed":actual["completed"],"exitCode":actual["exitCode"],"timedOut":actual["timedOut"],"sourceIntact":source_intact,"output":artifact,"outputText":verified,"outputValid":true,"oracle":oracle,"ownershipStopped":oracle,"environment":{"image":actual["spec"]["image"],"platform":actual["platform"],"network":"none"},"cleanup":{"state":"stopped"}}),
        )
    }
}

fn check_view(mut receipt: Value) -> Value {
    let body = receipt["outputText"].as_str().unwrap_or("");
    let preview = match serde_json::from_str::<Value>(body.trim()) {
        Ok(mut parsed) => {
            if let Some(items) = parsed["observations"].as_array_mut() {
                for item in items {
                    if let Some(screenshot) = item["screenshot"].as_object_mut() {
                        screenshot.remove("base64");
                    }
                }
            }
            stringify(&parsed)
        }
        Err(_) => body.chars().take(8000).collect(),
    };
    remove(
        &mut receipt,
        &["outputText", "outputValid", "oracle", "ownershipStopped"],
    );
    receipt["outputPreview"] = json!(preview);
    receipt
}
