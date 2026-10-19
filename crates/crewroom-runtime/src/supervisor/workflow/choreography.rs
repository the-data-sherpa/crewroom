use super::tools::Caller;
use super::*;
impl<S: StoreAccess> Workflow<S> {
    pub(super) async fn choreography_tool(&self, route: &str, c: &Caller) -> Result<Value> {
        let run = text(&c.run, "id")?;
        match route {
            "coding/choreography" => self.store(|s| {
                Ok(json!({
                    "choreography": s.choreography(run)?,
                    "blockers": s.choreography_blockers(run)?,
                    "messages": s.runtime(run)?["messages"],
                    "completions": s.coding_record(run)?.unwrap_or(Value::Null)["completions"]
                }))
            }),
            "coding/choreography-propose" => {
                let roles = self.project_roles(run, &self.0.roles.read()?)?;
                for member in array(&c.args["schedule"]["assignments"]) {
                    if !roles
                        .iter()
                        .any(|r| r["id"] == member["roleId"] && r["enabled"] == true)
                    {
                        return Err(conflict(
                            "A scheduled role is unavailable. Select or create an enabled user role.",
                        ));
                    }
                }
                self.store(|s| s.propose_choreography(&c.attempt, &c.key, &c.args))
            }
            "coding/package" => {
                if let Some(saved) =
                    self.store(|s| s.replay_package(&c.attempt, &c.key, &c.args))?
                {
                    return Ok(saved);
                }
                let mut snapshots = Vec::new();
                for artifact in array(&c.args["artifacts"]) {
                    let file = self
                        .0
                        .services
                        .repository
                        .read_file(run, text(artifact, "path")?)
                        .await?;
                    if file["sha256"] != artifact["sha256"] {
                        return Err(conflict(
                            "Package artifact changed. Inspect it again before reporting.",
                        ));
                    }
                    let mut snapshot =
                        self.0.services.evidence.artifact(text(&file, "content")?)?;
                    snapshot["path"] = artifact["path"].clone();
                    snapshots.push(snapshot);
                }
                let mut source = self.0.services.repository.status(run).await?["revision"].clone();
                source["artifacts"] = json!(snapshots);
                self.store(|s| s.record_package(&c.attempt, &c.key, &c.args, &source))
            }
            "coding/pair-exchange" => {
                let source = self.0.services.repository.status(run).await?["revision"].clone();
                let saved =
                    self.store(|s| s.prepare_pair_exchange(&c.attempt, &c.key, &c.args, &source))?;
                // A tool receipt confirms only this durable intent. The normal
                // delivery loop owns external Mail and runner acknowledgement,
                // including daemon recovery while transport is unavailable.
                self.notify(run);
                Ok(saved)
            }
            _ => Err(conflict("Unknown choreography operation.")),
        }
    }
}
