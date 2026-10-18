use crate::{
    Error, Result,
    store::{Store, conflict, id, integer, invalid, request_id, strict, text, trim_text},
};
use rusqlite::params;
use serde_json::{Value, json};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn trimmed<'a>(value: &'a Value, field: &str, label: &str, max: usize) -> Result<&'a str> {
    value[field]
        .as_str()
        .map(trim_text)
        .filter(|s| !s.is_empty() && s.chars().count() <= max)
        .ok_or_else(|| invalid(&format!("{label} is required (up to {max} characters).")))
}
/// Messages use the editor's field labels; the caller prefixes the check's position and ID.
fn validate_check<'a>(
    check: &'a Value,
    ids: &mut std::collections::HashSet<&'a str>,
) -> Result<()> {
    strict(
        check,
        &[
            "id",
            "image",
            "command",
            "args",
            "timeoutMs",
            "memoryMiB",
            "tmpExecutable",
            "required",
        ],
    )?;
    let check_id = check["id"].as_str().unwrap_or("");
    if !(1..=64).contains(&check_id.chars().count())
        || !check_id.as_bytes()[0].is_ascii_lowercase()
        || !check_id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(invalid(
            "Check ID must be 1–64 lowercase letters, digits or hyphens, starting with a letter.",
        ));
    }
    if !ids.insert(check_id) {
        return Err(invalid("Check ID is already used by another check."));
    }
    // Test the prefix before the length so a tag such as alpine:3.20 gets the digest message.
    let image = check["image"].as_str().unwrap_or("");
    if !image.starts_with("sha256:")
        || image.len() != 71
        || !image[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(invalid(
            "Image digest must be sha256: followed by 64 lowercase hex characters.",
        ));
    }
    if !check["command"]
        .as_str()
        .is_some_and(|command| Path::new(command).is_absolute())
    {
        return Err(invalid("Command must be an absolute path."));
    }
    if check["args"].as_array().is_none_or(|a| {
        a.len() > 100
            || a.iter()
                .any(|v| v.as_str().is_none_or(|s| s.chars().count() > 10000))
    }) {
        return Err(invalid(
            "Arguments must be at most 100 lines of up to 10,000 characters each.",
        ));
    }
    integer(check, "timeoutMs", 100, 3_600_000)
        .map_err(|_| invalid("Timeout must be 100–3,600,000 ms."))?;
    if check.get("memoryMiB").is_some() {
        integer(check, "memoryMiB", 128, 8192)
            .map_err(|_| invalid("Memory must be 128–8,192 MiB."))?;
    }
    if check["required"].as_bool().is_none()
        || check.get("tmpExecutable").is_some_and(|v| !v.is_boolean())
    {
        return Err(invalid(
            "Required and Executable temporary files must be true or false.",
        ));
    }
    Ok(())
}
fn update_value(value: &Value) -> Result<Value> {
    strict(
        value,
        &[
            "name",
            "path",
            "branchPrefix",
            "targetBranch",
            "checks",
            "revision",
            "requestId",
        ],
    )?;
    let revision = integer(value, "revision", 1, 9_007_199_254_740_991)?;
    let key = request_id(value)?;
    let mut fields = value.clone();
    fields.as_object_mut().unwrap().remove("revision");
    fields.as_object_mut().unwrap().remove("requestId");
    let mut normalized = configuration(&fields)?;
    normalized["revision"] = json!(revision);
    normalized["requestId"] = json!(key);
    Ok(normalized)
}
/// Validate an operator edit outside the workflow lock; commit rechecks references and revision.
pub fn validate_update(value: &Value) -> Result<ValidatedRepository> {
    let mut fields = update_value(value)?;
    fields.as_object_mut().unwrap().remove("revision");
    validate(&fields)
}

