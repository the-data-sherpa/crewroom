//! Immutable instruction comparisons and controller-owned cell receipts.
//! These records never adopt configuration or supply coding readiness evidence.
mod manifest;
mod results;
use crate::{
    Error, Result,
    store::{Store, conflict, id, invalid, parse, request_id, strict},
};
pub use manifest::{hash, validate as validate_manifest};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

impl Store {
    /// Read a committed creation receipt before checking today's catalog or tools.
    pub fn comparison_create_replay(&self, input: &Value) -> Result<Option<Value>> {
        strict(input, &["requestId", "manifest"])?;
        self.comparison_replay(request_id(input)?, "create", input)
    }
    pub fn comparison_adoption_replay(
        &self,
        comparison_id: &str,
        input: &Value,
    ) -> Result<Option<Value>> {
        self.comparison_replay(
            request_id(input)?,
            "adoption",
            &json!({"comparisonId":comparison_id,"input":input}),
        )
    }
    /// An existing S7 proposal is provenance, never an invented approval state.
    pub fn validate_comparison_proposal(&self, target: &Value) -> Result<()> {
        let Some(proposal_id) = target.get("proposalId").filter(|v| !v.is_null()) else {
            return Ok(());
        };
        let proposal = self.get_learning_proposal(
            proposal_id
                .as_str()
                .ok_or_else(|| invalid("Comparison proposal identity must be text."))?,
        )?;
        if target["kind"] != "role"
            || proposal["target"]["kind"] != "role"
            || proposal["target"]["roleId"] != target["id"]
            || proposal["baseVersion"] != target["baseRevision"]
            || proposal["change"] != target["proposed"]["instructions"]
            || proposal["status"] != "pending"
        {
            return Err(conflict(
                "Comparison must bind the exact pending role proposal and its current base revision.",
            ));
        }
        Ok(())
    }
    fn comparison_record(&self, comparison_id: &str) -> Result<Value> {
        self.record("comparisons", "id", comparison_id)?
            .ok_or_else(|| Error::NotFound("Comparison not found.".into()))
    }
    fn comparison_cells(&self, comparison_id: &str) -> Result<Vec<Value>> {
        let mut statement = self
            .db
            .prepare("SELECT data FROM comparison_cells WHERE comparison_id=? ORDER BY ordinal")?;
        statement
            .query_map([comparison_id], |r| r.get::<_, String>(0))?
            .map(|r| parse(r?))
            .collect()
    }
    fn save_comparison(&self, value: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE comparisons SET data=? WHERE id=?",
            params![value.to_string(), value["id"].as_str()],
        )?;
        Ok(())
    }
    fn save_comparison_cell(&self, value: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE comparison_cells SET run_id=?,data=? WHERE id=?",
            params![
                value["runId"].as_str(),
                value.to_string(),
                value["id"].as_str()
            ],
        )?;
        Ok(())
    }
    fn comparison_replay(
        &self,
        key: &str,
        operation: &str,
        payload: &Value,
    ) -> Result<Option<Value>> {
        let existing: Option<(String, String, String)> = self
            .db
            .query_row(
                "SELECT operation,payload,result FROM comparison_requests WHERE request_id=?",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        existing
            .map(|(old_operation, old_payload, result)| {
                if old_operation != operation || parse(old_payload)? != *payload {
                    return Err(conflict(
                        "Comparison request key was already used with different content.",
                    ));
                }
                parse(result)
            })
            .transpose()
    }
    fn remember_comparison(
        &self,
        key: &str,
        operation: &str,
        payload: &Value,
        result: &Value,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO comparison_requests VALUES(?,?,?,?)",
            params![key, operation, payload.to_string(), result.to_string()],
        )?;
        Ok(())
    }
    pub fn create_comparison(&self, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "manifest"])?;
        let key = request_id(input)?;
        validate_manifest(&input["manifest"])?;
        self.transaction(|s| {
            if let Some(value) = s.comparison_replay(key,"create",input)? { return Ok(value); }
            let manifest = &input["manifest"];
            s.validate_comparison_proposal(&manifest["target"])?;
            let comparison_id = id();
            let record = json!({"id":comparison_id,"manifestHash":hash(manifest),"state":"planned","desiredState":"planned","createdAt":s.time()});
            s.db.execute("INSERT INTO comparisons VALUES(?,?,?,?,?)",params![comparison_id,key,manifest.to_string(),record["manifestHash"].as_str(),record.to_string()])?;
            let mut ordinal = 0;
            for repetition in 1..=2 {
                for (index, case) in manifest["cases"].as_array().unwrap().iter().enumerate() {
                    let order = if (index + repetition) % 2 == 1 { ["current", "proposed"] } else { ["proposed", "current"] };
                    for variant in order {
                        let cell = json!({"id":id(),"comparisonId":comparison_id,"caseId":case["id"],"repetition":repetition,"variant":variant,"ordinal":ordinal,"state":"planned","createdAt":s.time()});
                        s.db.execute("INSERT INTO comparison_cells(id,comparison_id,case_id,repetition,variant,ordinal,data) VALUES(?,?,?,?,?,?,?)",params![cell["id"].as_str(),comparison_id,case["id"].as_str(),repetition as i64,variant,ordinal,cell.to_string()])?;
                        ordinal += 1;
                    }
                }
            }
            let result = s.get_comparison(&comparison_id)?;
            s.remember_comparison(key,"create",input,&result)?;
            Ok(result)
        })
    }
    pub fn get_comparison(&self, comparison_id: &str) -> Result<Value> {
        let mut record = self.comparison_record(comparison_id)?;
        record["manifest"] = parse(self.db.query_row(
            "SELECT manifest FROM comparisons WHERE id=?",
            [comparison_id],
            |r| r.get(0),
        )?)?;
        let mut cells = self.comparison_cells(comparison_id)?;
        let mut active_ms = 0_u64;
        for cell in &mut cells {
            let mut statement = self
                .db
                .prepare("SELECT data FROM comparison_events WHERE cell_id=? ORDER BY rowid")?;
            cell["events"] = json!(
                statement
                    .query_map([cell["id"].as_str()], |r| r.get::<_, String>(0))?
                    .map(|r| parse(r?))
                    .collect::<Result<Vec<_>>>()?
            );
            if let Some(run_id) = cell["runId"].as_str().map(str::to_owned) {
                let run = self.get_run(&run_id)?;
                cell["activeMs"] = json!(run["activeMs"].as_u64().unwrap_or(0));
                cell["runState"] = run["status"].clone();
                cell["attempts"] = self.runtime(&run_id)?["attempts"].clone();
                if cell.get("result").is_none() {
                    let uncertain = cell["attempts"].as_array().is_some_and(|attempts| {
                        attempts
                            .iter()
                            .any(|attempt| attempt["ownership"] == "uncertain")
                    });
                    if run["status"] == "attention_required" || uncertain {
                        cell["state"] = json!("held");
                        cell["attentionReason"] = run["attentionReason"].as_str().map_or_else(
                            || json!("Process ownership is uncertain; retain this cell until cleanup is verified."),
                            |reason| json!(reason),
                        );
                    } else if cell["events"].as_array().is_some_and(|events| {
                        events.iter().any(|event| {
                            matches!(event["type"].as_str(), Some("handoff" | "finalization"))
                        })
                    }) {
                        cell["state"] = json!("finalizing");
                    } else if record["desiredState"] != "cancelled" {
                        cell["state"] = json!(match run["status"].as_str() {
                            Some("paused") => "paused",
                            Some("pausing") => "pausing",
                            _ => "running",
                        });
                    }
                }
            }
            active_ms = active_ms.saturating_add(cell["activeMs"].as_u64().unwrap_or(0));
        }
        let observed = cells
            .iter()
            .filter(|c| c["result"]["observed"] == true)
            .count();
        let finished = cells.iter().filter(|c| c.get("result").is_some()).count();
        let outstanding = cells.iter().any(|cell| {
            cell["attempts"].as_array().is_some_and(|attempts| {
                attempts
                    .iter()
                    .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
            })
        });
        if record["desiredState"] == "paused"
            && !matches!(record["state"].as_str(), Some("completed" | "incomplete"))
        {
            record["state"] = json!(if outstanding { "pausing" } else { "paused" });
        }
        if record["desiredState"] == "cancelled" {
            // Preparation can own the serial slot before any agent attempt
            // exists. Only a committed result settles that admitted run.
            let unsettled = cells
                .iter()
                .any(|cell| cell["runId"].is_string() && cell.get("result").is_none());
            record["state"] = json!(if outstanding || unsettled {
                "cancelling"
            } else {
                "cancelled"
            });
        }
        if let Some(held) = cells.iter().find(|cell| cell["state"] == "held") {
            record["state"] = json!("held");
            record["attentionReason"] = held["attentionReason"].clone();
        }
        record["coverage"] = json!({"planned":cells.len(),"observed":observed,"finished":finished,"remaining":cells.len()-finished});
        record["activeMs"] = json!(active_ms);
        record["cells"] = json!(cells);
        Ok(record)
    }
    pub fn comparisons(&self) -> Result<Vec<Value>> {
        self.all("comparisons")?
            .iter()
            .map(|v| self.get_comparison(v["id"].as_str().unwrap()))
            .collect()
    }
    pub fn comparison_cell_for_run(&self, run_id: &str) -> Result<Option<Value>> {
        let Some(cell) = self.record("comparison_cells", "run_id", run_id)? else {
            return Ok(None);
        };
        let comparison_id = cell["comparisonId"].as_str().unwrap();
        let comparison = self.get_comparison(comparison_id)?;
        let cell = comparison["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == cell["id"])
            .unwrap();
        Ok(Some(
            json!({"cell":cell,"comparisonId":comparison_id,"manifest":comparison["manifest"],"manifestHash":comparison["manifestHash"],"desiredState":comparison["desiredState"]}),
        ))
    }
    /// Persist intent first; daemon ownership reconciliation finishes the pause/stop.
    pub fn control_comparison(&self, comparison_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "action"])?;
        let key = request_id(input)?;
        let action = input["action"]
            .as_str()
            .ok_or_else(|| invalid("Comparison action is required."))?;
        if !matches!(action, "start" | "pause" | "resume" | "cancel") {
            return Err(invalid("Unknown comparison control action."));
        }
        let payload = json!({"comparisonId":comparison_id,"input":input});
        self.transaction(|s| {
            if let Some(value) = s.comparison_replay(key, "control", &payload)? {
                return Ok(value);
            }
            let mut record = s.comparison_record(comparison_id)?;
            if matches!(
                record["state"].as_str(),
                Some("completed" | "incomplete" | "cancelled")
            ) {
                return Err(conflict(
                    "Historical comparisons cannot restart; create a new manifest.",
                ));
            }
            if action == "start" && record["state"] != "planned"
                || action == "resume" && record["desiredState"] != "paused"
            {
                return Err(conflict(
                    "Comparison control is not valid in its current state.",
                ));
            }
            if matches!(action, "start" | "resume") {
                let comparison = s.get_comparison(comparison_id)?;
                if comparison["activeMs"].as_u64().unwrap_or(0)
                    >= comparison["manifest"]["design"]["totalActiveMs"]
                        .as_u64()
                        .unwrap()
                {
                    return Err(conflict(
                        "The frozen comparison active allowance is exhausted.",
                    ));
                }
                record["desiredState"] = json!("running");
                record["state"] = json!("running");
            } else {
                let desired = if action == "pause" {
                    "paused"
                } else {
                    "cancelled"
                };
                record["desiredState"] = json!(desired);
                record["state"] = json!(desired);
                for mut cell in s.comparison_cells(comparison_id)? {
                    if cell.get("result").is_some() {
                        continue;
                    }
                    if let Some(run_id) = cell["runId"].as_str() {
                        if action == "pause" {
                            s.request_pause(run_id)?;
                        } else {
                            s.set_desired_state(run_id, "cancelled")?;
                        }
                        cell["state"] = json!(if action == "pause" {
                            "paused"
                        } else {
                            "cancelling"
                        });
                    } else if action == "cancel" {
                        cell["state"] = json!("cancelled");
                    }
                    s.save_comparison_cell(&cell)?;
                }
            }
            record["updatedAt"] = json!(s.time());
            s.save_comparison(&record)?;
            let result = s.get_comparison(comparison_id)?;
            s.remember_comparison(key, "control", &payload, &result)?;
            Ok(result)
        })
    }
    /// Reserve the same serial writer slot as coding before creating any process.
    pub fn create_comparison_cell_run(&self, comparison_id: &str, cell_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let comparison=s.get_comparison(comparison_id)?;
            let cells=comparison["cells"].as_array().unwrap();
            let mut cell=cells.iter().find(|c|c["id"]==cell_id).cloned().ok_or_else(||Error::NotFound("Comparison cell not found.".into()))?;
            if let Some(run_id)=cell["runId"].as_str() {return s.get_run(run_id);}
            if comparison["desiredState"]!="running" {return Err(conflict("Comparison must be explicitly started or resumed."));}
            let first=cells.iter().find(|c|c.get("result").is_none() && c["state"]!="cancelled").ok_or_else(||conflict("Comparison has no unfinished cell."))?;
            if first["id"]!=cell_id {return Err(conflict("Cells must execute in their frozen order after the preceding result commits."));}
            // Comparisons keep the whole workspace: every project's slot must be free.
            if !s.occupants("")?.is_empty() {return Err(conflict("Another coding or evaluation run retains an execution slot or unverified ownership."));}
            let manifest=&comparison["manifest"];
            let used=comparison["activeMs"].as_u64().unwrap_or(0);
            let remaining=manifest["design"]["totalActiveMs"].as_u64().unwrap().saturating_sub(used);
            if remaining==0 {return Err(conflict("The comparison active allowance is exhausted."));}
            let case=manifest["cases"].as_array().unwrap().iter().find(|c|c["id"]==cell["caseId"]).unwrap();
            let task_id=id();
            let task=json!({"id":task_id,"key":format!("WORK-{}",&task_id[..8]),"title":case["title"].as_str().unwrap_or("Workspace assignment"),"description":case["prompt"],"source":"evaluation","internal":true,"priority":"Unspecified","project":"","rank":0,"sourceRevision":1,"updatedAt":s.time()});
            let refinement=json!({"revision":1,"brief":case["prompt"],"criteria":case["requirements"]["criteria"].as_array().cloned().unwrap_or_default(),"needsReacceptance":false});
            s.db.execute("INSERT INTO tickets VALUES(?,NULL,NULL,?,?)",params![task_id,task.to_string(),refinement.to_string()])?;
            let limit=remaining.min(manifest["design"]["perCellActiveMs"].as_u64().unwrap());
            let mut run=s.create_alternate_run(s.get_task(&task_id)?,"evaluation",limit,None)?;
            let run_id=run["id"].as_str().unwrap().to_owned();
            run["desiredState"]=json!("running");
            run["controlRevision"]=json!(1);
            run["evaluation"]=json!({"comparisonId":comparison_id,"cellId":cell_id,"manifestHash":comparison["manifestHash"]});
            run["schedule"]=json!({"id":id(),"at":s.time()});
            run["evaluationInputs"]=case["inputs"][cell["variant"].as_str().unwrap()].clone();
            if let Some(guidance)=case.get("projectGuidance") {run["evaluationGuidance"]=guidance.clone();}
            if let Some(playbook)=run["evaluationInputs"][0]["brief"].get("playbook").cloned() {crate::playbooks::initial(&mut run,playbook);}
            s.save_run(&run)?;
            s.occupy_slot(&run_id)?;
            // Strip view-only fields before writing the durable cell.
            cell.as_object_mut().unwrap().remove("events");
            cell["runId"]=json!(run_id); cell["state"]=json!("running"); cell["startedAt"]=json!(s.time());
            s.save_comparison_cell(&cell)?;
            Ok(run)
        })
    }
    pub fn claim_comparison_cell_run(&self, comparison_id: &str, cell_id: &str) -> Result<Value> {
        self.create_comparison_cell_run(comparison_id, cell_id)
    }
    pub(crate) fn assert_evaluation_admission(&self, run: &Value, input: &Value) -> Result<()> {
        if run["purpose"] != "evaluation" {
            return Ok(());
        }
        let link = self
            .comparison_cell_for_run(run["id"].as_str().unwrap())?
            .ok_or_else(|| conflict("Evaluation run has no committed comparison cell."))?;
        if link["desiredState"] != "running"
            || link["cell"]["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| matches!(event["type"].as_str(), Some("handoff" | "finalization")))
            })
            || !run["evaluationInputs"]
                .as_array()
                .is_some_and(|team| team.contains(input))
        {
            return Err(conflict(
                "Evaluation admission must exactly match its running frozen manifest.",
            ));
        }
        self.assert_coding_slot_available(run["id"].as_str().unwrap())
    }
    pub(crate) fn assert_evaluation_dispatch(&self, run_id: &str) -> Result<()> {
        let run = self.get_run(run_id)?;
        if run["purpose"] != "evaluation" {
            return Ok(());
        }
        let link = self
            .comparison_cell_for_run(run_id)?
            .ok_or_else(|| conflict("Evaluation run has no comparison binding."))?;
        if link["desiredState"] != "running"
            || !link["cell"]["result"].is_null()
            || link["cell"]["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| matches!(event["type"].as_str(), Some("handoff" | "finalization")))
            })
            || run["activeMs"].as_u64().unwrap_or(0) >= run["activeLimitMs"].as_u64().unwrap_or(0)
        {
            return Err(conflict(
                "Evaluation is paused, finished or at its frozen active allowance.",
            ));
        }
        self.assert_coding_slot_available(run_id)
    }
}
