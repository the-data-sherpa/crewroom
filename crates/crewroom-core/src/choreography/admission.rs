use super::*;
impl Store {
    /// Production entry points capture this before first admission. Recovery of
    /// already-admitted legacy runs leaves the absent policy untouched.
    pub fn capture_execution_policy(&self, run_id: &str) -> Result<Value> {
        self.transaction(|s| {
            let mut run = s.get_run(run_id)?;
            if run["purpose"].as_str().unwrap_or("coding") == "coding"
                && s.run_records("assignments", run_id)?.is_empty()
            {
                run["executionPolicy"] = json!(1);
                s.save_run(&run)?;
            }
            Ok(run)
        })
    }
    pub fn choreography_assignment(
        &self,
        run_id: &str,
        name: &Value,
        role: &Value,
    ) -> Result<Option<Value>> {
        let run = self.get_run(run_id)?;
        if run["executionPolicy"] != 1 && self.choreography(run_id)?.is_none() {
            return Ok(None);
        }
        let state = self
            .choreography(run_id)?
            .unwrap_or_else(|| json!({"revision":0,"binding":binding(&run)}));
        if state["binding"] != binding(&run) {
            return Err(conflict(
                "Choreography belongs to an older plan or requirements revision.",
            ));
        }
        let scheduled = match member(&state, name) {
            Ok(member) => member.clone(),
            Err(error) => {
                // Operator insertion is a supplemental read-only responsibility.
                // Its existing proposal/retirement ledger gates readiness without
                // rewriting the frozen implementation schedule or its evidence.
                let proposal = self.team_proposals(run_id)?.into_iter().find(|p| {
                    p["origin"] == "operator"
                        && ["pending", "applied"].contains(&p["status"].as_str().unwrap_or(""))
                        && array(&p["specialists"]).iter().any(|m| m["name"] == *name)
                });
                let Some(proposal) = proposal else {
                    return Err(error);
                };
                let selected = array(&proposal["specialists"])
                    .iter()
                    .find(|m| m["name"] == *name)
                    .unwrap();
                let packages: Vec<_> = array(&run["implementationPlan"]["content"]["workPackages"])
                    .iter()
                    .filter(|p| {
                        array(&proposal["request"]["criteria"]).is_empty()
                            || array(&p["criteria"])
                                .iter()
                                .any(|c| array(&proposal["request"]["criteria"]).contains(c))
                    })
                    .map(|p| p["id"].clone())
                    .collect();
                json!({"name":name,"roleId":selected["role"]["id"],"responsibility":"consultant","writes":false,"packages":packages,"operatorProposalId":proposal["id"],"rationale":"Operator-requested supplemental assessment","input":selected["input"]})
            }
        };
        if scheduled["roleId"] != role["id"] {
            return Err(conflict(
                "Role differs from the named package responsibility.",
            ));
        }
        let packages: Vec<_> = array(&run["implementationPlan"]["content"]["workPackages"])
            .iter()
            .filter(|p| array(&scheduled["packages"]).contains(&p["id"]))
            .cloned()
            .collect();
        Ok(Some(
            json!({"policyVersion":1,"revision":state["revision"],"binding":state["binding"],"responsibility":scheduled,"packages":packages}),
        ))
    }
    pub fn validate_choreography_team(
        &self,
        run_id: &str,
        additions: &[Value],
        retirements: &[Value],
    ) -> Result<()> {
        let run = self.get_run(run_id)?;
        let Some(state) = self.choreography(run_id)? else {
            if run["executionPolicy"] == 1 {
                for addition in additions {
                    self.choreography_assignment(run_id, &addition["name"], &addition["role"])?;
                }
            }
            return Ok(());
        };
        let assignments = self.run_records("assignments", run_id)?;
        let retiring = |a: &Value| retirements.iter().any(|r| r["assignmentId"] == a["id"]);
        // Explicit requirements adoption invalidates these obligations. The
        // existing retirement path still stops/reconciles ownership and Mail.
        let superseded = state["binding"] != binding(&run);
        for a in assignments.iter().filter(|a| retiring(a)) {
            if superseded {
                continue;
            }
            let Ok(scheduled) = member(&state, &a["name"]) else {
                continue;
            };
            if !["reviewer", "reporter"]
                .contains(&scheduled["responsibility"].as_str().unwrap_or(""))
            {
                for p in array(&scheduled["packages"]) {
                    let package = array(&state["packages"])
                        .iter()
                        .find(|v| v["id"] == *p)
                        .unwrap();
                    if !array(&state["reports"]).iter().any(|r| {
                        r["assignmentId"] == a["id"]
                            && r["packageId"] == *p
                            && r["generation"] == package["generation"]
                    }) {
                        return Err(conflict(
                            "Reconcile the specialist's package report before retirement.",
                        ));
                    }
                    if ["writer", "navigator"]
                        .contains(&scheduled["responsibility"].as_str().unwrap_or(""))
                        && !self.pair_blockers(&state, p.as_str().unwrap())?.is_empty()
                    {
                        return Err(conflict(
                            "Resolve and deliver pair exchanges before retiring the pair.",
                        ));
                    }
                }
            }
        }
        let mut stages = std::collections::HashSet::new();
        for a in assignments
            .iter()
            .filter(|a| a["coordinator"] != true && a["retiredAt"].is_null() && !retiring(a))
        {
            if let Ok(scheduled) = member(&state, &a["name"]) {
                stages.insert(scheduled["stage"].as_u64().unwrap());
            }
        }
        for addition in additions {
            self.choreography_assignment(run_id, &addition["name"], &addition["role"])?;
            let Ok(scheduled) = member(&state, &addition["name"]) else {
                continue;
            };
            let stage = scheduled["stage"].as_u64().unwrap();
            stages.insert(stage);
            for prior in array(&state["schedule"]["assignments"])
                .iter()
                .filter(|p| p["stage"].as_u64().unwrap() < stage)
            {
                if !assignments.iter().any(|a| {
                    a["name"] == prior["name"] && (!a["retiredAt"].is_null() || retiring(a))
                }) {
                    return Err(conflict(
                        "Complete and reconcile the earlier stage before admitting its successor.",
                    ));
                }
            }
            if scheduled["responsibility"] == "reviewer"
                && self
                    .coding_record(run_id)?
                    .is_none_or(|c| array(&c["candidates"]).is_empty())
            {
                return Err(conflict(
                    "Freeze a candidate before admitting final review.",
                ));
            }
        }
        if stages.len() > 1 {
            return Err(conflict(
                "Retire the current stage before admitting another stage.",
            ));
        }
        Ok(())
    }
    pub fn assert_choreography_writer(
        &self,
        attempt_id: &str,
        claim: Option<&Value>,
    ) -> Result<()> {
        let attempt = self.get_attempt(attempt_id)?;
        let run_id = text(&attempt, "runId", 1, 100)?;
        let run = self.get_run(run_id)?;
        let Some(state) = self.choreography(run_id)? else {
            return if run["executionPolicy"] == 1 {
                Err(conflict(
                    "Package choreography must be admitted before writing.",
                ))
            } else {
                Ok(())
            };
        };
        let assignment = self.get_assignment(text(&attempt, "assignmentId", 1, 100)?)?;
        let responsibility = member(&state, &assignment["name"])?;
        if responsibility["writes"] != true || state["binding"] != binding(&run) {
            return Err(conflict(
                "This assignment has no current writing responsibility.",
            ));
        }
        let coding = self.coding_record(run_id)?.unwrap_or(Value::Null);
        let rework = claim.is_some_and(|c| {
            array(&coding["candidates"])
                .last()
                .is_some_and(|candidate| c["candidateId"] == candidate["id"])
        });
        if !array(&state["packages"]).iter().any(|p| {
            array(&responsibility["packages"]).contains(&p["id"])
                && (p["state"] == "active"
                    || (rework
                        || claim.is_none()
                            && self
                                .repository_work(run_id)
                                .ok()
                                .flatten()
                                .is_some_and(|w| w["writer"]["attemptId"] == attempt_id))
                        && p["state"] == "complete")
        }) {
            return Err(conflict(
                "Start a dependency-ready package before claiming writes.",
            ));
        }
        if claim.is_none()
            && self
                .repository_work(run_id)?
                .is_none_or(|w| w["writer"]["attemptId"] != attempt_id)
        {
            return Err(conflict(
                "The coordinator must grant this package's writer claim.",
            ));
        }
        // A completed checkpoint names exactly the next allowed writer. A
        // surviving old attempt cannot reacquire the released lease.
        if let Some(checkpoint) = array(&coding["completions"])
            .iter()
            .rev()
            .find(|c| c["mode"] == "checkpoint" && c["state"] == "completed")
        {
            let same_assignment = self.get_attempt(text(checkpoint, "attemptId", 1, 100)?)?["assignmentId"]
                == assignment["id"];
            let later = array(&coding["claims"])
                .iter()
                .any(|c| c["handoffId"] == checkpoint["id"]);
            if !later && checkpoint["successor"] != assignment["name"] || same_assignment && !later
            {
                return Err(conflict(
                    "Checkpoint ownership is reserved for its named successor.",
                ));
            }
        }
        Ok(())
    }
    pub fn assert_package_edit(&self, attempt_id: &str) -> Result<()> {
        let attempt = self.get_attempt(attempt_id)?;
        let run_id = text(&attempt, "runId", 1, 100)?;
        let Some(state) = self.choreography(run_id)? else {
            return Ok(());
        };
        let assignment = self.get_assignment(text(&attempt, "assignmentId", 1, 100)?)?;
        let responsibility = member(&state, &assignment["name"])?;
        if !array(&state["packages"]).iter().any(|p| {
            p["state"] == "active" && array(&responsibility["packages"]).contains(&p["id"])
        }) {
            return Err(conflict(
                "Reopen the affected package before changing closed work.",
            ));
        }
        Ok(())
    }
}
