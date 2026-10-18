use super::*;
impl Store {
    pub(super) fn package_rework_authorized(&self, run_id: &str) -> Result<bool> {
        let work = self.repository_work(run_id)?.unwrap_or(Value::Null);
        let coding = self.coding_record(run_id)?.unwrap_or(Value::Null);
        let candidate = array(&coding["candidates"]).last();
        Ok(candidate.is_some_and(|candidate| {
            array(&coding["claims"]).iter().any(|c| {
                c["writerAttemptId"] == work["writer"]["attemptId"]
                    && c["candidateId"] == candidate["id"]
            })
        }))
    }
    pub(super) fn package_blockers(&self, state: &Value, package: &str) -> Result<Vec<String>> {
        let current = array(&state["packages"])
            .iter()
            .find(|p| p["id"] == package)
            .ok_or_else(|| conflict("Unknown package."))?;
        let mut blockers = Vec::new();
        for owner in array(&state["schedule"]["assignments"]).iter().filter(|m| {
            !["reviewer", "reporter"].contains(&m["responsibility"].as_str().unwrap_or(""))
                && array(&m["packages"]).contains(&json!(package))
        }) {
            if !array(&state["reports"]).iter().any(|r| {
                r["packageId"] == package
                    && r["generation"] == current["generation"]
                    && r["name"] == owner["name"]
            }) {
                blockers.push(format!(
                    "Package {package} needs a report from {}.",
                    owner["name"].as_str().unwrap_or("its owner")
                ));
            }
        }
        blockers.extend(self.pair_blockers(state, package)?);
        Ok(blockers)
    }
    pub(super) fn pair_blockers(&self, state: &Value, package: &str) -> Result<Vec<String>> {
        let current = array(&state["packages"])
            .iter()
            .find(|p| p["id"] == package)
            .ok_or_else(|| conflict("Unknown package."))?;
        let mut blockers = Vec::new();
        let exchanges: Vec<_> = array(&state["exchanges"])
            .iter()
            .filter(|e| e["packageId"] == package && e["generation"] == current["generation"])
            .collect();
        for kind in ["approach", "challenge", "response", "resolution"] {
            if !exchanges.iter().any(|e| e["kind"] == kind) {
                blockers.push(format!("Package {package} needs a navigator {kind}."));
            }
        }
        for exchange in &exchanges {
            let message = self.get_peer_message(text(exchange, "messageId", 1, 100)?)?;
            if message["disposition"] != "received" {
                blockers.push(format!("Package {package} exchange awaits runner receipt."));
            }
            if exchange["kind"] == "challenge"
                && !exchanges
                    .iter()
                    .any(|e| e["kind"] == "resolution" && e["challengeId"] == exchange["id"])
            {
                blockers.push(format!(
                    "Package {package} has an unresolved navigator concern."
                ));
            }
        }
        Ok(blockers)
    }
    pub fn choreography_blockers(&self, run_id: &str) -> Result<Vec<String>> {
        let run = self.get_run(run_id)?;
        let Some(state) = self.choreography(run_id)? else {
            return Ok(if run["executionPolicy"] == 1 {
                vec!["The coordinator must propose package choreography.".into()]
            } else {
                vec![]
            });
        };
        let mut blockers = Vec::new();
        if state["binding"] != binding(&run) {
            blockers.push("Choreography covers superseded requirements or plan.".into());
        }
        for package in array(&state["packages"]) {
            let id = text(package, "id", 1, 64)?;
            if package["state"] != "complete" {
                blockers.push(format!("Package {id} is not complete."));
            }
            blockers.extend(self.package_blockers(&state, id)?);
        }
        Ok(blockers)
    }
    pub fn assert_choreography_reviewer(
        &self,
        assignment_id: &str,
        attempt_id: Option<&str>,
    ) -> Result<()> {
        let reviewer = self.get_assignment(assignment_id)?;
        let run_id = text(&reviewer, "runId", 1, 100)?;
        let Some(state) = self.choreography(run_id)? else {
            if self.get_run(run_id)?["executionPolicy"] == 1 {
                return Err(conflict("Package choreography is missing."));
            }
            return Ok(());
        };
        if member(&state, &reviewer["name"])?["responsibility"] != "reviewer" {
            return Err(conflict(
                "The pair and contributors cannot perform final independent review.",
            ));
        }
        let session = attempt_id
            .map(|id| self.get_attempt(id))
            .transpose()?
            .unwrap_or(Value::Null)["sessionId"]
            .clone();
        for assignment in self.run_records("assignments", run_id)? {
            if member(&state, &assignment["name"]).is_ok_and(|m| {
                !["reviewer", "reporter"].contains(&m["responsibility"].as_str().unwrap_or(""))
            }) || assignment["choreography"]["responsibility"]["responsibility"]
                .as_str()
                .is_some_and(|r| !["reviewer", "reporter"].contains(&r))
            {
                let mut q = self
                    .db
                    .prepare("SELECT data FROM attempts WHERE assignment_id=?")?;
                let attempts = q.query_map([assignment["id"].as_str().unwrap()], |r| {
                    r.get::<_, String>(0)
                })?;
                for prior in attempts {
                    let prior = crate::store::parse(prior?)?;
                    if assignment["id"] == reviewer["id"]
                        || !session.is_null() && session == prior["sessionId"]
                    {
                        return Err(conflict(
                            "Final reviewer shares a contributor assignment or session.",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
