use super::*;
fn review_input(input: &Value) -> Result<Value> {
    request(
        input,
        &["candidateId", "verdict", "summary", "findings", "coverage"],
    )?;
    let verdict = text(input, "verdict")?;
    if !["pass", "changes_requested"].contains(&verdict) {
        return Err(Error::Invalid("Invalid review verdict.".into()));
    }
    let findings = input["findings"]
        .as_array()
        .filter(|v| v.len() <= 200)
        .ok_or_else(|| Error::Invalid("Invalid findings.".into()))?
        .iter()
        .map(|f| {
            request(f, &["path", "line", "severity", "body"])?;
            let mut v = json!({});
            if f.get("path").is_some() {
                v["path"] = json!(limited(f, "path", 0, 1000)?);
            }
            if f.get("line").is_some() {
                v["line"] = json!(number(f, "line", 1, 9007199254740991)?);
            }
            let severity = text(f, "severity")?;
            if !["blocker", "major", "minor"].contains(&severity) {
                return Err(Error::Invalid("Invalid finding severity.".into()));
            }
            v["severity"] = json!(severity);
            v["body"] = json!(prose(f, "body", 4000)?);
            Ok(v)
        })
        .collect::<Result<Vec<_>>>()?;
    let coverage=input["coverage"].as_array().filter(|v|v.len()<=100).ok_or_else(||Error::Invalid("Invalid coverage.".into()))?.iter().map(|c|{
        request(c,&["criterion","assessment","checkIds"])?;let checks=c["checkIds"].as_array().filter(|v|v.len()<=100 && v.iter().all(Value::is_string)).ok_or_else(||Error::Invalid("Invalid covered check IDs.".into()))?;
        Ok(json!({"criterion":number(c,"criterion",0,9007199254740991)?,"assessment":prose(c,"assessment",4000)?,"checkIds":checks}))
    }).collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"candidateId":uuid_field(input,"candidateId")?,"verdict":verdict,"summary":prose(input,"summary",12000)?,"findings":findings,"coverage":coverage}),
    )
}
impl<S: StoreAccess> Evidence<S> {
    pub fn assign_review(&self, attempt_id: &str, key: &str, input: &Value) -> Result<Value> {
        request(input, &["assignee", "candidateId", "instruction"])?;
        limited(&json!({"key":key}), "key", 1, 200)?;
        let assignee = limited(input, "assignee", 1, usize::MAX)?;
        let candidate = uuid_field(input, "candidateId")?;
        let instruction = prose(input, "instruction", 40000)?;
        self.work.store(|s|{
            let attempt=s.get_attempt(attempt_id)?;let run_id=text(&attempt,"runId")?;let run=s.get_run(run_id)?;let runtime=s.runtime(run_id)?;let team=array(&runtime["assignments"]);
            if attempt["ownership"]!="held" || run["status"]!="running" || run["desiredState"]!="running" || !team.iter().any(|a|a["coordinator"]==true && a["currentAttemptId"]==attempt_id){return Err(conflict("Only the active coordinator may assign review."));}
            let record=Self::record(s,run_id)?;let reviewer=team.iter().find(|a|a["name"]==assignee && a["coordinator"]!=true && a.get("retiredAt").is_none()).ok_or_else(||conflict("Review requires a completed candidate and an admitted specialist."))?;
            if !array(&record["candidates"]).iter().any(|c|c["id"]==candidate){return Err(conflict("Review requires a completed candidate and an admitted specialist."));}
            s.assert_choreography_reviewer(text(reviewer,"id")?,None)?;
            for c in array(&record["candidates"]){for id in array(&c["implementationAttempts"]){if s.get_attempt(id.as_str().ok_or_else(||conflict("Invalid author identity."))?)?["assignmentId"]==reviewer["id"]{return Err(conflict("Review must be independent of implementation."));}}}
            if let Some(prior)=array(&record["reviewAssignments"]).iter().find(|a|a["coordinatorAttemptId"]==attempt_id && a["toolCallId"]==key){if prior["assignmentId"]!=reviewer["id"] || prior["candidateId"]!=candidate || prior["instruction"]!=instruction{return Err(conflict("Review assignment has different content."));}return Ok(prior.clone());}
            let assignment=json!({"id":uuid::Uuid::new_v4().to_string(),"coordinatorAttemptId":attempt_id,"toolCallId":key,"assignmentId":reviewer["id"],"candidateId":candidate,"instruction":instruction});
            s.update_coding_record(run_id,|previous|{let mut record=previous.unwrap();let mut assignments=array(&record["reviewAssignments"]).to_vec();assignments.push(assignment.clone());record["reviewAssignments"]=json!(assignments);Ok(record)})?;Ok(assignment)
        })
    }
    fn reviewer(s: &Store, attempt_id: &str, value: &Value, record: &Value) -> Result<Value> {
        let attempt = s.get_attempt(attempt_id)?;
        let run_id = text(&attempt, "runId")?;
        let run = s.get_run(run_id)?;
        let runtime = s.runtime(run_id)?;
        let assignment = array(&runtime["assignments"])
            .iter()
            .find(|a| a["currentAttemptId"] == attempt_id)
            .ok_or_else(|| {
                conflict("Only a current admitted review attempt may submit evidence.")
            })?;
        if !array(&record["candidates"])
            .iter()
            .any(|c| c["id"] == value["candidateId"])
            || attempt["sessionId"].as_str().is_none_or(str::is_empty)
            || attempt["ownership"] != "held"
            || run["status"] != "running"
            || run["desiredState"] != "running"
        {
            return Err(conflict(
                "Only a current admitted review attempt may submit evidence.",
            ));
        }
        s.assert_choreography_reviewer(text(assignment, "id")?, Some(attempt_id))?;
        for c in array(&record["candidates"]) {
            for id in array(&c["implementationAttempts"]) {
                let author = s.get_attempt(
                    id.as_str()
                        .ok_or_else(|| conflict("Invalid author identity."))?,
                )?;
                if author["assignmentId"] == assignment["id"]
                    || author["sessionId"] == attempt["sessionId"]
                {
                    return Err(conflict(
                        "Review must be independent of every implementation assignment and session.",
                    ));
                }
            }
        }
        if !array(&record["reviewAssignments"]).iter().any(|a| {
            a["assignmentId"] == assignment["id"] && a["candidateId"] == value["candidateId"]
        }) {
            return Err(conflict("Review was not assigned for this candidate."));
        }
        let coverage = array(&value["coverage"]);
        let criteria = array(&run["taskSnapshot"]["criteria"]);
        if coverage.iter().any(|c| {
            c["criterion"].as_u64().unwrap() >= criteria.len() as u64
                || array(&c["checkIds"]).iter().any(|id| {
                    !array(&record["requiredChecks"])
                        .iter()
                        .any(|check| check["id"] == *id)
                })
        }) || coverage
            .iter()
            .map(|c| c["criterion"].as_u64())
            .collect::<HashSet<_>>()
            .len()
            != coverage.len()
        {
            return Err(conflict(
                "Review coverage must reference distinct accepted criteria and configured checks.",
            ));
        }
        if value["verdict"] == "pass"
            && (coverage.len() != criteria.len()
                || array(&value["findings"])
                    .iter()
                    .any(|f| f["severity"] != "minor"))
        {
            return Err(conflict(
                "Passing review must cover every criterion and leave no blocking findings.",
            ));
        }
        let mut body = value.clone();
        body["attemptId"] = json!(attempt_id);
        body["assignmentId"] = assignment["id"].clone();
        body["sessionId"] = attempt["sessionId"].clone();
        body["checkPolicyRevision"] = json!(array(&record["checkPolicyChanges"]).len());
        Ok(body)
    }
    pub fn review(&self, attempt_id: &str, input: &Value) -> Result<Value> {
        let value = review_input(input)?;
        let (run_id, record, body) = self.work.store(|s| {
            let a = s.get_attempt(attempt_id)?;
            let run_id = text(&a, "runId")?.to_owned();
            let record = Self::record(s, &run_id)?;
            let body = Self::reviewer(s, attempt_id, &value, &record)?;
            Ok((run_id, record, body))
        })?;
        let candidate = array(&record["candidates"])
            .iter()
            .find(|c| c["id"] == value["candidateId"])
            .unwrap();
        if value["verdict"] == "pass" && !self.check_blockers(&record, candidate).0.is_empty() {
            return Err(conflict(
                "Required checks must pass for this exact candidate before review can pass.",
            ));
        }
        let artifact = self.artifact(&stringify(&body))?;
        self.work.store(|s| {
            if Self::record(s, &run_id)? != record
                || Self::reviewer(s, attempt_id, &value, &record)? != body
            {
                return Err(conflict(
                    "Review authorization or evidence changed during verification.",
                ));
            }
            let mut receipt = body;
            receipt["id"] = json!(uuid::Uuid::new_v4().to_string());
            receipt["artifact"] = artifact;
            receipt["createdAt"] = json!(time());
            s.update_coding_record(&run_id, |previous| {
                let mut record = previous.unwrap();
                record["reviews"]
                    .as_array_mut()
                    .ok_or_else(|| conflict("Review ledger unavailable."))?
                    .push(receipt.clone());
                Ok(record)
            })?;
            Ok(receipt)
        })
    }
}