fn configuration(value: &Value) -> Result<Value> {
    strict(
        value,
        &[
            "name",
            "path",
            "branchPrefix",
            "targetBranch",
            "checks",
            "requestId",
        ],
    )?;
    let name = trimmed(value, "name", "Repository name", 120)?;
    let path = value["path"]
        .as_str()
        .ok_or_else(|| invalid("Use an absolute repository path."))?;
    if !Path::new(path).is_absolute() {
        return Err(invalid("Use an absolute repository path."));
    }
    let target = trimmed(value, "targetBranch", "Target branch", 200)?;
    let mut saved = json!({"name":name,"path":path});
    if value.get("branchPrefix").is_some() {
        let prefix = text(value, "branchPrefix", 1, 100)?;
        if !prefix.ends_with('/')
            || !prefix[..prefix.len() - 1].split('/').all(|part| {
                part.as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            })
        {
            return Err(invalid(
                "Use a branch prefix ending in /, with letters, numbers, hyphens or underscores.",
            ));
        }
        saved["branchPrefix"] = json!(prefix);
    }
    saved["targetBranch"] = json!(target);
    if let Some(checks) = value.get("checks") {
        let checks = checks
            .as_array()
            .filter(|c| c.len() <= 100)
            .ok_or_else(|| invalid("Checks must be a list of at most 100 entries."))?;
        let mut ids = std::collections::HashSet::new();
        for (index, check) in checks.iter().enumerate() {
            // Name the check by position and ID so the operator can find the field to fix.
            let label = match check["id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.chars().count() <= 64)
            {
                Some(id) => format!("Check {} ({id})", index + 1),
                None => format!("Check {}", index + 1),
            };
            validate_check(check, &mut ids).map_err(|error| match error {
                Error::Invalid(message) => Error::Invalid(format!("{label}: {message}")),
                other => other,
            })?;
        }
        saved["checks"] = json!(
            checks
                .iter()
                .map(|check| {
                    let mut normalized = json!({});
                    for field in [
                        "id",
                        "image",
                        "command",
                        "args",
                        "timeoutMs",
                        "memoryMiB",
                        "tmpExecutable",
                        "required",
                    ] {
                        if let Some(value) = check.get(field) {
                            normalized[field] = value.clone();
                        }
                    }
                    normalized["timeoutMs"] = json!(
                        integer(check, "timeoutMs", 100, 3_600_000).expect("validated timeout")
                    );
                    if check.get("memoryMiB").is_some() {
                        normalized["memoryMiB"] = json!(
                            integer(check, "memoryMiB", 128, 8192).expect("validated memory")
                        );
                    }
                    normalized
                })
                .collect::<Vec<_>>()
        );
    }
    if value.get("requestId").is_some() {
        saved["requestId"] = json!(request_id(value)?);
    }
    Ok(saved)
}

const UNVERIFIED: &str = "Repository or target branch could not be verified.";
/// Replace the generic Git failure with a field-specific message; timeouts pass through.
fn explain(error: Error, message: &str) -> Error {
    match error {
        Error::Conflict(m) if m == UNVERIFIED => conflict(message),
        other => other,
    }
}

pub(crate) fn git(path: &Path, args: &[&str]) -> Result<String> {
    let mut child = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
        ])
        .arg("-C")
        .arg(path)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped output");
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output.join();
            return Err(conflict(
                "Repository validation timed out. The checkout was not saved.",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = output
        .join()
        .map_err(|_| conflict("Repository validation was interrupted."))??;
    if !status.success() || output.len() > 1_048_576 {
        return Err(conflict(UNVERIFIED));
    }
    Ok(String::from_utf8_lossy(&output).trim_end().to_owned())
}

pub struct ValidatedRepository(Value);

/// Run on the service's blocking worker, never on the render or HTTP event thread.
pub fn validate(value: &Value) -> Result<ValidatedRepository> {
    let mut value = configuration(value)?;
    let path = std::fs::canonicalize(value["path"].as_str().unwrap())
        .map_err(|_| conflict("Repository checkout is unavailable."))?;
    let target = value["targetBranch"].as_str().unwrap();
    let invalid_branch = format!("Target branch '{target}' is not a valid branch name.");
    if target.starts_with('-') {
        return Err(invalid(&invalid_branch));
    }
    let root = git(&path, &["rev-parse", "--show-toplevel"])
        .map_err(|e| explain(e, "Checkout path is not a Git repository root."))?;
    if std::fs::canonicalize(root)? != path {
        return Err(conflict("Select the repository root, not a subdirectory."));
    }
    let metadata = std::fs::symlink_metadata(path.join(".git"))?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(conflict("Repository Git metadata is unsafe."));
    }
    git(
        &path,
        &["check-ref-format", &format!("refs/heads/{target}")],
    )
    .map_err(|e| explain(e, &invalid_branch))?;
    git(
        &path,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("refs/heads/{target}^{{commit}}"),
        ],
    )
    .or_else(|_| {
        git(
            &path,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("refs/remotes/{target}^{{commit}}"),
            ],
        )
    })
    .map_err(|e| explain(e, &format!("Target branch '{target}' was not found.")))?;
    value["path"] = json!(path.to_string_lossy());
    Ok(ValidatedRepository(value))
}

