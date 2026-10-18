use super::*;
use crate::repository::request;
use crewroom_core::{learning::Learning, serialized::stringify};

pub(super) struct Caller {
    pub(super) run: Value,
    pub(super) assignment: Value,
    pub(super) attempt: String,
    pub(super) key: String,
    pub(super) args: Value,
}
fn envelope(input: &Value) -> Result<(String, String, Value)> {
    request(input, &["attemptId", "toolCallId", "arguments"])?;
    let attempt = text(input, "attemptId")?.to_owned();
    let key = text(input, "toolCallId")?.to_owned();
    if !input["arguments"].is_object() {
        return Err(conflict("Host tool arguments must be an object."));
    }
    Ok((attempt, key, input["arguments"].clone()))
}
impl<S: StoreAccess> Workflow<S> {
    fn authorize(&self, authorization: &str, input: &Value, route: &str) -> Result<Caller> {
        self.check_open()?;
        let (attempt, key, args) = envelope(input)?;
        if !self.0.hosts.authenticates(&attempt, authorization) {
            return Err(conflict("Agent host is not authorized."));
        }
        self.store(|s| {
            let current=s.get_attempt(&attempt)?;let run=s.get_run(text(&current,"runId")?)?;let runtime=s.runtime(text(&run,"id")?)?;
            let assignment=active(&runtime).into_iter().find(|a|a["currentAttemptId"]==attempt).ok_or_else(||conflict("Attempt is no longer the current assignment."))?.clone();
            if run["desiredState"]!="running" || present(&run,"sourceConflict") || current["ownership"]!="held" || (run["status"]!="running" && !(route=="agent-message" && run["status"]=="awaiting_runner")) {return Err(conflict("Agent host no longer owns active work."));}
            let purpose=crewroom_core::run_purpose(&run)?;let coordinator=assignment["coordinator"]==true;
            let config=json!({"purpose":purpose,"coordinator":coordinator,"choreography":run["executionPolicy"]==1,"coding":s.repository_work(text(&run,"id")?)?.is_some(),"research":purpose=="research","researchRepository":present(&run,"researchSource"),"refinement":matches!(purpose,"explore"|"grill"),"intake":purpose=="grill"&&present(&run,"intakeId"),"publication":purpose=="coding"&&self.0.services.publication.is_some(),"jira":!matches!(purpose,"explore"|"grill")&&self.0.services.jira.is_some(),"reporter":assignment["role"]["id"]=="delivery-reporter"&&!coordinator,"researchCoordinatorReporter":purpose=="research"&&coordinator&&!active(&runtime).iter().any(|a|a["role"]["id"]=="delivery-reporter"),"commandEngine":self.0.config.engine});
            let permitted=crate::host_protocol::permits_backend_route(&config,route);
            if !permitted{return Err(conflict("Tool is not registered for this assignment."));}
            Ok(Caller{run,assignment,attempt,key,args})
        })
    }
    /// The daemon passes the authenticated host bearer, never the operator token.
    /// Route authorization is repeated after the run's FIFO queue is reached.
    pub async fn tool(&self, route: &str, authorization: &str, input: Value) -> Result<Value> {
        let caller = self.authorize(authorization, &input, route)?;
        let run = text(&caller.run, "id")?.to_owned();
        let read = matches!(
            route,
            "coding/choreography"
                | "coding/status"
                | "coding/read"
                | "coding/search"
                | "coding/output"
                | "intake/read"
                | "intake/search"
                | "research/read"
                | "research/search"
                | "publication/status"
                | "jira/updates"
                | "agent-roles"
                | "playbook/status"
        );
        if read {
            return self.execute_tool(route, caller).await;
        }
        let this = self.clone();
        let route = route.to_owned();
        let authorization = authorization.to_owned();
        let id = run.clone();
        wait(self.0.tasks.enqueue(&run, None, move || async move {
            let result = async {
                let caller = this.authorize(&authorization, &input, &route)?;
                this.execute_tool(&route, caller).await
            }
            .await;
            this.notify(&id);
            result.map_err(Arc::new)
        }))
        .await
    }
    async fn execute_tool(&self, route: &str, c: Caller) -> Result<Value> {
        let run = text(&c.run, "id")?;
        let repository = &self.0.services.repository;
        let evidence = &self.0.services.evidence;
        let empty = || request(&c.args, &[]);
        if route.starts_with("evaluation/") {
            return self.evaluation_tool(route, &c).await;
        }
        match route {
            "coding/choreography"
            | "coding/choreography-propose"
            | "coding/package"
            | "coding/pair-exchange" => self.choreography_tool(route, &c).await,
            "coding/checkpoint" => {
                repository
                    .request_checkpoint(&c.attempt, &c.key, &c.args)
                    .await
            }
            "agent-message" => {
                request(&c.args, &["to", "body"])?;
                let recipient = text(&c.args, "to")?;
                text(&c.args, "body")?;
                if c.run["purpose"] == "evaluation" {
                    self.verify_evaluation_recipient(
                        run,
                        c.assignment["coordinator"] == true,
                        recipient,
                    )?;
                }
                let mut args = c.args.clone();
                if let Some(reply) = self
                    .store(|s| s.get_attempt(&c.attempt))?
                    .get("activeInputMessageId")
                {
                    args["replyTo"] = reply.clone();
                }
                if self.evaluation_is_finalizing(run)? {
                    // Replay retained intent only. Sealing must not turn a
                    // pending pre-handoff message into a new external send.
                    return self.store(|store| store.prepare_message(&c.attempt, &c.key, &args));
                }
                self.messages(run)?.send(&c.attempt, &c.key, &args).await
            }
            "steering-receipt" => self.store(|s| s.apply_steering(&c.attempt, &c.key, &c.args)),
            "refinement-proposal" => {
                self.store(|s| s.propose_refinement(&c.attempt, &c.key, &c.args))
            }
            "learning-proposal" => {
                self.store(|s| Learning::new(s, &self.0.roles).propose(&c.attempt, &c.key, &c.args))
            }
            "agent-roles" => {
                empty()?;
                Ok(json!(self.0.roles.read()?.iter().filter(|r|r["enabled"]==true).map(|r|json!({"id":r["id"],"name":r["name"],"purpose":r["purpose"],"model":r["model"],"effort":r["effort"]})).collect::<Vec<_>>()))
            }
            "playbook/status" => {
                empty()?;
                Ok(
                    json!({"catalog":crewroom_core::playbooks::Playbooks::beside_roles(&self.0.roles).read()?.iter().map(|p|json!({"id":p["id"],"name":p["name"],"description":p["description"].as_str().unwrap_or("").chars().take(200).collect::<String>(),"revision":p["revision"],"stepCount":array(&p["steps"]).len()})).collect::<Vec<_>>(),"selected":c.run["playbook"],"proposal":{ "id":c.run["playbookProposal"]["id"],"status":c.run["playbookProposal"]["status"],"name":c.run["playbookProposal"]["playbook"]["name"]},"version":c.run["playbookVersion"].as_u64().unwrap_or(0),"advisoryOnly":true}),
                )
            }
            "playbook/propose" => self.store(|s| {
                s.propose_playbook(
                    &c.attempt,
                    &c.key,
                    &c.args,
                    &crewroom_core::playbooks::Playbooks::beside_roles(&self.0.roles).read()?,
                )
            }),
            "playbook/step" => self.store(|s| s.record_playbook_step(&c.attempt, &c.key, &c.args)),
            "team-proposal" => self.propose_team(&c),
            "intake/questions" => {
                self.store(|s| s.ask_intake_questions(&c.attempt, &c.key, &c.args))
            }
            "intake/plan" => self.store(|s| s.propose_intake_plan(&c.attempt, &c.key, &c.args)),
            "intake/read" | "intake/search" => {
                let source = self.store(|s| s.intake_source_for_run(run))?;
                let value = if route == "intake/read" {
                    request(&c.args, &["path"])?;
                    repository
                        .read_research(run, text(&c.args, "path")?)
                        .await?
                } else {
                    repository.search_research(run, &c.args).await?
                };
                self.store(|s| s.record_intake_inspection(&c.attempt, text(&source, "id")?))?;
                Ok(value)
            }
            "research/read" => {
                request(&c.args, &["path"])?;
                repository.read_research(run, text(&c.args, "path")?).await
            }
            "research/search" => repository.search_research(run, &c.args).await,
            "research/submit" => {
                self.0
                    .services
                    .research
                    .submit(&c.attempt, &c.key, &c.args)
                    .await
            }
            "research/complete" => self
                .0
                .services
                .research
                .request_completion(&c.attempt, &c.key, &c.args),
            "publication/status" => {
                empty()?;
                self.0
                    .services
                    .publication
                    .as_ref()
                    .ok_or_else(|| conflict("Publication is unavailable."))?
                    .view(run)
                    .await
            }
            "publication/prepare" => {
                self.0
                    .services
                    .publication
                    .as_ref()
                    .ok_or_else(|| conflict("Publication is unavailable."))?
                    .prepare(&c.attempt, &c.args)
                    .await
            }
            "jira/updates" => {
                empty()?;
                Ok(json!(
                    self.0
                        .services
                        .jira
                        .as_ref()
                        .ok_or_else(|| conflict("Jira delivery is unavailable."))?
                        .read(run)?
                ))
            }
            "jira/report" => {
                self.0
                    .services
                    .jira
                    .as_ref()
                    .ok_or_else(|| conflict("Jira delivery is unavailable."))?
                    .prepare(&c.attempt, &c.args)
                    .await
            }
            "coding/status" => {
                empty()?;
                let assessment = evidence.assess(run).await?;
                self.store(|s| {
                    let work=s.repository_work(run)?.unwrap_or(Value::Null);let record=s.coding_record(run)?;
                    let target=array(&work["refreshes"]).iter().rev().find(|r|r["state"]=="conflicted"&&!present(r,"supersededBy"));
                    let mut value=json!({"record":context::coding_context(record.as_ref())?,"assessment":assessment,"checksAvailable":self.0.services.checks.is_some()});
                    if let Some(writer)=work.get("writer"){value["writer"]=writer.clone();}
                    if let Some(target)=target{value["targetConflict"]=json!({"requestId":target["requestId"],"state":target["resolutionWorkspace"]["state"].as_str().unwrap_or("operator_required"),"conflicts":target.get("conflicts").cloned().unwrap_or(json!([]))});}
                    Ok(value)
                })
            }
            "coding/output" => {
                request(&c.args, &["checkEvidenceId"])?;
                let id = text(&c.args, "checkEvidenceId")?;
                if !crewroom_core::store::valid_uuid(id) {
                    return Err(conflict("Invalid check evidence identity."));
                }
                evidence.check_output(run, id)
            }
            "coding/read" => {
                request(&c.args, &["path"])?;
                if let Some(path) = c.args.get("path") {
                    let path = path
                        .as_str()
                        .ok_or_else(|| conflict("Invalid repository path."))?;
                    if let Some(revision) = self.review_revision(&c)? {
                        repository.read_revision(run, &revision, path).await
                    } else {
                        repository.read_file(run, path).await
                    }
                } else {
                    repository.list_files(run).await
                }
            }
            "coding/search" => {
                repository
                    .search(run, &c.args, self.review_revision(&c)?.as_ref())
                    .await
            }
            "coding/shell" => repository.prepare_shell(&c.attempt, &c.args).await,
            "coding/claim" => {
                repository
                    .claim_implementation(&c.attempt, &c.key, &c.args)
                    .await
            }
            "coding/edit" => repository.edit(&c.attempt, &c.key, &c.args).await,
            "coding/patch" => repository.patch(&c.attempt, &c.key, &c.args).await,
            "coding/complete" => {
                repository
                    .request_completion(&c.attempt, &c.key, &c.args)
                    .await
            }
            "coding/conflict-continue" => {
                let result = repository
                    .continue_target_conflict(&c.attempt, &c.key, &c.args)
                    .await?;
                if result["state"] == "operator_required" {
                    self.store(|s| {
                        s.set_desired_state(run, "paused")?;
                        Ok(())
                    })?;
                    // Schedule cleanup after the tool reply; never await this
                    // same lane while the host is waiting for its backend tool.
                    let this = self.clone();
                    let id = run.to_owned();
                    self.0.tasks.enqueue(run,None,move||async move{
                        this.stop_run(&id,None).await.map_err(Arc::new)?;
                        this.store(|s|s.require_attention(&id,"Target conflict resolution needs an operator decision; the original branch and PR remain unchanged.")).map_err(Arc::new)
                    })?;
                }
                Ok(result)
            }
            "coding/review-assign" => evidence.assign_review(&c.attempt, &c.key, &c.args),
            "coding/review" => evidence.review(&c.attempt, &c.args),
            "coding/check-prepare" => {
                self.0
                    .services
                    .checks
                    .as_ref()
                    .ok_or_else(|| conflict("Contained repository checks are unavailable."))?
                    .prepare(&c.attempt, &c.key, &c.args)
                    .await
            }
            "coding/check-finish" => {
                empty()?;
                self.0
                    .services
                    .checks
                    .as_ref()
                    .ok_or_else(|| conflict("Contained repository checks are unavailable."))?
                    .finish(&c.attempt, &c.key)
                    .await
            }
            _ => Err(conflict("Unknown agent tool route.")),
        }
    }
    fn review_revision(&self, c: &Caller) -> Result<Option<Value>> {
        self.store(|s| {
            let run = text(&c.run, "id")?;
            let record = s.coding_record(run)?.unwrap_or(Value::Null);
            let review = array(&record["reviewAssignments"])
                .iter()
                .rev()
                .find(|r| r["assignmentId"] == c.assignment["id"]);
            let candidate = review.and_then(|r| {
                array(&record["candidates"])
                    .iter()
                    .find(|candidate| candidate["id"] == r["candidateId"])
            });
            if s.repository_work(run)?
                .is_some_and(|w| w["writer"]["attemptId"] == c.attempt)
            {
                Ok(None)
            } else {
                Ok(candidate.map(|c| c["revision"].clone()))
            }
        })
    }
    fn propose_team(&self, c: &Caller) -> Result<Value> {
        request(&c.args, &["specialists", "retire"])?;
        let additions = c.args["specialists"]
            .as_array()
            .filter(|a| a.len() <= 3)
            .ok_or_else(|| conflict("At most three specialists may be proposed."))?;
        let retire = c.args.get("retire").cloned().unwrap_or(json!([]));
        let retire = retire
            .as_array()
            .filter(|a| a.len() <= 3)
            .ok_or_else(|| conflict("At most three specialists may retire."))?;
        let mut name = "Coordinator".to_owned();
        while additions.iter().any(|a| a["name"] == name) {
            name.push('X');
        }
        let mut team = vec![
            json!({"name":name,"coordinator":true,"roleId":"coordinator","input":"Proposal schema validation"}),
        ];
        for item in additions {
            request(item, &["name", "roleId", "input", "criteria", "dependsOn"])?;
            let mut item = item.clone();
            item["coordinator"] = json!(false);
            team.push(item);
        }
        let normalized = crewroom_core::parse_team(&json!({"team":team}))?;
        let members = &array(&normalized["team"])[1..];
        let run = text(&c.run, "id")?;
        let runtime = self.store(|s| s.runtime(run))?;
        if let Some(previous) = self
            .store(|s| s.team_proposals(run))?
            .into_iter()
            .find(|p| p["attemptId"] == c.attempt && p["toolCallId"] == c.key)
        {
            let original:Vec<_>=array(&previous["specialists"]).iter().map(|s|json!({"name":s["name"],"roleId":s["role"]["id"],"input":s["input"],"criteria":array(&s["brief"]["criteria"]).iter().map(|c|json!({"index":c["index"],"obligation":c["obligation"],"checkIds":c["checkIds"]})).collect::<Vec<_>>(),"dependsOn":s["brief"].get("dependsOn").cloned().unwrap_or(json!([]))})).collect();
            let requested: Vec<_> = members
                .iter()
                .map(|m| {
                    let mut m = m.clone();
                    remove(&mut m, &["coordinator"]);
                    deduplicate(&mut m["dependsOn"]);
                    for c in m["criteria"].as_array_mut().unwrap() {
                        deduplicate(&mut c["checkIds"]);
                    }
                    m
                })
                .collect();
            let retirements:Vec<_>=array(&previous["retirements"]).iter().map(|r|json!({"name":array(&runtime["assignments"]).iter().find(|a|a["id"]==r["assignmentId"]).map(|a|a["name"].clone()).unwrap_or(Value::Null),"reason":r["reason"]})).collect();
            if json!(original) != json!(requested) || json!(retirements) != json!(retire) {
                return Err(conflict("Proposal identity has different content."));
            }
            return Ok(previous);
        }
        let checks = self.store(|s| s.configured_check_ids(run))?;
        let briefs = crewroom_core::snapshot_briefs(
            members,
            &c.run["taskSnapshot"]["criteria"],
            &checks,
            array(&runtime["assignments"]),
        )?;
        let roles = self.0.roles.read()?;
        let specialists=members.iter().map(|m|{
            let role=roles.iter().find(|r|r["id"]==m["roleId"]&&r["enabled"]==true).ok_or_else(||conflict("Selected specialist role is unavailable."))?;
            preparation::validate_research_role(c.run["purpose"].as_str().unwrap_or("coding"),role,m)?;
            Ok(json!({"name":m["name"],"input":m["input"],"role":role,"model":self.0.config.assignment_model(role)?,"brief":briefs[text(m,"name")?]}))
        }).collect::<Result<Vec<_>>>()?;
        self.store(|s| s.propose_team(&c.attempt, &c.key, &specialists, retire))
    }
    pub fn insert_specialist(&self, run: &str, input: &Value) -> Result<Value> {
        self.check_open()?;
        let input = crewroom_core::parse_insertion_request(input)?;
        if let Some(previous) = self.store(|s| s.insertion_replay(run, &input))? {
            return Ok(previous);
        }
        let roles = self.0.roles.read()?;
        let role = roles
            .iter()
            .find(|r| r["id"] == input["roleId"] && r["enabled"] == true)
            .ok_or_else(|| conflict("Selected specialist role is disabled or unavailable."))?;
        let model = self.0.config.assignment_model(role)?;
        crate::model_catalog::policy::verify_preview(&model, &input["modelPreview"])?;
        let result = self.store(|s| s.propose_operator_specialist(run, &input, role, &model))?;
        self.notify(run);
        Ok(result)
    }
}
fn deduplicate(v: &mut Value) {
    if let Some(values) = v.as_array_mut() {
        let mut seen = HashSet::new();
        values.retain(|v| seen.insert(stringify(v)));
    }
}
