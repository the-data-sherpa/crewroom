//! Revision-specific proof produced by the controller. Agent review is a
//! separate attributed claim and never supplies executed command results.
mod artifacts;
mod candidates;
mod checks;
pub use checks::Checks;
mod policy;
pub use policy::definition as check_definition;
mod readiness;
mod research;
mod reviews;
use crate::{
    conflict,
    messaging::StoreAccess,
    repository::{Repository, array, files::hash, limited, request, text},
};
pub use artifacts::Artifacts;
use crewroom_core::{Error, Result, serialized::stringify, store::Store};
pub use research::Research;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
fn uuid_field<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    let value = text(input, key)?;
    if !crewroom_core::store::valid_uuid(value) {
        return Err(Error::Invalid(format!("Invalid {key} UUID.")));
    }
    Ok(value)
}
fn number(input: &Value, key: &str, min: u64, max: u64) -> Result<u64> {
    input[key]
        .as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= min as f64 && *n <= max as f64)
        .map(|n| n as u64)
        .ok_or_else(|| Error::Invalid(format!("Invalid {key}.")))
}
fn prose(input: &Value, key: &str, max: usize) -> Result<String> {
    let value = crewroom_core::store::trim_text(text(input, key)?);
    if value.is_empty() || value.chars().count() > max {
        return Err(Error::Invalid(format!("Invalid {key} length.")));
    }
    Ok(value.into())
}
fn time() -> String {
    crewroom_core::timestamp(crewroom_core::now_ms())
}
#[derive(Clone)]
pub struct Evidence<S: StoreAccess> {
    pub(crate) work: Repository<S>,
    pub(crate) artifacts: Artifacts,
}
impl<S: StoreAccess> Evidence<S> {
    pub fn new(work: Repository<S>, root: PathBuf) -> Result<Self> {
        Ok(Self {
            work,
            artifacts: Artifacts::new(root)?,
        })
    }
    pub fn artifact(&self, body: &str) -> Result<Value> {
        self.artifacts.write(body)
    }
    pub fn read_artifact(&self, artifact: &Value) -> Result<String> {
        self.artifacts.read(artifact)
    }
    fn record(s: &Store, run_id: &str) -> Result<Value> {
        s.coding_record(run_id)?
            .ok_or_else(|| Error::NotFound("Coding evidence is not configured.".into()))
    }
    pub fn check_output(&self, run_id: &str, check_id: &str) -> Result<Value> {
        let record = self.work.store(|s| Self::record(s, run_id))?;
        let check = array(&record["checks"])
            .iter()
            .find(|c| c["id"] == check_id)
            .ok_or_else(|| Error::NotFound("Check evidence is unavailable for this run.".into()))?;
        let candidate = array(&record["candidates"])
            .iter()
            .find(|c| c["id"] == check["candidateId"])
            .ok_or_else(|| Error::NotFound("Check evidence is unavailable for this run.".into()))?;
        let output = self.read_artifact(&check["output"])?;
        let truncated = output.encode_utf16().count() > 16000;
        let mut units = 0;
        let preview: String = output
            .chars()
            .rev()
            .take_while(|c| {
                units += c.len_utf16();
                units <= 16000
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(
            json!({"checkEvidenceId":check["id"],"checkId":check["checkId"],"candidateId":candidate["id"],"revision":candidate["revision"],"exitCode":check["exitCode"],"output":check["output"],"outputPreview":preview,"truncated":truncated}),
        )
    }
}