impl Store {
    pub fn is_owned_repository_worktree(&self, path: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM repository_work WHERE json_extract(data,'$.path')=?)",
            [path],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn repository_by_id(&self, id: &str) -> Result<Value> {
        self.record("repositories", "id", id)?
            .ok_or_else(|| Error::NotFound("Repository not found.".into()))
    }
    pub(crate) fn referencing_runs(&self, repository_id: &str) -> Result<Vec<Value>> {
        let mut statement=self.db.prepare("SELECT data FROM runs WHERE status NOT IN ('cancelled','closed','completed') AND json_extract(data,'$.taskSnapshot.repositoryId')=?")?;
        let rows = statement.query_map([repository_id], |r| r.get::<_, String>(0))?;
        rows.map(|row| crate::store::parse(row?)).collect()
    }
    pub(crate) fn refuse_referenced(&self, references: &[Value], action: &str) -> Result<()> {
        if references.is_empty() {
            return Ok(());
        }
        let mut keys = Vec::new();
        for run in references {
            let key = run["taskSnapshot"]["key"].as_str().unwrap_or("");
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        Err(conflict(&format!(
            "Cannot {action} while queued or open work references this repository: {}.",
            keys.join(", ")
        )))
    }
    pub fn repository_update_replay(&self, id: &str, input: &Value) -> Result<Option<Value>> {
        let value = update_value(input)?;
        self.exact_queue_replay(
            value["requestId"].as_str().unwrap(),
            "repository-update",
            &json!({"id":id,"value":value}),
        )
    }
    pub fn update_repository(
        &self,
        id: &str,
        input: &ValidatedRepository,
        update: &Value,
    ) -> Result<Value> {
        let value = update_value(update)?;
        let validated = configuration(&input.0)?;
        self.transaction(|s| {
            let payload=json!({"id":id,"value":value});let key=value["requestId"].as_str().unwrap();
            if let Some(replay)=s.exact_queue_replay(key,"repository-update",&payload)? {return Ok(replay);}
            let current=s.repository_by_id(id)?;
            if current.get("retiredAt").is_some() {return Err(conflict("Retired repositories cannot be edited."));}
            if current["revision"].as_u64().unwrap_or(1)!=value["revision"].as_u64().unwrap() {return Err(conflict("Repository revision changed. Keep your draft and reload the current configuration before saving."));}
            if s.all("repository_work")?.iter().any(|w|w["path"]==validated["path"]) {return Err(conflict("A Crewroom-owned linked worktree cannot be configured as a repository."));}
            let path=validated["path"].as_str().unwrap();
            if s.all("repositories")?.iter().filter(|r|r["id"]!=id && r.get("retiredAt").is_none()).any(|r| {
                let saved=r["path"].as_str().unwrap();let canonical=std::fs::canonicalize(saved).ok();canonical.as_ref().map(|p|p.to_string_lossy()).as_deref().unwrap_or(saved)==path && r["targetBranch"]==validated["targetBranch"]
            }) {return Err(conflict("This repository path and target branch are already configured."));}
            let references=s.referencing_runs(id)?;
            let before=current["checks"].as_array().map(Vec::as_slice).unwrap_or(&[]);let after=validated["checks"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let removed:Vec<_>=before.iter().filter(|c|!after.iter().any(|n|n["id"]==c["id"])).map(|c|&c["id"]).collect();
            let referenced=references.iter().any(|r|r["launch"]["team"].as_array().is_some_and(|team|team.iter().any(|member|member["criteria"].as_array().is_some_and(|criteria|criteria.iter().any(|c|c["checkIds"].as_array().is_some_and(|checks|checks.iter().any(|id|removed.contains(&id))))))));
            if current["path"]!=validated["path"] || current["targetBranch"]!=validated["targetBranch"] || referenced {s.refuse_referenced(&references,"change checkout, target branch or referenced checks")?;}
            let mut saved=json!({"id":current["id"]});for field in ["requestId","guidance","ticketSource","ticketSourceHistory"] {if let Some(v)=current.get(field) {saved[field]=v.clone();}}
            for (field,v) in validated.as_object().unwrap() {if field!="requestId" {saved[field]=v.clone();}}
            saved["revision"]=json!(current["revision"].as_u64().unwrap_or(1)+1);
            s.db.execute("UPDATE repositories SET data=? WHERE id=?",params![saved.to_string(),id])?;s.receipt(key,"repository-update",&payload,&saved)?;Ok(saved)
        })
    }
    /// Replay before Git, including schema-26 repositories predating queue receipts.
    pub fn repository_replay(&self, value: &Value) -> Result<Option<Value>> {
        let mut canonical = configuration(value)?;
        if canonical.get("requestId").is_none() {
            return Ok(None);
        }
        if let Ok(path) = std::fs::canonicalize(canonical["path"].as_str().unwrap()) {
            canonical["path"] = json!(path.to_string_lossy());
        }
        self.repository_creation_replay(&canonical)
    }
    fn repository_creation_replay(&self, value: &Value) -> Result<Option<Value>> {
        let Some(key) = value["requestId"].as_str() else {
            return Ok(None);
        };
        if let Some(prior) = self.replay(key, "repository-create", value)? {
            return Ok(Some(prior));
        }
        for mut prior in self.all("repositories")? {
            if prior["requestId"] != key {
                continue;
            }
            let original = prior.clone();
            for field in [
                "id",
                "guidance",
                "revision",
                "retiredAt",
                "ticketSource",
                "ticketSourceHistory",
            ] {
                prior.as_object_mut().unwrap().remove(field);
            }
            if !crate::repository_work::same(Some(&prior), Some(value)) {
                return Err(conflict(
                    "Repository requestId was already used for different configuration.",
                ));
            }
            return Ok(Some(original));
        }
        Ok(None)
    }
    pub fn create_repository(&self, value: &Value) -> Result<Value> {
        if let Some(prior) = self.repository_replay(value)? {
            return Ok(prior);
        }
        self.save_repository(&validate(value)?)
    }
    pub fn save_repository(&self, validated: &ValidatedRepository) -> Result<Value> {
        let validated = configuration(&validated.0)?;
        self.transaction(|s| {
            if let Some(prior) = s.repository_creation_replay(&validated)? {
                return Ok(prior);
            }
            if s.is_owned_repository_worktree(validated["path"].as_str().unwrap())? {
                return Err(conflict(
                    "A Crewroom-owned linked worktree cannot be configured as a repository.",
                ));
            }
            let path = validated["path"].as_str().unwrap();
            if s.all("repositories")?.iter().any(|r| {
                let saved = r["path"].as_str().unwrap();
                let canonical = std::fs::canonicalize(saved).ok();
                r.get("retiredAt").is_none()
                    && canonical
                        .as_ref()
                        .map(|p| p.to_string_lossy())
                        .as_deref()
                        .unwrap_or(saved)
                        == path
                    && r["targetBranch"] == validated["targetBranch"]
            }) {
                return Err(conflict(
                    "This repository path and target branch are already configured.",
                ));
            }
            let mut saved = validated.clone();
            saved["id"] = json!(id());
            saved["revision"] = json!(1);
            s.db.execute(
                "INSERT INTO repositories VALUES(?,?)",
                params![saved["id"].as_str(), saved.to_string()],
            )?;
            if let Some(key) = validated["requestId"].as_str() {
                s.receipt(key, "repository-create", &validated, &saved)?;
            }
            Ok(saved)
        })
    }
    pub fn retire_repository(&self, repo_id: &str, body: &Value) -> Result<Value> {
        strict(body, &["revision", "requestId"])?;
        let revision = integer(body, "revision", 1, 9_007_199_254_740_991)?;
        let key = request_id(body)?;
        let payload = json!({"id":repo_id,"value":{"revision":revision,"requestId":key}});
        self.transaction(|s| {
            if let Some(prior) = s.exact_queue_replay(key, "repository-retire", &payload)? {
                return Ok(prior);
            }
            let mut repo = s.repository_by_id(repo_id)?;
            if repo["revision"].as_u64().unwrap_or(1) != revision {
                return Err(conflict(
                    "Repository revision changed. Reload the current configuration before retiring.",
                ));
            }
            if repo.get("retiredAt").is_some() {
                return Err(conflict("Repository is already retired."));
            }
            s.refuse_referenced(&s.referencing_runs(repo_id)?,"retire")?;
            repo["revision"] = json!(revision + 1);
            repo["retiredAt"] = json!(s.time());
            s.db.execute(
                "UPDATE repositories SET data=? WHERE id=?",
                params![repo.to_string(), repo_id],
            )?;
            s.receipt(key, "repository-retire", &payload, &repo)?;
            Ok(repo)
        })
    }
}

/// Pin read-only research to the configured target, without creating a worktree.
pub(crate) fn pin_research(repository: &Value) -> Result<Value> {
    let path = std::fs::canonicalize(repository["path"].as_str().unwrap())
        .map_err(|_| conflict("Selected research repository checkout is unavailable. Restore it before queuing research."))?;
    if std::fs::canonicalize(git(&path, &["rev-parse", "--show-toplevel"])?)? != path {
        return Err(conflict("Select the repository root checkout."));
    }
    let metadata = std::fs::symlink_metadata(path.join(".git"))?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(conflict("Repository Git metadata is unsafe."));
    }
    let target = repository["targetBranch"]
        .as_str()
        .ok_or_else(|| invalid("Invalid target branch."))?;
    git(
        &path,
        &["check-ref-format", &format!("refs/heads/{target}")],
    )?;
    let commit = git(
        &path,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("refs/heads/{target}^{{commit}}"),
        ],
    )
    .or_else(|_| {
        git(
            &path,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("refs/remotes/{target}^{{commit}}"),
            ],
        )
    })?;
    Ok(
        json!({"repositoryId":repository["id"],"repositoryRevision":repository["revision"].as_u64().unwrap_or(1),"path":path,"commit":commit}),
    )
}
