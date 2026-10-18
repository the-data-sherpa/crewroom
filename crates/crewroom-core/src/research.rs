//! Read-only Git preflight separated from durable research admission.
use crate::{
    Result,
    store::{Store, conflict, integer, request_id, strict, trim_text},
};
use serde_json::Value;

struct Request {
    revision: u64,
    limit: u64,
    key: String,
}
impl Request {
    fn parse(input: &Value) -> Result<Self> {
        strict(input, &["revision", "requestId", "activeLimitMs"])?;
        Ok(Self {
            revision: integer(input, "revision", 1, u64::MAX)?,
            key: request_id(input)?.into(),
            limit: if input.get("activeLimitMs").is_some() {
                integer(input, "activeLimitMs", 1000, u64::MAX)?
            } else {
                3_600_000
            },
        })
    }
    fn replay(&self, store: &Store, task_id: &str) -> Result<Option<Value>> {
        let prior = store.alternate_replay(task_id, &self.key)?;
        if let Some(prior) = &prior
            && (prior["purpose"] != "research"
                || prior["taskRevision"] != self.revision
                || prior["activeLimitMs"] != self.limit)
        {
            return Err(conflict("Research request identity has different content."));
        }
        Ok(prior)
    }
    fn current_task(&self, store: &Store, task_id: &str) -> Result<Value> {
        let task = store.get_task(task_id)?;
        crate::store::operator_task(&task)?;
        if task["revision"] != self.revision
            || task["needsReacceptance"] == true
            || trim_text(task["brief"].as_str().unwrap_or("")).is_empty()
            || task["criteria"].as_array().is_none_or(Vec::is_empty)
        {
            return Err(conflict(
                "Accept a current brief and criteria before queuing research.",
            ));
        }
        if store.open_run(task_id)?.is_some() {
            return Err(conflict("Close the open run before queuing research."));
        }
        Ok(task)
    }
}
/// Captured under the workflow lock; private fields prevent caller-supplied Git proofs.
pub struct ResearchPreflight {
    task_id: String,
    request: Request,
    repository: Option<Value>,
    replay: bool,
}
/// Produced only by this module's read-only Git validation, never by HTTP input.
pub struct PinnedResearch {
    preflight: ResearchPreflight,
    source: Option<Value>,
}
impl ResearchPreflight {
    /// Run on a blocking worker outside the workflow mutex and SQLite transaction.
    pub fn validate(self) -> Result<PinnedResearch> {
        let source = if self.replay {
            None
        } else {
            self.repository
                .as_ref()
                .map(crate::repositories::pin_research)
                .transpose()?
        };
        Ok(PinnedResearch {
            preflight: self,
            source,
        })
    }
}
impl Store {
    pub fn prepare_research(&self, task_id: &str, input: &Value) -> Result<ResearchPreflight> {
        let request = Request::parse(input)?;
        self.get_task(task_id)?;
        let replay = request.replay(self, task_id)?.is_some();
        let repository = if replay {
            None
        } else {
            let task = request.current_task(self, task_id)?;
            task["repositoryId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|id| self.active_repository(id))
                .transpose()?
        };
        Ok(ResearchPreflight {
            task_id: task_id.into(),
            request,
            repository,
            replay,
        })
    }
    pub fn queue_pinned_research(&self, pinned: &PinnedResearch) -> Result<Value> {
        let ResearchPreflight {
            task_id, request, ..
        } = &pinned.preflight;
        self.transaction(|s| {
            s.get_task(task_id)?;
            if let Some(prior) = request.replay(s, task_id)? {
                return Ok(prior);
            }
            let task = request.current_task(s, task_id)?;
            let repository = task["repositoryId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|id| s.active_repository(id))
                .transpose()?;
            let source = pinned.source.as_ref();
            if repository.is_some() != source.is_some()
                || source.and_then(|v| v.get("repositoryId")) != task.get("repositoryId")
            {
                return Err(conflict("Selected research repository must be pinned before admission."));
            }
            if let (Some(repository), Some(source)) = (&repository, source) {
                let checkout = std::fs::canonicalize(repository["path"].as_str().unwrap())
                    .map_err(|_| conflict("Selected research repository checkout is unavailable. Restore it before queuing research."))?;
                if source["repositoryRevision"].as_u64() != Some(repository["revision"].as_u64().unwrap_or(1))
                    || source["path"].as_str() != checkout.to_str()
                {
                    return Err(conflict("Selected research repository changed during Git preflight. Retry queuing research from the current configuration."));
                }
            }
            let mut run = s.create_alternate_run(task, "research", request.limit, Some(&request.key))?;
            if let Some(source) = source {
                run["researchSource"] = source.clone();
            }
            s.save_run(&run)?;
            s.get_run(run["id"].as_str().unwrap())
        })
    }
    /// Synchronous convenience for callers without an external workflow mutex.
    pub fn queue_research(&self, task_id: &str, input: &Value) -> Result<Value> {
        self.queue_pinned_research(&self.prepare_research(task_id, input)?.validate()?)
    }
}
