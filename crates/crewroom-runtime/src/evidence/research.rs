use super::*;
#[derive(Clone)]
pub struct Research<S: StoreAccess> {
    evidence: Evidence<S>,
}
fn whitespace(c: char) -> bool {
    crewroom_core::store::trim_text(c.encode_utf8(&mut [0; 4])).is_empty()
}
fn normalize(s: &str) -> String {
    s.split(whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn integer(v: &Value, key: &str) -> Result<i64> {
    v[key]
        .as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0. && n.abs() <= 9007199254740991.)
        .map(|n| n as i64)
        .ok_or_else(|| Error::Invalid(format!("Invalid {key}.")))
}
fn input(v: &Value) -> Result<Value> {
    request(
        v,
        &[
            "title",
            "kind",
            "body",
            "citations",
            "limitations",
            "coverage",
        ],
    )?;
    let kind = text(v, "kind")?;
    if !["research", "design", "documentation-draft"].contains(&kind) {
        return Err(Error::Invalid("Invalid research artifact kind.".into()));
    }
    let citations=v["citations"].as_array().filter(|a|a.len()<=100).ok_or_else(||Error::Invalid("Invalid citations.".into()))?.iter().map(|c|{
        request(c,&["path","commit","startLine","endLine","excerpt"])?;
        Ok(json!({"path":limited(c,"path",1,1000)?,"commit":limited(c,"commit",1,64)?,"startLine":integer(c,"startLine")?,"endLine":integer(c,"endLine")?,"excerpt":limited(c,"excerpt",0,20000)?}))
    }).collect::<Result<Vec<_>>>()?;
    let mut value = json!({"title":prose(v,"title",200)?,"kind":kind,"body":limited(v,"body",1,1024*1024)?,"citations":citations,"limitations":"","coverage":[]});
    if v.get("limitations").is_some() {
        value["limitations"] = json!(limited(v, "limitations", 0, 20000)?);
    }
    if v.get("coverage").is_some() {
        let coverage=v["coverage"].as_array().filter(|a|a.len()<=100).ok_or_else(||Error::Invalid("Invalid research coverage.".into()))?.iter().map(|c|{request(c,&["criterionId","section"])?;Ok(json!({"criterionId":integer(c,"criterionId")?,"section":prose(c,"section",200)?}))}).collect::<Result<Vec<_>>>()?;
        value["coverage"] = json!(coverage);
    }
    Ok(value)
}
fn heading(line: &str) -> Option<String> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=4).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.chars().next().is_some_and(whitespace) {
        return None;
    }
    let rest = rest.trim_start_matches(whitespace);
    if rest.is_empty() || rest.contains(['\u{2028}', '\u{2029}']) {
        return None;
    }
    let rest = rest.trim_end_matches(whitespace);
    let end = rest.trim_end_matches('#').trim_end_matches(whitespace);
    // The retained non-greedy capture always consumes at least one character.
    Some(normalize(if end.is_empty() {
        &rest[..rest.chars().next()?.len_utf8()]
    } else {
        end
    }))
}
impl<S: StoreAccess> Research<S> {
    pub fn new(evidence: Evidence<S>) -> Self {
        Self { evidence }
    }
    fn replay(
        &self,
        artifacts: &[Value],
        attempt_id: &str,
        key: &str,
        value: &Value,
    ) -> Result<Option<Value>> {
        let Some(prior) = artifacts
            .iter()
            .find(|a| a["attemptId"] == attempt_id && a["toolCallId"] == key)
        else {
            return Ok(None);
        };
        let citations: Vec<_> = array(&prior["citations"])
            .iter()
            .map(|c| {
                let mut c = c.clone();
                if let Some(o) = c.as_object_mut() {
                    o.remove("verified");
                    o.remove("reason");
                }
                c
            })
            .collect();
        let previous = json!({"title":prior["title"],"kind":prior["kind"],"body":self.evidence.read_artifact(&prior["body"])?,"citations":citations,"limitations":prior["limitations"],"coverage":prior["coverage"]});
        if previous != *value {
            return Err(conflict("Artifact tool identity has different content."));
        }
        Ok(Some(prior.clone()))
    }
    fn authorize(s: &Store, attempt_id: &str) -> Result<(Value, Value, Value)> {
        let attempt = s.get_attempt(attempt_id)?;
        let run = s.get_run(text(&attempt, "runId")?)?;
        if run["purpose"] != "research" {
            return Err(conflict("Research artifacts require a research run."));
        }
        let assignment = array(&s.runtime(text(&run, "id")?)?["assignments"])
            .iter()
            .find(|a| {
                a["id"] == attempt["assignmentId"]
                    && a["currentAttemptId"] == attempt_id
                    && a.get("retiredAt").is_none()
            })
            .cloned()
            .ok_or_else(|| conflict("Only an active research agent may submit an artifact."))?;
        if attempt["ownership"] != "held"
            || !["idle", "executing"].contains(&attempt["state"].as_str().unwrap_or(""))
            || run["status"] != "running"
            || run["desiredState"] != "running"
            || run.get("sourceConflict").is_some()
        {
            return Err(conflict(
                "Only an active research agent may submit an artifact.",
            ));
        }
        Ok((attempt, run, assignment))
    }
    pub async fn submit(
        &self,
        attempt_id: &str,
        key: &str,
        request_value: &Value,
    ) -> Result<Value> {
        limited(&json!({"key":key}), "key", 1, 200)?;
        let value = input(request_value)?;
        let run_id = self
            .evidence
            .work
            .store(|s| Ok(text(&s.get_attempt(attempt_id)?, "runId")?.to_owned()))?;
        let lock = self.evidence.work.lock(&run_id);
        let _guard = lock.lock().await;
        let saved = self
            .evidence
            .work
            .store(|s| s.research_artifacts(&run_id))?;
        if let Some(prior) = self.replay(&saved, attempt_id, key, &value)? {
            return Ok(prior);
        }
        let (attempt, run, assignment) = self
            .evidence
            .work
            .store(|s| Self::authorize(s, attempt_id))?;
        let mut reasons: Vec<String> = Vec::new();
        if crewroom_core::store::trim_text(text(&value, "limitations")?).is_empty() {
            reasons.push("Limitations are required and cannot be empty.".into());
        }
        let coverage = array(&value["coverage"]);
        if coverage.is_empty() {
            reasons.push("Criterion coverage is required.".into());
        }
        let headings: HashSet<_> = text(&value, "body")?
            .split('\n')
            .filter_map(heading)
            .collect();
        for entry in coverage {
            let criterion = entry["criterionId"].as_i64().unwrap();
            if criterion < 0 || criterion as usize >= array(&run["taskSnapshot"]["criteria"]).len()
            {
                reasons.push(format!(
                    "Criterion {criterion} is not in accepted revision {}.",
                    run["taskRevision"]
                ));
            }
            let section = text(entry, "section")?;
            if !headings.contains(&normalize(section)) {
                reasons.push(format!(
                    "Criterion {criterion}: heading \"{section}\" is absent from the artifact."
                ));
            }
        }
        if coverage
            .iter()
            .map(|c| c["criterionId"].as_i64())
            .collect::<HashSet<_>>()
            .len()
            != coverage.len()
        {
            reasons.push("Criterion coverage entries must be distinct.".into());
        }
        let mut citations = Vec::new();
        for item in array(&value["citations"]) {
            let reason = if !run["researchSource"].is_object() {
                Some("No repository was pinned for this run.".into())
            } else if item["commit"] != run["researchSource"]["commit"] {
                Some(format!(
                    "Citation commit is not the pinned commit {}.",
                    text(&run["researchSource"], "commit")?
                ))
            } else {
                match self
                    .evidence
                    .work
                    .read_research(&run_id, text(item, "path")?)
                    .await
                {
                    Err(e) => Some(e.to_string()),
                    Ok(read) => {
                        let content = text(&read, "content")?.replace("\r\n", "\n");
                        let mut lines: Vec<_> = content.split('\n').collect();
                        if lines.last() == Some(&"") {
                            lines.pop();
                        }
                        let start = item["startLine"].as_i64().unwrap();
                        let end = item["endLine"].as_i64().unwrap();
                        let path = text(item, "path")?;
                        if start < 1 || end < start || end > lines.len() as i64 {
                            Some(format!(
                                "Line range {start}-{end} is outside {path} ({} lines).",
                                lines.len()
                            ))
                        } else if normalize(text(item, "excerpt")?)
                            != normalize(&lines[start as usize - 1..end as usize].join("\n"))
                        {
                            Some(format!("Excerpt does not match {path}:{start}-{end}."))
                        } else {
                            None
                        }
                    }
                }
            };
            let mut citation = item.clone();
            citation["verified"] = json!(reason.is_none());
            if let Some(reason) = reason {
                reasons.push(format!("Citation {}: {reason}", citations.len() + 1));
                citation["reason"] = json!(reason);
            }
            citations.push(citation);
        }
        if run["researchSource"].is_object() && citations.is_empty() {
            reasons.push(
                "A repository-backed artifact needs at least one verified source citation.".into(),
            );
        }
        let model = attempt.get("model").unwrap_or(&assignment["model"]);
        let mut provenance = json!({"requested":model["requested"],"selector":model["resolved"]["selector"],"provider":model["resolved"]["provider"],"id":model["resolved"]["id"],"effort":model["resolved"]["effort"]});
        if let Some(f) = model.get("fallbackFrom") {
            provenance["fallbackFrom"] = f.clone();
        }
        let mut artifact = json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run_id,"assignmentId":assignment["id"],"attemptId":attempt_id,"toolCallId":key,"model":provenance,"requirementRevision":run["taskRevision"],"title":value["title"],"kind":value["kind"],"body":self.evidence.artifact(text(&value,"body")?)?,"citations":citations,"limitations":value["limitations"],"coverage":coverage,"state":if reasons.is_empty(){"verified"}else{"rejected"},"reasons":reasons,"createdAt":time()});
        if let Some(commit) = run["researchSource"].get("commit") {
            artifact["pinnedCommit"] = commit.clone();
        }
        self.evidence.work.store(|s| {
            let (current, accepted, assigned) = Self::authorize(s, attempt_id)?;
            if current != attempt
                || assigned != assignment
                || accepted["taskRevision"] != run["taskRevision"]
                || accepted["researchSource"] != run["researchSource"]
            {
                return Err(conflict(
                    "Research requirements, source or assignment changed during submission.",
                ));
            }
            s.save_research_artifact(&artifact)
        })
    }
    pub fn read(&self, run_id: &str) -> Result<Value> {
        let (run, artifacts, insertion) = self.evidence.work.store(|s| {
            Ok((
                s.get_run(run_id)?,
                s.research_artifacts(run_id)?,
                s.insertion_blockers(run_id)?,
            ))
        })?;
        if run["purpose"] != "research" {
            return Err(conflict("This is not a research run."));
        }
        let current: Vec<_> = artifacts
            .iter()
            .filter(|a| {
                a["state"] == "verified"
                    && a.get("supersededBy").is_none()
                    && a["requirementRevision"] == run["taskRevision"]
                    && a["pinnedCommit"] == run["researchSource"]["commit"]
            })
            .collect();
        let valid: Vec<_> = current
            .iter()
            .filter(|a| self.evidence.read_artifact(&a["body"]).is_ok())
            .collect();
        let mut covered: Vec<_> = valid
            .iter()
            .flat_map(|a| {
                array(&a["coverage"])
                    .iter()
                    .filter_map(|c| c["criterionId"].as_i64())
            })
            .collect();
        covered.sort_unstable();
        covered.dedup();
        let mut blockers: Vec<_> = array(&run["taskSnapshot"]["criteria"])
            .iter()
            .enumerate()
            .filter(|(i, _)| !covered.contains(&(*i as i64)))
            .map(|(i, c)| {
                format!(
                    "Criterion {i}: {} has no verified current artifact.",
                    c.as_str().unwrap_or("")
                )
            })
            .collect();
        blockers.extend(insertion.clone());
        if current.len() != valid.len() {
            blockers.push("A current artifact is missing or its SHA-256 changed.".into());
        }
        self.evidence.work.store(|s| {
            if s.get_run(run_id)? != run
                || s.research_artifacts(run_id)? != artifacts
                || s.insertion_blockers(run_id)? != insertion
            {
                return Err(conflict("Research evidence changed during verification."));
            }
            Ok(())
        })?;
        let mut result = json!({"purpose":"research","criteria":run["taskSnapshot"]["criteria"],"artifacts":artifacts,"assessment":{"ready":blockers.is_empty(),"blockers":blockers,"covered":covered,"revision":run["taskRevision"],"completed":run["status"]=="completed" && run["researchOutcome"]["revision"]==run["taskRevision"]}});
        if let Some(source) = run.get("researchSource") {
            result["source"] = source.clone();
        }
        if let Some(outcome) = run.get("researchOutcome") {
            result["outcome"] = outcome.clone();
        }
        Ok(result)
    }
    fn unchanged(s: &Store, run_id: &str, result: &Value) -> Result<Value> {
        let run = s.get_run(run_id)?;
        if run["taskRevision"] != result["assessment"]["revision"]
            || run["researchSource"] != result["source"]
            || json!(s.research_artifacts(run_id)?) != result["artifacts"]
            || !s.insertion_blockers(run_id)?.is_empty()
        {
            return Err(conflict("Research evidence changed before completion."));
        }
        Ok(run)
    }
    pub fn request_completion(&self, attempt_id: &str, key: &str, input: &Value) -> Result<Value> {
        request(input, &[])?;
        limited(&json!({"key":key}), "key", 1, 200)?;
        let run_id = self
            .evidence
            .work
            .store(|s| Ok(text(&s.get_attempt(attempt_id)?, "runId")?.to_owned()))?;
        let result = self.read(&run_id)?;
        if result["assessment"]["ready"] != true {
            return Err(conflict(&format!(
                "Research outcome is incomplete: {}",
                array(&result["assessment"]["blockers"])
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            )));
        }
        self.evidence.work.store(|s| {
            Self::unchanged(s, &run_id, &result)?;
            let (attempt, _, assignment) = Self::authorize(s, attempt_id)?;
            if assignment["coordinator"] != true || attempt["ownership"] != "held" {
                return Err(conflict(
                    "Only the active research coordinator may request completion.",
                ));
            }
            s.request_research_completion(&run_id, attempt_id, key)
        })
    }
    pub fn complete(&self, run_id: &str) -> Result<Value> {
        let result = self.read(run_id)?;
        if result["assessment"]["ready"] != true {
            return Err(conflict(
                "Research outcome is incomplete or its artifacts are unavailable.",
            ));
        }
        self.evidence.work.store(|s| {
            let run = Self::unchanged(s, run_id, &result)?;
            let request = if run["status"] == "completed" {
                &run["researchOutcome"]
            } else {
                &run["researchCompletion"]
            };
            s.complete_research(
                run_id,
                text(request, "requestId")?,
                number(&result["assessment"], "revision", 1, 9007199254740991)?,
            )
        })
    }
}
