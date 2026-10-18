//! Frozen instruction comparisons run inside the existing workflow authority.
//! Only this module sees the private variant mapping; agent tools see a case workspace.
mod maintenance;
mod preparation;
mod scoring;
mod tools;
use super::*;
use crate::{container_command::ContainerCommand, repository::files};
use crewroom_core::serialized::stringify;

pub fn pack() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../../scripts/fixtures/for14-comparison/v1/case-pack.json"
    ))
    .expect("embedded comparison pack")
}
pub fn digest(value: &Value) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut keys = o.keys().collect::<Vec<_>>();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&o[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            _ => v.clone(),
        }
    }
    files::hash(stringify(&sorted(value)))
}
pub fn normalized_target(value: &Value) -> Value {
    let mut v = value.clone();
    remove(
        &mut v,
        &[
            "history",
            "provenance",
            "createdAt",
            "updatedAt",
            "draftProposalId",
            "learningProposalId",
        ],
    );
    if v.get("revision").is_none() {
        v["revision"] = json!(1);
    }
    if v.get("instructions").is_some() {
        if v.get("principles").is_none() {
            v["principles"] = json!([]);
        }
        if v.get("job").is_none() {
            v["job"] = Value::Null;
        }
    }
    v
}
pub(super) fn assignment_prompt(
    assignment: &Value,
    attempt: &Value,
    team: &[Value],
) -> Result<String> {
    let peers = team
        .iter()
        .filter(|a| a["id"] != assignment["id"])
        .map(|a| json!({"name":a["name"],"role":a["role"]["name"]}))
        .collect::<Vec<_>>();
    let control = if assignment["coordinator"] == true {
        "Coordinate the attached task with the assigned specialist using send_message. Convey the ordinary task, route genuine questions, and receive its handoff. The team and checklist are fixed for this assignment. Do not supply a solution or prescribe a tool sequence. Use playbook_status for your pinned checklist and record its progress when appropriate."
    } else {
        "Complete the assigned task within the project instructions. Use handoff_assignment to retain your final complete or incomplete status and explanation."
    };
    Ok(format!(
        "{}\nAssignment: {}\nPrinciples: {}\nProject guidance: {}\nPeers: {}\n{}",
        text(&assignment["role"], "instructions")?,
        text(attempt, "input")?,
        stringify(&assignment["brief"]["principles"]),
        stringify(&assignment["brief"]["projectGuidance"]),
        stringify(&json!(peers)),
        control
    ))
}
impl<S: StoreAccess> Workflow<S> {
    pub(super) fn verify_evaluation_tools(&self, run: &str, config: &Value) -> Result<()> {
        if self.store(|s| s.get_run(run))?["purpose"] == "evaluation" {
            let link = self.evaluation_link(run)?;
            let actor = if config["coordinator"] == true {
                "coordinator"
            } else {
                "worker"
            };
            if config["purpose"] != "evaluation"
                || json!(crate::host_protocol::host_tools(config))
                    != link["manifest"]["execution"]["tools"][actor]
            {
                return Err(conflict(
                    "Evaluation launch tools differ from the frozen manifest.",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn verify_evaluation_recipient(
        &self,
        run: &str,
        coordinator: bool,
        recipient: &str,
    ) -> Result<()> {
        let link = self.evaluation_link(run)?;
        let actor = if coordinator { "coordinator" } else { "worker" };
        let tool = array(&link["manifest"]["execution"]["tools"][actor])
            .iter()
            .find(|tool| tool["name"] == "send_message")
            .ok_or_else(|| conflict("Frozen evaluation messaging is unavailable."))?;
        if !array(&tool["parameters"]["properties"]["to"]["enum"])
            .iter()
            .any(|name| name.as_str() == Some(recipient))
        {
            return Err(conflict(
                "Evaluation messages must address the frozen peer recipient.",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_evaluation_launch(&self, run: &str) -> Result<()> {
        if self.store(|s| s.get_run(run))?["purpose"] == "evaluation" {
            let link = self.evaluation_link(run)?;
            if array(&link["cell"]["events"])
                .iter()
                .any(|e| matches!(e["type"].as_str(), Some("handoff" | "finalization")))
            {
                return Err(conflict(
                    "Evaluation finalization is pending; no further agents may launch.",
                ));
            }
            self.verify_evaluation_identity(&link["manifest"])
                .map_err(|error| {
                    conflict(&format!("Frozen evaluation provider unavailable: {error}"))
                })?;
        }
        Ok(())
    }
    pub(super) fn evaluation_is_finalizing(&self, run: &str) -> Result<bool> {
        if self.store(|s| s.get_run(run))?["purpose"] != "evaluation" {
            return Ok(false);
        }
        let link = self.evaluation_link(run)?;
        Ok(!link["cell"]["result"].is_object()
            && array(&link["cell"]["events"])
                .iter()
                .any(|e| matches!(e["type"].as_str(), Some("handoff" | "finalization"))))
    }
    fn evaluation_link(&self, run: &str) -> Result<Value> {
        self.store(|s| s.comparison_cell_for_run(run))?
            .ok_or_else(|| conflict("Run is not a comparison cell."))
    }
    fn evaluation_root(&self, run: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(run).map_err(|_| conflict("Invalid evaluation run identity."))?;
        let root = self.0.config.root.join("cases").join(run);
        private_directory(&root)?;
        Ok(root)
    }
    fn evaluation_case(link: &Value) -> Result<&Value> {
        array(&link["manifest"]["cases"])
            .iter()
            .find(|c| c["id"] == link["cell"]["caseId"])
            .ok_or_else(|| conflict("Frozen case is missing."))
    }
    pub fn evaluation_assignments(&self, run: &str) -> Result<Vec<Value>> {
        let link = self.evaluation_link(run)?;
        self.verify_evaluation_identity(&link["manifest"])?;
        let case = Self::evaluation_case(&link)?;
        let variant = text(&link["cell"], "variant")?;
        let prepared = array(&case["inputs"][variant]).to_vec();
        if prepared.len() != 2 {
            return Err(conflict("Evaluation needs its fixed two-actor team."));
        }
        Ok(prepared)
    }
    pub async fn prepare_evaluation(&self, run: &str) -> Result<()> {
        let link = self.evaluation_link(run)?;
        self.verify_evaluation_identity(&link["manifest"])?;
        let case = Self::evaluation_case(&link)?;
        let root = self.evaluation_root(run)?;
        let workspace = root.join("workspace");
        private_directory(&workspace)?;
        let marker = root.join("prepared.json");
        if marker.exists() {
            let saved: Value = serde_json::from_slice(&crate::read(&marker)?)?;
            if saved["sourceHash"] != case["sourceHash"] {
                return Err(conflict("Frozen case materialization changed."));
            }
            return Ok(());
        }
        self.verify_evaluation_image(run, &link["manifest"]).await?;
        let prior_receipt = root.join("prior-proof.json");
        if case["priorFiles"].is_array() && !prior_receipt.exists() {
            materialize(&workspace, &root, array(&case["priorFiles"]), &[])?;
            let receipt = self
                .evaluation_check(run, run, "prior-proof", text(case, "priorCheckId")?, false)
                .await?;
            if receipt["exitCode"] != 0 || receipt["sourceIntact"] != true {
                return Err(conflict("The prior-source fixture check did not pass."));
            }
            let source_a = self.evaluation_source(run)?;
            if source_a["sha256"] != case["priorSourceHash"] {
                return Err(conflict("Prior-source fixture hash differs."));
            }
            crate::durable_new(&prior_receipt, stringify(&receipt).as_bytes())?;
        }
        if prior_receipt.exists() {
            let receipt: Value = serde_json::from_slice(&crate::read(&prior_receipt)?)?;
            self.store(|s|s.append_comparison_event(text(&link["cell"],"id")?,&json!({"requestId":format!("prior:{run}"),"type":"check","payload":{"prior":true,"receipt":receipt}})))?;
        }
        materialize(
            &workspace,
            &root,
            array(&case["files"]),
            array(&case["priorFiles"]),
        )?;
        crate::durable_new(
            &marker,
            stringify(&json!({"sourceHash":case["sourceHash"]})).as_bytes(),
        )?;
        Ok(())
    }
    pub fn start_evaluation(&self, run: &str) -> Result<Task> {
        let prepared = self.evaluation_assignments(run)?;
        self.start(
            run,
            json!({"team":[]}),
            prepared.iter().map(|v| v["role"].clone()).collect(),
        )
    }
    pub(super) async fn finish_evaluation_if_ready(&self, run: &str) -> Result<()> {
        let link = self.evaluation_link(run)?;
        let handoff = array(&link["cell"]["events"])
            .iter()
            .rev()
            .find(|e| e["type"] == "handoff");
        if handoff.is_none() {
            return Ok(());
        }
        self.finalize_evaluation(run).await
    }
    fn seal_evaluation_handoff(&self, run: &str, link: &Value) -> Result<()> {
        if !array(&link["cell"]["events"])
            .iter()
            .any(|e| e["type"] == "finalization")
        {
            // Older journals may contain a handoff without the atomic seal.
            // Recover its captured source, never recapture later source as final.
            let handoff = array(&link["cell"]["events"])
                .iter()
                .rev()
                .find(|event| event["type"] == "handoff")
                .ok_or_else(|| conflict("Evaluation finalization requires a worker handoff."))?;
            let source = handoff["payload"]["receipt"]["source"].clone();
            if source != self.evaluation_source(run)? {
                let reason = "Evaluation source changed after the worker handoff.";
                self.store(|store| store.require_attention(run, reason))?;
                return Err(conflict(reason));
            }
            self.store(|s| s.append_comparison_event(text(&link["cell"], "id")?, &json!({
                "requestId":format!("finalization:{run}"), "type":"finalization", "payload":{"source":source}
            })))?;
        }
        Ok(())
    }
    /// Once finalization is durable, recovery can only finish this source; it must
    /// never admit another worker generation for this completed handoff.
    pub async fn finalize_evaluation(&self, run: &str) -> Result<()> {
        let link = self.evaluation_link(run)?;
        if link["cell"]["result"].is_object() {
            return Ok(());
        }
        let (current, runtime) =
            self.store(|store| Ok((store.get_run(run)?, store.runtime(run)?)))?;
        let source_hold = matches!(
            current["attentionReason"].as_str(),
            Some(
                "Evaluation source changed after the worker handoff."
                    | "Evaluation source changed after finalization; result remains held."
            )
        );
        if current["status"] == "attention_required"
            && source_hold
            && array(&runtime["attempts"])
                .iter()
                .all(|attempt| attempt["state"] == "admitted" || attempt["ownership"] == "stopped")
        {
            let captured = array(&link["cell"]["events"])
                .iter()
                .find(|event| event["type"] == "finalization")
                .map(|event| &event["payload"]["source"])
                .or_else(|| {
                    array(&link["cell"]["events"])
                        .iter()
                        .rev()
                        .find(|event| event["type"] == "handoff")
                        .map(|event| &event["payload"]["receipt"]["source"])
                });
            if captured.is_some_and(|source| {
                self.evaluation_source(run)
                    .map_or(true, |actual| &actual != source)
            }) {
                return Ok(());
            }
        }
        let stopped = self.stop_run(run, Some("evaluation")).await?;
        if !matches!(stopped["status"].as_str(), Some("paused" | "cancelled")) {
            return Err(conflict("Evaluation cleanup remains unresolved."));
        }
        self.seal_evaluation_handoff(run, &link)?;
        self.finish_evaluation_result(run, "completed", None)
            .await?;
        Ok(())
    }
    pub async fn finish_evaluation_result(
        &self,
        run: &str,
        state: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        let mut link = self.evaluation_link(run)?;
        if link["cell"]["result"].is_object() {
            return Ok(link["cell"]["result"].clone());
        }
        let runtime = self.store(|s| s.runtime(run))?;
        if array(&runtime["attempts"])
            .iter()
            .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
        {
            return Err(conflict(
                "Evaluation result requires verified stopped ownership.",
            ));
        }
        if array(&link["cell"]["events"])
            .iter()
            .any(|event| event["type"] == "handoff")
        {
            self.seal_evaluation_handoff(run, &link)?;
            link = self.evaluation_link(run)?;
        }
        // A scheduler snapshot may predate the accepted handoff. Its durable
        // terminal intent wins over a later budget/pause/cancel observation.
        let sealed = array(&link["cell"]["events"])
            .iter()
            .any(|event| event["type"] == "finalization");
        let state = if sealed { "completed" } else { state };
        let reason = if sealed { None } else { reason };
        let source = self.evaluation_source(run)?;
        let events = array(&link["cell"]["events"]);
        if state == "completed" {
            let intent = events
                .iter()
                .find(|e| e["type"] == "finalization")
                .ok_or_else(|| {
                    conflict("Completed evaluation requires durable finalization intent.")
                })?;
            if intent["payload"]["source"] != source {
                let reason = "Evaluation source changed after finalization; result remains held.";
                self.store(|store| store.require_attention(run, reason))?;
                return Err(conflict(reason));
            }
        }
        let handoff = events
            .iter()
            .rev()
            .find(|e| e["type"] == "handoff")
            .map(|e| e["payload"]["receipt"].clone())
            .unwrap_or(Value::Null);
        let mut checks = events
            .iter()
            .filter(|e| e["type"] == "check" && e["payload"]["prior"] != true)
            .map(|e| e["payload"]["receipt"].clone())
            .collect::<Vec<_>>();
        let case = Self::evaluation_case(&link)?;
        for check in &mut checks {
            check["outputText"] = json!(self.0.services.evidence.read_artifact(&check["output"])?);
            check["outputValid"] = json!(true);
        }
        let oracle = if state == "completed" {
            let worker = array(&runtime["assignments"])
                .iter()
                .find(|a| a["coordinator"] == false)
                .ok_or_else(|| conflict("Evaluated worker is missing."))?;
            let check_id = if case["id"] == "task-04" {
                "browser"
            } else {
                "acceptance"
            };
            self.evaluation_check(
                run,
                text(worker, "currentAttemptId")?,
                "final-oracle",
                check_id,
                true,
            )
            .await?
        } else {
            Value::Null
        };
        let scores = scoring::score(case, &source, &checks, &handoff, &oracle)?;
        let observed = state != "unavailable" && scores["observed"] == true;
        let final_state = if state == "completed" && !observed {
            "incomplete"
        } else {
            state
        };
        let reason = reason.or(if final_state == "incomplete" {
            Some("Case did not retain a behavioral observation.")
        } else {
            None
        });
        let usage_events = self.store(|s| {
            s.agent_events_of_type(run, &["provider_request", "provider_usage", "message_end"])
        })?;
        let steering = self.store(|s| s.steering(run))?;
        let mut repairs = 0;
        let mut failed = false;
        let mut changed = false;
        for event in events {
            if event["payload"]["prior"] == true {
                continue;
            }
            if event["type"] == "edit"
                && failed
                && event["payload"]["receipt"]["hash"]
                    != event["payload"]["arguments"]["expectedHash"]
            {
                changed = true;
            }
            if event["type"] == "check" {
                let check = &event["payload"]["receipt"];
                if failed && changed && check["exitCode"] == 0 {
                    repairs += 1;
                    failed = false;
                    changed = false;
                }
                if check["exitCode"]
                    .as_i64()
                    .is_some_and(|v| v != 0 && v != 70)
                {
                    failed = true;
                }
            }
        }
        let run_record = self.store(|s| s.get_run(run))?;
        let result = json!({"state":final_state,"reason":reason,"observed":observed,"handoff":handoff,"source":source,"checks":checks,"priorChecks":events.iter().filter(|e|e["payload"]["prior"]==true).collect::<Vec<_>>(),"oracle":oracle,"scores":scores,
            "attempts":runtime["attempts"],"assignments":runtime["assignments"],"activeMs":run_record["activeMs"],
            "repairCycles":null,"repairCandidates":{"count":repairs,"classification":"Requires independent review of substantive changes; failed checks, edits and renewed verification remain in the event ledger."},"interventions":{"count":steering.len(),"messages":steering,"waitMs":null,"waitUnavailableReason":"Decision wait is not independently measured by this runtime."},
            "usage":{"state":if usage_events.is_empty(){"unavailable"}else{"recorded-events"},"events":usage_events,"actualCostUsd":null,"reason":"Native request counts and actual billed cost may be unavailable; retained provider observations keep their actual or estimated basis."},
            "cleanup":{"state":"stopped","attemptIds":array(&runtime["attempts"]).iter().map(|a|a["id"].clone()).collect::<Vec<_>>()}});
        self.store(|s| {
            s.record_comparison_result(
                text(&link["cell"], "id")?,
                &json!({"requestId":format!("result:{run}"),"result":result}),
            )
        })
    }
}

fn materialize(workspace: &Path, root: &Path, fileset: &[Value], prior: &[Value]) -> Result<()> {
    for file in fileset {
        let name = text(file, "path")?;
        let content = text(file, "content")?;
        if files::hash(content) != file["sha256"] {
            return Err(conflict("Frozen fixture content hash differs."));
        }
        let before = match files::read_text(workspace, name, 1_000_000) {
            Ok(body) => Some(body),
            Err(crewroom_core::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if before.as_deref().is_some_and(|v| {
            v != content && !prior.iter().any(|p| p["path"] == name && p["content"] == v)
        }) {
            return Err(conflict(
                "Interrupted fixture preparation has changed source.",
            ));
        }
        if before.as_deref() != Some(content) {
            files::replace(
                workspace,
                root,
                name,
                before.as_ref().map(files::hash).as_deref(),
                content,
            )?;
        }
    }
    Ok(())
}
