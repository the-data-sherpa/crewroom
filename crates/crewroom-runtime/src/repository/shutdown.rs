//! Physical cleanup also covers promotion descriptions saved only in SQLite.
use super::*;
use crate::{CheckedProcess, supervisor::tasks::Task};
use git::Records;

impl<S: StoreAccess> Repository<S> {
    /// Fence checked Git dispatch immediately and drive cleanup through cancelled callers.
    /// Workflow callers must separately fence their outer operations and control state.
    pub fn stop_run(&self, run: &str) -> Result<Task> {
        self.stop_mutations(Some(run))
    }
    pub fn close(&self) -> Result<Task> {
        self.stop_mutations(None)
    }
    pub fn mutations_busy(&self, run: &str) -> bool {
        self.inner.git.busy(run)
    }
    fn stop_mutations(&self, run: Option<&str>) -> Result<Task> {
        let this = self.clone();
        self.inner.git.stop(run, move || this.saved_promotions())
    }
    fn saved_promotions(&self) -> Records {
        let mut result = Records::default();
        let records = match self.store(|s| s.repository_work_records()) {
            Ok(records) => records,
            Err(e) => {
                result.error(None, e);
                return result;
            }
        };
        for (id, data) in records {
            let collect = || -> Result<Vec<Result<CheckedProcess>>> {
                let work: Value = serde_json::from_str(&data)?;
                if text(&work, "runId")? != id {
                    return Err(conflict("Saved repository ownership changed identity."));
                }
                let refreshes = match work.get("refreshes") {
                    None => return Ok(Vec::new()),
                    Some(Value::Array(rows)) => rows,
                    _ => return Err(conflict("Saved target ownership is malformed.")),
                };
                Ok(refreshes
                    .iter()
                    .filter_map(|r| {
                        let w = &r["resolutionWorkspace"];
                        let saved = w.get("promotionProcess");
                        if saved.is_none() && w["state"] != "promoting" {
                            return None;
                        }
                        Some((|| {
                            Ok(self
                                .inner
                                .git
                                .saved_promotion(
                                    &id,
                                    text(w, "operationId")?,
                                    saved.ok_or_else(|| {
                                        conflict("Saved promotion ownership is missing.")
                                    })?,
                                    self.path(&work)?,
                                    text(w, "toHead")?,
                                )?
                                .descriptor)
                        })())
                    })
                    .collect())
            };
            match collect() {
                Ok(rows) => {
                    for row in rows {
                        match row {
                            Ok(descriptor) => result.saved.push((id.clone(), descriptor)),
                            Err(e) => result.error(Some(&id), e),
                        }
                    }
                }
                Err(e) => result.error(Some(&id), e),
            }
        }
        result
    }
}
