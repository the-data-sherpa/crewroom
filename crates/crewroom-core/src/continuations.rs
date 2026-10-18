//! Cancelled-run successors retain history and transfer only durable ownership.
//! Git/head/dirty-content and provider proofs must be supplied by the N4/N5 controller.
use crate::{
    Result,
    dependencies::coding,
    launch::parse_launch,
    requirements::requirements_hash,
    store::{Store, conflict, id, integer, invalid, remove, request_id, strict},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashSet;

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn request(value: &Value) -> Result<Value> {
    strict(value, &["requestId", "revision", "roleId"])?;
    let key = request_id(value)?;
    let revision = integer(value, "revision", 1, 9_007_199_254_740_991)?;
    let role = value["roleId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("A role identity is required."))?;
    Ok(json!({"requestId":key,"revision":revision,"roleId":role}))
}
fn held(reason: &str) -> crate::Error {
    conflict(&format!("Continuation held: {reason}"))
}
// Field order is part of the retained controller's verification contract.
fn same_serialized(left: &Value, right: &Value) -> bool {
    crate::serialized::same(left, right)
}
impl Store {
    /// Preserve the legacy request's serialized identity, including nested field order.
    pub(crate) fn exact_queue_replay(
        &self,
        key: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<Option<Value>> {
        let row: Option<(String, String, String)> = self
            .db
            .query_row(
                "SELECT kind,payload,result FROM queue_requests WHERE request_id=?",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((prior_kind, prior_payload, result)) = row else {
            return Ok(None);
        };
        let serialized = crate::serialized::stringify(payload);
        if prior_kind != kind || prior_payload != serialized {
            return Err(conflict("Queue requestId already has different content."));
        }
        Ok(Some(serde_json::from_str(&result)?))
    }
    pub fn repository_work(&self, run_id: &str) -> Result<Option<Value>> {
        self.get_run(run_id)?;
        self.record("repository_work", "run_id", run_id)
    }
    /// Private ownership recovery must decode each record independently: one
    /// damaged row must not prevent cleanup of another run's checked children.
    pub fn repository_work_records(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .db
            .prepare("SELECT run_id,data FROM repository_work ORDER BY run_id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub(crate) fn save_repository_work(&self, work: &Value) -> Result<Value> {
        self.db.execute(
            "UPDATE repository_work SET data=? WHERE run_id=?",
            params![work.to_string(), work["runId"].as_str()],
        )?;
        Ok(work.clone())
    }
    pub fn continuation_replay(&self, run_id: &str, input: &Value) -> Result<Option<Value>> {
        let value = request(input)?;
        let mut payload = json!({"runId":run_id});
        payload
            .as_object_mut()
            .unwrap()
            .extend(value.as_object().unwrap().clone());
        self.exact_queue_replay(
            value["requestId"].as_str().unwrap(),
            "continue-later",
            &payload,
        )
    }
    pub fn continuation_publication(&self, run_id: &str) -> Result<Option<Value>> {
        if let Some(own) = self.publication(run_id)? {
            return Ok(Some(own));
        }
        let mut child = self.get_run(run_id)?;
        let identity = child["continuedFrom"].clone();
        if identity["publicationId"].as_str().is_none_or(str::is_empty) {
            return Ok(None);
        }
        let mut visited = HashSet::new();
        while let Some(link) = child.get("continuedFrom") {
            if !visited.insert(child["id"].as_str().unwrap().to_owned()) {
                return Err(held("publication lineage contains a cycle."));
            }
            let parent_id = link["runId"].as_str().unwrap();
            let parent = self.get_run(parent_id)?;
            if link["publicationId"] != identity["publicationId"]
                || link["prId"] != identity["prId"]
                || parent["continuedBy"] != child["id"]
                || self
                    .repository_work(parent_id)?
                    .is_none_or(|w| w["transferredTo"] != child["id"])
            {
                return Err(held(
                    "the predecessor publication or ownership no longer matches.",
                ));
            }
            if let Some(record) = self.publication(parent_id)? {
                if record["id"] != identity["publicationId"]
                    || record["pr"]["id"] != identity["prId"]
                {
                    return Err(held("the predecessor PR identity changed."));
                }
                return Ok(Some(record));
            }
            child = parent;
        }
        Err(held("the predecessor publication is unavailable."))
    }
    pub fn publication_superseded(&self, run_id: &str, publication_id: &str) -> Result<bool> {
        let mut parent = self.get_run(run_id)?;
        let mut visited = HashSet::new();
        while let Some(child_id) = parent["continuedBy"].as_str() {
            if !visited.insert(parent["id"].as_str().unwrap().to_owned()) {
                break;
            }
            let child = self.get_run(child_id)?;
            if child["continuedFrom"]["runId"] != parent["id"]
                || self
                    .repository_work(parent["id"].as_str().unwrap())?
                    .is_none_or(|w| w["transferredTo"] != child["id"])
            {
                return Ok(false);
            }
            if self
                .publication(child_id)?
                .is_some_and(|p| p["id"] == publication_id)
            {
                return Ok(true);
            }
            parent = child;
        }
        Ok(false)
    }
    pub fn cancelled_source(&self, run_id: &str, successor_id: Option<&str>) -> Result<Value> {
        let run = self.get_run(run_id)?;
        let work = self.repository_work(run_id)?;
        let publication = self.continuation_publication(run_id)?;
        if !coding(&run) || run["status"] != "cancelled" || run["desiredState"] != "cancelled" {
            return Err(held("Cancel must be confirmed before continuing later."));
        }
        if run["continuedBy"]
            .as_str()
            .is_some_and(|s| Some(s) != successor_id)
        {
            return Err(held(
                "This run already has a linked successor. Open it from run history.",
            ));
        }
        let work = work
            .filter(|w| w["state"] == "ready" && !array(&w["checkpoints"]).is_empty())
            .ok_or_else(|| held("No owned work is available. Queue a new-branch run instead."))?;
        if work["transferredTo"]
            .as_str()
            .is_some_and(|s| Some(s) != successor_id)
        {
            return Err(held("Worktree ownership belongs to another run."));
        }
        if work.get("writer").is_some_and(|w| !w.is_null())
            || array(&self.runtime(run_id)?["attempts"])
                .iter()
                .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
        {
            return Err(held("Process or writer ownership is unresolved."));
        }
        if array(&work["refreshes"]).iter().any(|r| {
            r["state"] == "rebasing"
                || r["state"] == "conflicted" && r.get("supersededBy").is_none()
                || r["resolutionWorkspace"]["legacyOwnershipUnknown"] == true
                || ["preparing", "continuing", "promoting"]
                    .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
        }) {
            return Err(held(
                "Target recovery or Git ownership is unresolved. Inspect the retained workspace.",
            ));
        }
        let coding = self.coding_record(run_id)?.unwrap_or(Value::Null);
        if array(&coding["edits"])
            .iter()
            .any(|v| v["state"] == "pending")
            || array(&coding["completions"])
                .iter()
                .any(|v| v["state"] == "requested")
            || array(&coding["checkOperations"])
                .iter()
                .any(|v| !["recorded", "rejected"].contains(&v["state"].as_str().unwrap_or("")))
        {
            return Err(held(
                "A saved edit, completion or check still needs reconciliation.",
            ));
        }
        if publication.as_ref().is_some_and(|p| {
            p.get("merged").is_some_and(|m| !m.is_null() && m != false)
                || p["pr"]["providerState"] != "OPEN"
                || p["pr"]["state"] != "confirmed"
                || p["pr"]["id"].as_u64().is_none_or(|id| id == 0)
                || p["push"]["state"] != "confirmed"
        }) {
            return Err(held(
                "The PR is closed, merged or its delivery is unconfirmed. Reconcile publication before continuing.",
            ));
        }
        let task = self.get_task(run["taskId"].as_str().unwrap())?;
        let configured = task["repositoryId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|id| self.repository_by_id(id))
            .transpose()?;
        let configured = configured
            .filter(|c| {
                c.get("retiredAt").is_none()
                    && c["id"] == work["repository"]["id"]
                    && c["path"] == work["repository"]["path"]
                    && c["targetBranch"] == work["repository"]["targetBranch"]
            })
            .ok_or_else(|| {
                held("The selected repository no longer matches the retained branch.")
            })?;
        let checkpoint = array(&work["checkpoints"]).last().unwrap().clone();
        let mut source = json!({"run":run,"work":work});
        if let Some(publication) = publication {
            source["publication"] = publication;
        }
        source["configured"] = configured;
        source["checkpoint"] = checkpoint;
        Ok(source)
    }
    pub fn continuation_view(&self, run_id: &str) -> Result<Value> {
        let work = self.repository_work(run_id)?.unwrap_or(Value::Null);
        let mut result = json!({});
        if let Some(branch) = work.get("branch") {
            result["branch"] = branch.clone();
        }
        if let Some(head) = array(&work["checkpoints"])
            .last()
            .and_then(|c| c["revision"].get("head"))
        {
            result["head"] = head.clone();
        }
        let checked = self.continuation_publication(run_id).and_then(|p| {
            if let Some(url) = p.as_ref().and_then(|p| p["pr"].get("url")) {
                result["prUrl"] = url.clone();
            }
            self.cancelled_source(run_id, None)
        });
        if let Err(error) = checked {
            result["reason"] = json!(error.to_string());
        }
        Ok(result)
    }
    pub fn queue_cancelled_continuation(
        &self,
        run_id: &str,
        input: &Value,
        roles: &[Value],
        expected_work: &Value,
    ) -> Result<Value> {
        let value = request(input)?;
        self.transaction(|s| {
            if let Some(replay)=s.continuation_replay(run_id,&value)? {return Ok(replay);}
            let mut source=s.cancelled_source(run_id,None)?;let task=s.get_task(source["run"]["taskId"].as_str().unwrap())?;
            if !same_serialized(&source["work"],expected_work) {return Err(held("retained work changed during verification."));}
            if task["revision"]!=value["revision"] || task["needsReacceptance"]==true || crate::store::trim_text(task["brief"].as_str().unwrap_or("")).is_empty() || array(&task["criteria"]).is_empty() {return Err(conflict("Accept the current brief and criteria before continuing."));}
            if s.open_run(task["id"].as_str().unwrap())?.is_some() {return Err(conflict("This ticket already has an open run."));}
            let coding=s.coding_record(run_id)?.unwrap_or(Value::Null);
            let completions:Vec<_>=array(&coding["completions"]).iter().filter(|c|c["state"]=="completed").map(|c|c["summary"].clone()).collect();
            let instructions=crate::serialized::stringify(&json!({"brief":task["brief"],"criteria":task["criteria"],"previousRunId":run_id,"continuation":"Continue the retained branch and edits. Previous evidence is historical; collect fresh checks and independent review. If the target moved, record fresh implementation, then pause for an explicit target refresh before validation.","previousCompletions":completions}));
            let launch=parse_launch(&json!({"team":[{"name":"Coordinator","roleId":value["roleId"],"coordinator":true,"input":instructions}],"repository":{"mode":"continue","branch":source["work"]["branch"],"expectedHead":source["checkpoint"]["revision"]["head"]}}),Some(roles))?;
            s.validate_launch_briefs(&task,&launch)?;
            let run_id_new=id();let key=value["requestId"].as_str().unwrap();
            let mut run=json!({"id":run_id_new,"taskId":task["id"],"purpose":"coding","requestId":key,"status":"awaiting_runner","position":array(&s.coding_queue()?["plans"]).len(),"createdAt":s.time(),"taskRevision":task["revision"],"taskSnapshot":task,"roles":[],"stages":[],"activeMs":0,"activeLimitMs":source["run"]["activeLimitMs"].as_u64().unwrap_or(3_600_000)});
            if let Some(plan) = s.current_implementation_plan(task["id"].as_str().unwrap())? {run["implementationPlan"] = plan;}
            s.db.execute("INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",params![run_id_new,run["taskId"].as_str(),run.to_string()])?;s.bump_queue()?;
            run["launch"]=launch;run["continuedFrom"]=json!({"runId":run_id,"branch":source["work"]["branch"],"expectedHead":source["checkpoint"]["revision"]["head"],"worktreeHash":source["checkpoint"]["revision"]["worktreeHash"]});
            if let Some(publication)=source.get("publication") {run["continuedFrom"]["publicationId"]=publication["id"].clone();run["continuedFrom"]["prId"]=publication["pr"]["id"].clone();}
            s.save_run(&run)?;source["run"]["continuedBy"]=json!(run_id_new);s.save_run(&source["run"])?;
            let mut payload=json!({"runId":run_id});payload.as_object_mut().unwrap().extend(value.as_object().unwrap().clone());s.receipt(key,"continue-later",&payload,&run)?;Ok(run)
        })
    }
    pub fn transfer_cancelled_work(
        &self,
        run_id: &str,
        expected_work: &Value,
        repository: &Value,
        checkpoint: &Value,
        continuation_target_ref: Option<&str>,
    ) -> Result<Value> {
        self.transaction(|s| {
            let run = s.get_run(run_id)?;
            let link = &run["continuedFrom"];
            if link.is_null()
                || run["status"] != "awaiting_runner"
                || ["paused", "cancelled"].contains(&run["desiredState"].as_str().unwrap_or(""))
                || array(&s.runtime(run_id)?["attempts"])
                    .iter()
                    .any(|a| a["state"] != "admitted")
            {
                return Err(held(
                    "a newer control or execution superseded worktree admission.",
                ));
            }
            let mut source = s.cancelled_source(link["runId"].as_str().unwrap(), Some(run_id))?;
            if source["run"]["continuedBy"] != run_id
                || !same_serialized(&source["work"], expected_work)
                || !same_serialized(
                    &s.repository_by_id(repository["id"].as_str().unwrap())?,
                    repository,
                )
            {
                return Err(held(
                    "retained ownership or repository configuration changed.",
                ));
            }
            if let Some(previous) = s.repository_work(run_id)? {
                return Ok(previous);
            }
            let mut work = source["work"].clone();
            work["runId"] = json!(run_id);
            work["repository"] = repository.clone();
            work["mode"] = json!("continue");
            work["startHead"] = link["expectedHead"].clone();
            work["requirementsHash"] = json!(requirements_hash(
                &run["taskSnapshot"],
                if array(&run["adoptions"]).is_empty() {
                    None
                } else {
                    run["taskRevision"].as_u64()
                }
            ));
            work["checkpoints"] = json!([checkpoint]);
            for field in ["writer", "transferredTo", "refreshes"] {
                remove(&mut work, field);
            }
            if let Some(target) = continuation_target_ref.filter(|s| !s.is_empty()) {
                work["targetRef"] = source["work"]["base"].clone();
                work["continuationTargetRef"] = json!(target);
            }
            source["work"]["transferredTo"] = json!(run_id);
            s.save_repository_work(&source["work"])?;
            s.db.execute(
                "INSERT INTO repository_work VALUES(?,?,?,?)",
                params![
                    run_id,
                    work["commonDir"].as_str(),
                    work["branch"].as_str(),
                    work.to_string()
                ],
            )?;
            Ok(work)
        })
    }
}
