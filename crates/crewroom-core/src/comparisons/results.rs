use super::*;
use crate::store::{integer, text};

fn validate_handoff_source(source: &Value) -> Result<()> {
    strict(source, &["sha256", "files"])?;
    let files = source["files"]
        .as_array()
        .ok_or_else(|| invalid("Handoff source needs a file manifest."))?;
    if files.is_empty() || files.len() > 200 || source["sha256"] != hash(&source["files"]) {
        return Err(invalid(
            "Handoff source manifest hash or file count is invalid.",
        ));
    }
    let mut previous: Option<&str> = None;
    for file in files {
        strict(file, &["path", "sha256"])?;
        let path = text(file, "path", 1, 4000)?;
        let digest = text(file, "sha256", 64, 64)?;
        if path.starts_with('/')
            || path.contains('\\')
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || previous.is_some_and(|prior| prior >= path)
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid(
                "Handoff source requires sorted unique relative files and SHA-256 hashes.",
            ));
        }
        previous = Some(path);
    }
    Ok(())
}

impl Store {
    /// Accept the current worker's terminal assignment status and seal exactly
    /// that source in the same transaction. Replay precedes all live admission
    /// checks so a lost reply cannot create a second terminal boundary.
    pub fn record_comparison_handoff(&self, cell_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "type", "attemptId", "payload"])?;
        let key = text(input, "requestId", 1, 300)?;
        if input["type"] != "handoff" {
            return Err(invalid("Terminal comparison input must be a handoff."));
        }
        let attempt_id = text(input, "attemptId", 1, 300)?;
        let payload = json!({"cellId":cell_id,"input":input});
        self.transaction(|s| {
            if let Some(event) = s.comparison_replay(key, "event", &payload)? {
                return Ok(event);
            }
            strict(&input["payload"], &["arguments", "receipt"])?;
            let arguments = &input["payload"]["arguments"];
            strict(arguments, &["status", "summary"])?;
            if !matches!(arguments["status"].as_str(), Some("complete" | "incomplete"))
                || text(arguments, "summary", 1, 40000)?.trim().is_empty()
            {
                return Err(invalid("Handoff needs a terminal status and explanation."));
            }
            let receipt = &input["payload"]["receipt"];
            strict(receipt, &["status", "summary", "source"])?;
            if receipt["status"] != arguments["status"] || receipt["summary"] != arguments["summary"] {
                return Err(invalid("Handoff receipt must match its accepted arguments."));
            }
            validate_handoff_source(&receipt["source"])?;
            let cell = s.record("comparison_cells", "id", cell_id)?
                .ok_or_else(|| Error::NotFound("Comparison cell not found.".into()))?;
            let run_id = cell["runId"].as_str().ok_or_else(|| conflict("Cell has no admitted run."))?;
            let run = s.account_time_in_transaction(run_id)?;
            s.assert_evaluation_dispatch(run_id)?;
            let attempt = s.get_attempt(attempt_id)?;
            let assignment = s.get_assignment(attempt["assignmentId"].as_str().unwrap())?;
            if run["purpose"] != "evaluation"
                || run["status"] != "running"
                || run["desiredState"] != "running"
                || attempt["runId"] != run_id
                || assignment["runId"] != run_id
                || assignment["currentAttemptId"] != attempt_id
                || assignment.get("retiredAt").is_some()
                || assignment["coordinator"] != false
                || attempt["ownership"] != "held"
                || !matches!(attempt["state"].as_str(), Some("idle" | "executing"))
            {
                return Err(conflict("Only the current active evaluation worker may hand off its assignment."));
            }
            let event = s.append_comparison_event_inner(cell_id, input)?;
            s.append_comparison_event_inner(cell_id, &json!({
                "requestId":format!("finalization:{run_id}"), "type":"finalization",
                "payload":{"source":receipt["source"],"handoffEventId":event["id"],"handoffRequestId":key}
            }))?;
            Ok(event)
        })
    }

    /// Append controller-observed facts, never a replacement for old failures.
    pub fn append_comparison_event(&self, cell_id: &str, input: &Value) -> Result<Value> {
        if input["type"] == "handoff" {
            return self.record_comparison_handoff(cell_id, input);
        }
        self.transaction(|s| s.append_comparison_event_inner(cell_id, input))
    }

    fn append_comparison_event_inner(&self, cell_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "type", "attemptId", "payload"])?;
        let key = text(input, "requestId", 1, 300)?;
        let kind = text(input, "type", 1, 60)?;
        if ![
            "handoff",
            "check",
            "edit",
            "repair",
            "intervention",
            "error",
            "provider",
            "launch",
            "ownership",
            "artifact",
            "usage",
            "playbook",
            "finalization",
        ]
        .contains(&kind)
        {
            return Err(invalid("Unknown comparison event type."));
        }
        let payload = json!({"cellId":cell_id,"input":input});
        self.transaction(|s| {
            if let Some(result)=s.comparison_replay(key,"event",&payload)? {return Ok(result);}
            let cell=s.record("comparison_cells","id",cell_id)?.ok_or_else(||Error::NotFound("Comparison cell not found.".into()))?;
            if cell.get("result").is_some() {return Err(conflict("A committed result's event ledger is sealed."));}
            let run_id=cell["runId"].as_str().ok_or_else(||conflict("Cell has no admitted run."))?;
            let terminal: Option<String> = s.db.query_row("SELECT data FROM comparison_events WHERE cell_id=? AND json_extract(data,'$.type') IN ('handoff','finalization') ORDER BY CASE json_extract(data,'$.type') WHEN 'finalization' THEN 0 ELSE 1 END,rowid DESC LIMIT 1", [cell_id], |row|row.get(0)).optional()?;
            if let Some(terminal) = terminal {
                let terminal = parse(terminal)?;
                let sealing_legacy = kind == "finalization" && terminal["type"] == "handoff";
                if sealing_legacy {
                    if input["payload"]["source"] != terminal["payload"]["receipt"]["source"] {
                        return Err(conflict("Finalization must retain the source captured by the accepted handoff."));
                    }
                } else if !matches!(kind,"check"|"ownership"|"artifact"|"usage"|"error"|"provider")
                    || (kind=="check" && (input.get("attemptId").is_some() || input["payload"]["receipt"]["oracle"]!=true)) {
                    return Err(conflict("Final assignment source is sealed; only controller assessment and cleanup observations may follow."));
                }
            }
            if let Some(attempt_id)=input.get("attemptId") {
                let attempt=s.get_attempt(attempt_id.as_str().ok_or_else(||invalid("attemptId must be text."))?)?;
                if attempt["runId"]!=run_id {return Err(conflict("Attempt does not belong to this comparison cell."));}
            }
            let event=json!({"id":id(),"comparisonId":cell["comparisonId"],"cellId":cell_id,"runId":run_id,"requestId":key,"attemptId":input["attemptId"],"type":kind,"payload":input["payload"],"at":s.time()});
            s.db.execute("INSERT INTO comparison_events VALUES(?,?,?,?)",params![event["id"].as_str(),cell["comparisonId"].as_str(),cell_id,event.to_string()])?;
            s.remember_comparison(key,"event",&payload,&event)?;
            Ok(event)
        })
    }

    /// Internal trusted scorer boundary. Call only after artifact verification and
    /// stopping every owned process; committed results are immutable.
    pub fn record_comparison_result(&self, cell_id: &str, input: &Value) -> Result<Value> {
        strict(input, &["requestId", "result"])?;
        let key = text(input, "requestId", 1, 300)?;
        let result = &input["result"];
        let state = text(result, "state", 1, 30)?;
        if !matches!(
            state,
            "completed" | "incomplete" | "unavailable" | "cancelled"
        ) || !result["observed"].is_boolean()
        {
            return Err(invalid(
                "Comparison result needs a terminal state and explicit observed coverage.",
            ));
        }
        if result["cleanup"]["state"] != "stopped"
            || result["usage"].as_object().is_none_or(|v| v.is_empty())
        {
            return Err(invalid(
                "Result needs a stopped cleanup receipt and actual usage or an unavailable reason.",
            ));
        }
        if state == "completed" && (result["observed"] != true || !result["scores"].is_object()) {
            return Err(invalid(
                "Completed case observations require a structured score.",
            ));
        }
        if state == "unavailable" && result["observed"] != false {
            return Err(invalid("Unavailable cells cannot count as observed cases."));
        }
        if state != "completed" {
            text(result, "reason", 1, 4000)?;
        }
        let payload = json!({"cellId":cell_id,"input":input});
        self.transaction(|s| {
            if let Some(value)=s.comparison_replay(key,"result",&payload)? {return Ok(value);}
            let mut cell=s.record("comparison_cells","id",cell_id)?.ok_or_else(||Error::NotFound("Comparison cell not found.".into()))?;
            if cell.get("result").is_some() {return Err(conflict("Cell result is already committed. Results cannot be replaced."));}
            let finalization: Option<String> = s.db.query_row("SELECT data FROM comparison_events WHERE cell_id=? AND json_extract(data,'$.type') IN ('handoff','finalization') ORDER BY CASE json_extract(data,'$.type') WHEN 'finalization' THEN 0 ELSE 1 END,rowid DESC LIMIT 1", [cell_id], |row|row.get(0)).optional()?;
            if let Some(intent) = finalization {
                let intent = parse(intent)?;
                let source = if intent["type"] == "handoff" { &intent["payload"]["receipt"]["source"] } else { &intent["payload"]["source"] };
                if state != "completed" || &result["source"] != source {
                    return Err(conflict("An accepted terminal handoff requires final assessment of its sealed source; later stop or budget results cannot replace it."));
                }
            }
            let run_id=cell["runId"].as_str().ok_or_else(||conflict("Cell has no run to settle."))?.to_owned();
            let run=s.account_time(&run_id)?;
            let runtime=s.runtime(&run_id)?;
            let attempts=runtime["attempts"].as_array().unwrap();
            if attempts.iter().any(|a|a["state"]!="admitted" && a["ownership"]!="stopped") {
                return Err(conflict("Every cell attempt must have verified stopped ownership before a result commits."));
            }
            if result["observed"]==true && !attempts.iter().any(|a|a["hostPid"].as_u64().is_some() && a["ownership"]=="stopped" && runtime["assignments"].as_array().unwrap().iter().any(|assignment| assignment["id"] == a["assignmentId"] && assignment["coordinator"] == false)) {
                return Err(conflict("Observed coverage requires a real, stopped process attempt."));
            }
            let mut stored=result.clone();
            stored["activeMs"]=run["activeMs"].clone();
            stored["runId"]=json!(run_id);
            stored["attemptIds"]=json!(attempts.iter().map(|a|a["id"].clone()).collect::<Vec<_>>());
            stored["recordedAt"]=json!(s.time());
            cell["resultHash"]=json!(hash(&stored)); cell["result"]=stored; cell["state"]=json!(state); cell["finishedAt"]=json!(s.time());
            s.save_comparison_cell(&cell)?;
            // Evaluation's terminal state is deliberately not a coding outcome.
            let mut run=s.get_run(&run_id)?;
            run["status"]=json!(if state=="cancelled" {"cancelled"} else {"completed"});
            run["desiredState"]=json!(if state=="cancelled" {"cancelled"} else {"paused"});
            run["evaluationResult"]=json!({"cellId":cell_id,"hash":cell["resultHash"],"state":state});
            run.as_object_mut().unwrap().remove("activeSince");
            s.save_run(&run)?;
            s.vacate_slot(&run_id)?;
            let comparison_id=cell["comparisonId"].as_str().unwrap();
            let mut comparison=s.comparison_record(comparison_id)?;
            let cells=s.comparison_cells(comparison_id)?;
            if cells.iter().all(|c|c.get("result").is_some() || c["state"]=="cancelled") {
                if comparison["desiredState"]!="cancelled" {
                    comparison["state"]=json!(if cells.iter().all(|c|c["result"]["observed"]==true) {"completed"} else {"incomplete"});
                    comparison["desiredState"]=json!("paused");
                }
                comparison["finishedAt"]=json!(s.time());
                s.save_comparison(&comparison)?;
            } else if state=="unavailable" && comparison["desiredState"]=="running" {
                // Pausing in the result transaction closes the crash window where
                // recovery could otherwise admit the next frozen cell.
                comparison["state"]=json!("paused");
                comparison["desiredState"]=json!("paused");
                comparison["updatedAt"]=json!(s.time());
                s.save_comparison(&comparison)?;
            }
            s.remember_comparison(key,"result",&payload,&cell)?;
            Ok(cell)
        })
    }

    /// Associate an already verified catalog save. Catalog writes remain the
    /// service's existing revision-checked Save/Approve authority.
    pub fn record_comparison_adoption(&self, comparison_id: &str, input: &Value) -> Result<Value> {
        strict(
            input,
            &[
                "requestId",
                "baseRevision",
                "proposedHash",
                "adoptedRevision",
            ],
        )?;
        let key = request_id(input)?;
        let base = integer(input, "baseRevision", 1, u64::MAX)?;
        let adopted = integer(input, "adoptedRevision", 1, u64::MAX)?;
        text(input, "proposedHash", 64, 64)?;
        let payload = json!({"comparisonId":comparison_id,"input":input});
        self.transaction(|s| {
            if let Some(value)=s.comparison_replay(key,"adoption",&payload)? {return Ok(value);}
            let target=s.get_comparison(comparison_id)?["manifest"]["target"].clone();
            if target["baseRevision"]!=base || target["proposedHash"]!=input["proposedHash"] || base.checked_add(1)!=Some(adopted) {
                return Err(conflict("Saved revision does not match this comparison's exact target and base."));
            }
            let mut record=s.comparison_record(comparison_id)?;
            let adoption=json!({"baseRevision":base,"adoptedRevision":adopted,"proposedHash":input["proposedHash"],"at":s.time()});
            if let Some(existing)=record.get("adoption") {
                if existing["baseRevision"]!=base || existing["adoptedRevision"]!=adopted || existing["proposedHash"]!=input["proposedHash"] {return Err(conflict("Comparison already has a different adoption receipt."));}
                s.remember_comparison(key,"adoption",&payload,existing)?;
                return Ok(existing.clone());
            }
            record["adoption"]=adoption.clone(); s.save_comparison(&record)?;
            s.remember_comparison(key,"adoption",&payload,&adoption)?;
            Ok(adoption)
        })
    }
}
