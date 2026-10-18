use super::*;
use crate::store::{integer, strict, trimmed_text};
use rusqlite::params;

impl Store {
    pub fn choreography_history(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run_records("choreography_revisions", run_id)
    }
    pub fn choreography(&self, run_id: &str) -> Result<Option<Value>> {
        let mut query = self.db.prepare(
            "SELECT data FROM choreography_revisions WHERE run_id=? ORDER BY revision DESC LIMIT 1",
        )?;
        let mut rows = query.query([run_id])?;
        rows.next()?
            .map(|r| crate::store::parse(r.get::<_, String>(0)?))
            .transpose()
    }
    pub(super) fn append_choreography(
        &self,
        run_id: &str,
        state: &mut Value,
        event: Value,
    ) -> Result<()> {
        if state.to_string().len() > 2_000_000 {
            return Err(conflict(
                "Choreography history reached its bounded size; retain work for operator inspection.",
            ));
        }
        let revision = state["revision"].as_u64().unwrap_or(0) + 1;
        state["revision"] = json!(revision);
        state["event"] = event;
        state["updatedAt"] = json!(self.time());
        self.db.execute(
            "INSERT INTO choreography_revisions VALUES(?,?,?)",
            params![run_id, revision as i64, state.to_string()],
        )?;
        Ok(())
    }
    pub fn propose_choreography(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
    ) -> Result<Value> {
        strict(input, &["revision", "schedule", "reopenPackages"])?;
        text(&json!({"key":key}), "key", 1, 160)?;
        integer(input, "revision", 0, u64::MAX)?;
        let receipt = format!("choreography:{attempt_id}:{key}");
        self.transaction(|s| {
            let payload=json!({"attemptId":attempt_id,"input":input});
            if let Some(prior)=s.replay(&receipt,"choreography",&payload)? {return Ok(prior);}
            let (mut run,assignment,_)=s.choreography_caller(attempt_id)?;
            if assignment["coordinator"] != true {return Err(conflict("Only the coordinator proposes choreography."));}
            let run_id_owned=text(&run,"id",1,100)?.to_owned();
            let run_id=run_id_owned.as_str();
            let previous=s.choreography(run_id)?;
            let rebind=run["implementationPlan"]["decision"]["basis"]["taskRevision"] != run["taskRevision"]
                || previous.as_ref().is_some_and(|p|p["binding"] != binding(&run));
            if rebind {
                if !array(&run["adoptions"]).iter().any(|a|a["toRevision"] == run["taskRevision"]) {return Err(conflict("A successor binding requires explicit operator requirements adoption."));}
                run["implementationPlan"]=s.current_implementation_plan(text(&run,"taskId",1,100)?)?.ok_or_else(||conflict("Accept a current implementation plan before reconciling adopted requirements."))?;
            }
            if previous.as_ref().map(|p| p["revision"].clone()).unwrap_or(json!(0)) != input["revision"] {
                return Err(conflict("Choreography changed; reload before proposing."));
            }
            let assignments = s.run_records("assignments",run_id)?;
            // A retired operator consultant may precede the first schedule.
            // It has no predecessor package history to reopen.
            let admitted = previous.is_some() && assignments.iter().any(|a| a["coordinator"] != true);
            let affected = array(&input["reopenPackages"]);
            if input.get("reopenPackages").is_some() && (input["reopenPackages"].as_array().is_none() || affected.len()>100 || affected.iter().any(|v|v.as_str().is_none()) || affected.iter().collect::<std::collections::HashSet<_>>().len()!=affected.len()) {return Err(conflict("Affected packages must be a bounded distinct list of IDs."));}
            if rebind && previous.as_ref().is_some_and(|prior|array(&prior["packages"]).iter().any(|p|!affected.contains(&p["id"])) || array(&run["implementationPlan"]["content"]["workPackages"]).iter().any(|p|!affected.contains(&p["id"]))) {return Err(conflict("Adopted requirements require fresh obligations for every old and current package."));}
            if assignments.iter().any(|a| a["coordinator"] != true && a["retiredAt"].is_null()) {
                return Err(conflict("Retire and reconcile current specialists before proposing a successor schedule."));
            }
            if array(&input["schedule"]["assignments"]).iter().any(|m| assignments.iter().any(|a| a["name"] == m["name"] && (a["coordinator"] == true || rebind || previous.as_ref().is_none_or(|p|member(p,&m["name"]).map_or(true, |old|old != m))))) {
                return Err(conflict("An admitted assignment brief is immutable; use a fresh name for changed responsibilities."));
            }
            if admitted && (affected.is_empty() || previous.is_none()) {
                return Err(conflict("A successor schedule must identify affected packages and retain its predecessor."));
            }
            if let Some(prior) = &previous {
                if !rebind && affected.iter().any(|id| !array(&prior["packages"]).iter().any(|p|p["id"] == *id && p["state"] == "complete")) {
                    return Err(conflict("Successor choreography requires completed affected packages."));
                }
                for package in array(&prior["packages"]).iter().filter(|p| !affected.contains(&p["id"])) {
                    let owners = |schedule: &Value| -> Vec<Value> { array(&schedule["assignments"]).iter().filter(|m|array(&m["packages"]).contains(&package["id"])).cloned().collect() };
                    if admitted && owners(&prior["schedule"]) != owners(&input["schedule"]) { return Err(conflict("A successor must preserve unaffected package responsibilities.")); }
                }
            }
            if run["implementationPlan"]["decision"]["decision"] != "accept" {
                return Err(conflict("Choreography requires an accepted implementation plan."));
            }
            Schedule::parse(&run["implementationPlan"], &input["schedule"])?;
            let packages:Vec<_>=array(&run["implementationPlan"]["content"]["workPackages"]).iter()
                .map(|p| json!({"id":p["id"],"dependsOn":p["dependsOn"],"state":"proposed","generation":1})).collect();
            let mut state=json!({"policyVersion":1,"runId":run_id,"revision":input["revision"],"binding":binding(&run),"schedule":input["schedule"],"packages":packages,"reports":[],"exchanges":[],"handoffs":[]});
            if admitted {
                let prior = previous.as_ref().unwrap();
                for key in ["reports","exchanges","handoffs"] {state[key] = prior[key].clone();}
                if rebind {
                    for package in state["packages"].as_array_mut().unwrap() {
                        if let Some(old)=array(&prior["packages"]).iter().find(|p|p["id"]==package["id"]) {package["generation"]=json!(old["generation"].as_u64().unwrap()+1);}
                    }
                } else {state["packages"] = prior["packages"].clone();}
                state["predecessorRevision"] = prior["revision"].clone();
                state["reopenPackages"] = json!(affected);
            }
            if rebind {s.save_run(&run)?;}
            s.append_choreography(run_id,&mut state,json!({"kind":if admitted {"successor_proposed"} else {"proposed"},"attemptId":attempt_id}))?;
            s.receipt(&receipt,"choreography",&payload,&state)?;
            Ok(state)
        })
    }
    /// Resolve durable request identity before observing mutable repository files.
    pub fn replay_package(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
    ) -> Result<Option<Value>> {
        text(&json!({"key":key}), "key", 1, 160)?;
        self.replay(
            &format!("package:{attempt_id}:{key}"),
            "package",
            &json!({"attemptId":attempt_id,"input":input}),
        )
    }
    /// Package reports and decisions share one CAS ledger. Runtime code supplies
    /// an observed source identity after verifying referenced repository files.
    pub fn record_package(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
        source: &Value,
    ) -> Result<Value> {
        strict(
            input,
            &[
                "revision",
                "packageId",
                "kind",
                "summary",
                "artifacts",
                "documentation",
            ],
        )?;
        text(&json!({"key":key}), "key", 1, 160)?;
        let package = text(input, "packageId", 1, 64)?;
        let kind = text(input, "kind", 1, 20)?;
        trimmed_text(input, "summary", 1, 8000)?;
        if !["start", "block", "resolve", "report", "complete", "reopen"].contains(&kind) {
            return Err(conflict("Unknown package action."));
        }
        validate_artifacts(input)?;
        let receipt = format!("package:{attempt_id}:{key}");
        self.transaction(|s| {
            // Source observations may change after a successful response is lost.
            // Retry identity is the caller's immutable request, not a later read.
            let payload=json!({"attemptId":attempt_id,"input":input});
            if let Some(prior)=s.replay(&receipt,"package",&payload)? {return Ok(prior);}
            let (run,assignment,_)=s.choreography_caller(attempt_id)?;
            let run_id=text(&run,"id",1,100)?;
            let mut state=s.current_choreography(&run,&input["revision"])?;
            let package_before=package_mut(&mut state,package)?.clone();
            if kind == "report" {
                let responsibility=member(&state,&assignment["name"])?.clone();
                if !array(&responsibility["packages"]).contains(&json!(package)) || responsibility["responsibility"] == "reviewer" {
                    return Err(conflict("This assignment has no contribution to this package."));
                }
                if !["active","blocked"].contains(&package_before["state"].as_str().unwrap_or("")) {
                    return Err(conflict("Only active package work accepts reports."));
                }
                if responsibility["responsibility"] == "documentation" {
                    let doc=&input["documentation"];
                    strict(doc,&["disposition","reason"])?;
                    trimmed_text(doc,"reason",1,4000)?;
                    if !["changed","no_change"].contains(&doc["disposition"].as_str().unwrap_or(""))
                        || doc["disposition"] == "changed" && array(&input["artifacts"]).is_empty() {
                        return Err(conflict("Documentation needs changed artifacts or a reasoned no-change disposition."));
                    }
                } else if !input["documentation"].is_null() {
                    return Err(conflict("Only the documentation owner records its disposition."));
                }
                state["reports"].as_array_mut().unwrap().push(json!({"id":crate::store::id(),"packageId":package,"generation":package_before["generation"],"assignmentId":assignment["id"],"attemptId":attempt_id,"name":assignment["name"],"responsibility":responsibility["responsibility"],"source":source,"summary":input["summary"],"artifacts":input["artifacts"],"documentation":input["documentation"],"snapshots":source["artifacts"],"at":s.time()}));
            } else {
                if assignment["coordinator"] != true {return Err(conflict("Only the coordinator reconciles package state."));}
                let next=match kind {"start"|"resolve"|"reopen"=>PackageState::Active,"block"=>PackageState::Blocked,_=>PackageState::Complete};
                let before:PackageState=serde_json::from_value(package_before["state"].clone())?;
                let rework=kind == "reopen" && s.package_rework_authorized(run_id)?;
                if !before.permits(next,rework) {return Err(conflict("This package transition is not permitted."));}
                if kind == "start" || kind == "reopen" {
                    for dep in array(&package_before["dependsOn"]) {
                        if !array(&state["packages"]).iter().any(|p|p["id"]==*dep && p["state"]=="complete") {return Err(conflict("Package dependencies are not complete."));}
                    }
                    let owners=s.run_records("assignments",run_id)?;
                    for responsibility in ["writer","navigator"] {
                        if !owners.iter().any(|a| a["retiredAt"].is_null() && member(&state,&a["name"]).is_ok_and(|m|m["responsibility"]==responsibility && array(&m["packages"]).contains(&json!(package)))) {return Err(conflict("Admit both pair assignments before starting their package."));}
                    }
                }
                if kind == "complete" {
                    let blockers=s.package_blockers(&state,package)?;
                    if !blockers.is_empty() {return Err(conflict(&blockers.join(" ")));}
                }
                let current=package_mut(&mut state,package)?;
                current["state"]=serde_json::to_value(next)?;
                current["reason"]=input["summary"].clone();
                if kind == "reopen" {current["generation"]=json!(current["generation"].as_u64().unwrap()+1);}
                if kind == "complete" {current["source"]=source.clone();}
            }
            s.append_choreography(run_id,&mut state,json!({"kind":kind,"packageId":package,"attemptId":attempt_id,"summary":input["summary"]}))?;
            s.receipt(&receipt,"package",&payload,&state)?;
            Ok(state)
        })
    }
}
fn validate_artifacts(input: &Value) -> Result<()> {
    let artifacts = input["artifacts"]
        .as_array()
        .filter(|a| a.len() <= 50)
        .ok_or_else(|| {
            conflict("Supply a bounded artifact list, empty for a reasoned no-change report.")
        })?;
    for artifact in artifacts {
        strict(artifact, &["path", "sha256"])?;
        text(artifact, "path", 1, 1000)?;
        let digest = text(artifact, "sha256", 64, 64)?;
        if !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(conflict("Artifacts need SHA-256 identities."));
        }
    }
    Ok(())
}
