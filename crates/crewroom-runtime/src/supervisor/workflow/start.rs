use super::*;
use configuration::HostAssignment;
use context::{host_input, session_needs_replacement};

impl<S: StoreAccess> Workflow<S> {
    pub fn preflight_start(&self, run: &str, input: &Value, roles: &[Value]) -> Result<Vec<Value>> {
        self.check_open()?;
        let runtime = self.store(|s| s.runtime(run))?;
        if active(&runtime).iter().any(|a| {
            self.0
                .hosts
                .observes(a["currentAttemptId"].as_str().unwrap_or(""))
        }) {
            return Err(conflict(
                "This run already has agents. Resume or reconcile its existing attempts.",
            ));
        }
        if self.store(|s| s.get_run(run))?["purpose"] == "evaluation" {
            return self.evaluation_assignments(run);
        }
        self.store(|s| {
            let current = s.capture_execution_policy(run)?;
            if current["executionPolicy"] == 1 && array(&runtime["assignments"]).is_empty() && array(&input["team"]).iter().any(|a| a["coordinator"] != true) {
                return Err(conflict("Start the coordinator first; it binds the accepted packages before proposing the pair."));
            }
            if let Some(reason) = s.implementation_plan_hold(&current)? {
                return Err(conflict(&reason));
            }
            if current["intakeId"].is_string() && current["purpose"] == "grill" {
                s.intake_source_for_run(run)?;
            }
            s.validate_insertion_team(run, array(&crewroom_core::parse_team(input)?["team"]))?;
            preparation::prepare_assignments(
                &self.0.config,
                &current,
                s.repository_work(run)?.as_ref(),
                &s.configured_check_ids(run)?,
                input,
                roles,
            )
        })
    }
    pub fn start_refinement(&self, run: &str) -> Result<Task> {
        let roles = self.0.roles.read()?;
        let input = self
            .0
            .config
            .refinement_team(&self.store(|s| s.get_run(run))?, &roles)?;
        self.start(run, input, roles)
    }
    pub fn start(&self, run: &str, input: Value, roles: Vec<Value>) -> Result<Task> {
        self.preflight_start(run, &input, &roles)?;
        let original = self.store(|s| s.get_run(run))?;
        let reservation = self.reserve(run)?;
        let this = self.clone();
        let id = run.to_owned();
        self.0.tasks.enqueue(run, None, move || async move {
            let _reservation = reservation;
            let result = async {
                if this.store(|s| s.get_run(&id))?["controlRevision"] != original["controlRevision"]
                {
                    return Err(conflict(
                        "A newer control request stopped this queued launch.",
                    ));
                }
                this.start_run(&id, &input, &roles).await
            }
            .await;
            result.map_err(Arc::new)
        })
    }
    pub fn start_scheduled(
        &self,
        run: &str,
        decision: &str,
        control: u64,
        input: Value,
        roles: Vec<Value>,
    ) -> Result<Task> {
        let reservation = self.reserve(run)?;
        let this = self.clone();
        let id = run.to_owned();
        let decision = decision.to_owned();
        self.0.tasks.enqueue(run, None, move || async move {
            let _reservation = reservation;
            let result = async {
                this.check_open()?;
                if !this.store(|s| s.can_replay_initial_launch(&id, &decision, control))? {
                    return Ok(Value::Null);
                }
                this.start_run(&id, &input, &roles).await
            }
            .await;
            result.map_err(Arc::new)
        })
    }
    async fn start_run(&self, run: &str, input: &Value, roles: &[Value]) -> Result<Value> {
        let current = self.store(|s| s.get_run(run))?;
        if current["implementationPlan"].is_object()
            && array(&self.store(|s| s.runtime(run))?["assignments"]).is_empty()
        {
            let source = self.store(|s| {
                let task = s.get_task(text(&current, "taskId")?)?;
                s.prepare_plan_source(text(&task, "id")?, task["revision"].as_u64().unwrap())
            })?;
            let pinned = tokio::task::spawn_blocking(move || source.validate())
                .await
                .map_err(|_| conflict("Plan source validation interrupted."))??;
            self.store(|s| s.save_plan_source(&pinned))?;
        }
        let prepared = self.preflight_start(run, input, roles)?;
        let (admitted, control) = self.store(|s| {
            s.transaction(|s| {
                for item in &prepared {
                    if current["purpose"] != "evaluation" {
                        self.0
                            .config
                            .verify_model_preview(&item["role"], &item["model"])?;
                    }
                }
                let admitted = s.admit_initial_team(run, &prepared)?;
                let current = s.set_desired_state(run, "running")?;
                Ok((admitted, revision(&current)?))
            })
        })?;
        let result = async {
            let messages = self.messages(run)?;
            messages.register(run).await?;
            self.current(run, control)?;
            self.launch_assignments(run, array(&admitted), control)
                .await?;
            self.current(run, control)?;
            self.store(|s| s.start_execution(run))?;
            let coordinator = array(&admitted)
                .iter()
                .find(|v| v["assignment"]["coordinator"] == true)
                .ok_or_else(|| conflict("Coordinator is missing."))?;
            let attempt = &coordinator["attempt"];
            let id = text(attempt, "id")?;
            self.0
                .hosts
                .request(
                    id,
                    &host_input(
                        &format!("initial:{id}"),
                        &json!({"assignment":attempt["input"]}),
                        None,
                    )?,
                )
                .await?;
            self.0.state.lock().unwrap().errors.remove(run);
            self.start_timer();
            self.notify(run);
            self.snapshot(run)
        }
        .await;
        if let Err(error) = &result {
            self.failure(run, error);
            let _ = self.store(|s| {
                let current = s.get_run(run)?;
                if current["desiredState"] == "running" && revision(&current)? == control {
                    s.set_desired_state(run, "paused")?;
                }
                Ok(())
            });
            let _ = self.stop_run(run, Some("recovery")).await;
        }
        result
    }
    async fn lease_port(&self, attempt: &str) -> Result<u16> {
        let Some(pool) = self.0.config.ports.as_ref() else {
            return Ok(0);
        };
        for port in pool {
            {
                let mut state = self.0.state.lock().unwrap();
                if state.closing {
                    return Err(conflict("Workflow supervision is stopping."));
                }
                if state.ports.iter().any(|(id, p)| id != attempt && p == port) {
                    continue;
                }
                state.ports.insert(attempt.into(), *port);
            }
            match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, *port)).await {
                Ok(probe) => {
                    drop(probe);
                    return Ok(*port);
                }
                Err(_) => {
                    self.0.state.lock().unwrap().ports.remove(attempt);
                }
            }
        }
        Err(conflict(
            "No configured agent host port is free. Stop other agents or configure more host ports.",
        ))
    }
    pub(super) async fn launch_assignments(
        &self,
        run: &str,
        admitted: &[Value],
        control: u64,
    ) -> Result<()> {
        for item in admitted {
            self.verify_evaluation_launch(run)?;
            let current = self.current(run, control)?;
            let attempt = self.store(|s| s.get_attempt(text(&item["attempt"], "id")?))?;
            let assignment = self.store(|s| s.get_assignment(text(&attempt, "assignmentId")?))?;
            let previous = attempt["previousAttemptId"]
                .as_str()
                .map(|id| self.store(|s| s.get_attempt(id)))
                .transpose()?;
            let (runtime, work, events) = self.store(|s| {
                Ok((
                    s.runtime(run)?,
                    s.repository_work(run)?,
                    s.agent_events(run)?,
                ))
            })?;
            let id = text(&attempt, "id")?;
            let resume = if let Some(p) = &previous {
                !session_needs_replacement(&events, text(p, "id")?)
                    // An intake decision interrupts the current tool turn. OMP may retain
                    // opaque reasoning from that aborted turn without a verifiable budget.
                    // Reconstruct from durable questions/answers instead of replaying it.
                    && !(current["intakeId"].is_string()
                        && p["runnerKind"].as_str().unwrap_or("omp") == "omp"
                        && events.iter().rev().find(|e| e["attemptId"] == p["id"] && e["event"]["type"] == "prompt_result")
                            .is_none_or(|e| e["event"]["status"] != "completed")) && {
                    let path = p["sessionDirectory"]
                        .as_str()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| {
                            self.0
                                .config
                                .root
                                .join(p["id"].as_str().unwrap())
                                .join("sessions")
                        });
                    path.starts_with(&self.0.config.root)
                        && path.is_dir()
                        && crate::canonical(&path).is_ok()
                }
            } else {
                false
            };
            // FOR-69: a changed executable is re-probed; a pass is recorded on
            // the attempt and launches, a failure holds the launch.
            let attempt = self.hold_changed_executable(&attempt, &assignment).await?;
            let port = self.lease_port(id).await?;
            self.current(run, control)?;
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let config = self.0.config.host_configuration(HostAssignment {
                run: &current,
                assignment: &assignment,
                attempt: &attempt,
                team: array(&runtime["assignments"]),
                work: work.as_ref(),
                previous: previous.as_ref(),
                resume_session: resume,
                port,
                provider_token: &token,
                environment: &self.0.environment,
                publication: self.0.services.publication.is_some(),
                jira: self.0.services.jira.is_some(),
                github: self.0.services.github.is_some(),
            })?;
            self.verify_evaluation_tools(run, &config)?;
            private_directory(Path::new(text(&config, "runnerCwd")?))?;
            self.current(run, control)?;
            wait(self.0.hosts.launch(id, control, config)).await?;
        }
        Ok(())
    }
}
