use super::*;
use crate::repositories::{git, pin_research};
use std::path::Path;

/// Capture under the workflow lock; validate Git without holding that lock.
pub struct SourcePreflight {
    task: Value,
    repository: Value,
}
/// A server-produced proof. HTTP and agent callers cannot manufacture one.
pub struct PinnedSource {
    preflight: SourcePreflight,
    source: Value,
}
impl SourcePreflight {
    pub fn validate(self) -> Result<PinnedSource> {
        let mut source = pin_research(&self.repository)?;
        let path = Path::new(source["path"].as_str().unwrap());
        let commit = source["commit"].as_str().unwrap();
        let tree = git(path, &["rev-parse", &format!("{commit}^{{tree}}")])?;
        let files = git(path, &["ls-tree", "-rz", "--full-tree", commit])?;
        let mut instructions = Vec::new();
        let mut instruction_bytes = 0;
        for file in files.split('\0').filter(|f| !f.is_empty()) {
            let Some((metadata, name)) = file.split_once('\t') else {
                return Err(conflict("Invalid pinned tree."));
            };
            if ![Some("AGENTS.md"), Some("CLAUDE.md")]
                .contains(&Path::new(name).file_name().and_then(|v| v.to_str()))
            {
                continue;
            }
            if !metadata.starts_with("100644 blob ") && !metadata.starts_with("100755 blob ") {
                return Err(conflict("Pinned instructions must be regular text files."));
            }
            let body = git(path, &["cat-file", "blob", &format!("{commit}:{name}")])?;
            instruction_bytes += body.len();
            if body.contains('\0') || instruction_bytes > 100_000 || instructions.len() >= 100 {
                return Err(conflict("Pinned instructions exceed the text limit."));
            }
            instructions.push(json!({"path":name,"body":body,"sha256":hash(&json!(body))}));
        }
        source["tree"] = json!(tree);
        source["instructions"] = json!(instructions);
        source["instructionsHash"] = json!(hash(&source["instructions"]));
        source["guidance"] = self.repository["guidance"].clone();
        source["taskRevision"] = self.task["revision"].clone();
        source["sourceRevision"] = self.task["sourceRevision"].clone();
        source["requirementsHash"] = json!(crate::requirements_hash(&self.task, None));
        source["id"] = json!(hash(&source));
        Ok(PinnedSource {
            preflight: self,
            source,
        })
    }
}
impl Store {
    pub fn prepare_plan_source(&self, task_id: &str, revision: u64) -> Result<SourcePreflight> {
        let task = self.get_task(task_id)?;
        if task["revision"] != revision || task["needsReacceptance"] == true {
            return Err(conflict(
                "Accept the current requirements before inspecting the plan source.",
            ));
        }
        let repository = self.active_repository(
            task["repositoryId"]
                .as_str()
                .ok_or_else(|| conflict("Select a repository for intake."))?,
        )?;
        Ok(SourcePreflight { task, repository })
    }
    pub fn save_plan_source(&self, pinned: &PinnedSource) -> Result<Value> {
        self.transaction(|s| {
            let task_id = pinned.preflight.task["id"].as_str().unwrap();
            let current = s.prepare_plan_source(task_id, pinned.preflight.task["revision"].as_u64().unwrap())?;
            if current.task != pinned.preflight.task || current.repository != pinned.preflight.repository {
                return Err(conflict("Task or repository changed during source inspection. Retry with the current revision."));
            }
            s.db.execute("INSERT INTO plan_sources VALUES(?,?) ON CONFLICT(task_id) DO UPDATE SET data=excluded.data", params![task_id,pinned.source.to_string()])?;
            Ok(pinned.source.clone())
        })
    }
    pub fn plan_source(&self, task_id: &str) -> Result<Option<Value>> {
        self.record("plan_sources", "task_id", task_id)
    }
    pub(crate) fn source_hold(&self, task: &Value, basis: &Value) -> Result<Option<String>> {
        if task["needsReacceptance"] == true
            || basis["requirementsHash"] != crate::requirements_hash(task, None)
        {
            return Ok(Some(
                "Requirements changed; revalidate the implementation plan.".into(),
            ));
        }
        let Some(repo_id) = task["repositoryId"].as_str() else {
            return Ok(Some("Select a repository.".into()));
        };
        let repo = match self.active_repository(repo_id) {
            Ok(r) => r,
            Err(_) => return Ok(Some("Selected repository unavailable.".into())),
        };
        if basis["repositoryId"] != repo_id
            || basis["repositoryRevision"] != repo["revision"].as_u64().unwrap_or(1)
            || basis["guidance"] != repo["guidance"]
        {
            return Ok(Some(
                "Repository configuration or instructions changed; revalidate the plan.".into(),
            ));
        }
        let source = self
            .plan_source(task["id"].as_str().unwrap())?
            .unwrap_or(Value::Null);
        for field in ["commit", "tree", "instructionsHash"] {
            if basis[field] != source[field] || source[field].is_null() {
                return Ok(Some(
                    "Repository source moved; revalidate the implementation plan.".into(),
                ));
            }
        }
        Ok(None)
    }
}
