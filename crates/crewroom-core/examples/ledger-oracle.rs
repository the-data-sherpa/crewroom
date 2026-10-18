//! Development-only transaction oracle. Never used by the daemon or desktop.
//! The caller supplies a disposable database and one JSON operation over stdin.
use crewroom_core::{Result, learning::Learning, roles::Roles, store::Store};
use serde_json::{Value, json};
use std::io::Read;

fn execute(store: &Store, op: &Value) -> Result<Value> {
    let run = op["runId"].as_str().unwrap_or("");
    let assignment = op["assignmentId"].as_str().unwrap_or("");
    let attempt = op["attemptId"].as_str().unwrap_or("");
    let catalog = || {
        op["rolePath"].as_str().map(Roles::new).ok_or_else(|| {
            crewroom_core::Error::Invalid("An explicit disposable role path is required.".into())
        })
    };
    let tool_call = || {
        op["toolCallId"]
            .as_str()
            .ok_or_else(|| crewroom_core::Error::Invalid("toolCallId must be text.".into()))
    };
    let roles = op["roles"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    match op["method"].as_str().unwrap_or("") {
        "runtime" => store.runtime(run),
        "requirementRevisions" => Ok(json!(
            store.requirement_revisions(op["taskId"].as_str().unwrap())?
        )),
        "agentEventsOfType" => Ok(json!(
            store.agent_events_of_type(
                run,
                &op["types"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect::<Vec<_>>()
            )?
        )),
        "isOwnedRepositoryWorktree" => Ok(json!(
            store.is_owned_repository_worktree(op["path"].as_str().unwrap())?
        )),
        "configuredCheckIds" => Ok(json!(store.configured_check_ids(run)?)),
        "validateInsertionTeam" => {
            store.validate_insertion_team(run, op["team"].as_array().unwrap())?;
            Ok(Value::Null)
        }
        "deferDependencyLaunch" => store.defer_unadmitted_launch(run, op["reason"].as_str(), true),
        "reserveRepositoryWork" => store.reserve_repository_work(&op["record"]),
        "confirmRepositoryWork" => store.confirm_repository_work(
            run,
            op["gitFile"].as_str().unwrap(),
            &op["instructions"],
            &op["checkpoint"],
        ),
        "claimRepositoryWriter" => {
            store.claim_repository_writer(attempt, &op["checkpoint"], op.get("claim"))
        }
        "releaseRepositoryWriter" => store.release_repository_writer(attempt, &op["checkpoint"]),
        "checkpointRepository" => store.checkpoint_repository(run, &op["checkpoint"]),
        "preflightTargetRefresh" => store.preflight_target_refresh(run, op.get("supersedes")),
        "beginTargetRefresh" => store.begin_target_refresh(run, &op["refresh"]),
        "recordTargetRefreshWorkspace" => store.record_target_refresh_workspace(
            run,
            op["requestId"].as_str().unwrap(),
            &op["workspace"],
            op.get("conflicts"),
        ),
        "beginTargetConflictContinuation" => store.begin_target_conflict_continuation(
            run,
            op["requestId"].as_str().unwrap(),
            &op["identity"],
        ),
        "finishTargetConflictContinuation" => store.finish_target_conflict_continuation(
            run,
            op["requestId"].as_str().unwrap(),
            &op["identity"],
            &op["result"],
        ),
        "beginTargetPromotion" => store.begin_target_promotion(
            run,
            op["requestId"].as_str().unwrap(),
            op["operationId"].as_str().unwrap(),
            op["toHead"].as_str().unwrap(),
            &op["process"],
        ),
        "rejectUnpromotedConflict" => store.reject_unpromoted_conflict(
            run,
            op["requestId"].as_str().unwrap(),
            op["operationId"].as_str().unwrap(),
            &op["proof"],
        ),
        "interruptUnpromotedConflict" => store.interrupt_unpromoted_conflict(
            run,
            op["requestId"].as_str().unwrap(),
            op["operationId"].as_str().unwrap(),
            &op["proof"],
        ),
        "recordTargetRebaseRecovery" => store.record_target_rebase_recovery(
            run,
            op["requestId"].as_str().unwrap(),
            op["stage"].as_str().unwrap(),
        ),
        "finishTargetRefresh" => store.finish_target_refresh(
            run,
            op["requestId"].as_str().unwrap(),
            &op["outcome"],
            op.get("checkpoint"),
        ),
        "createTask" => store.create_task(&op["input"]),
        "assignRepository" => store.assign_repository(op["taskId"].as_str().unwrap(), &op["input"]),
        "addNote" => store.add_note(op["taskId"].as_str().unwrap(), &op["input"]),
        "refine" => store.refine(op["taskId"].as_str().unwrap(), &op["input"]),
        "setDependencies" => store.set_dependencies(op["taskId"].as_str().unwrap(), &op["input"]),
        "queueRefinement" => store.queue_refinement(op["taskId"].as_str().unwrap(), &op["input"]),
        "queueResearch" => store.queue_research(op["taskId"].as_str().unwrap(), &op["input"]),
        "createRepository" => store.create_repository(&op["input"]),
        "repositoryReplay" => Ok(json!(store.repository_replay(&op["input"])?)),
        "saveRepository" => {
            store.save_repository(&crewroom_core::repositories::validate(&op["input"])?)
        }
        "snapshot" => store.snapshot(),
        "repositoryWork" => Ok(json!(store.repository_work(run)?)),
        "continuationReplay" => Ok(json!(store.continuation_replay(run, &op["input"])?)),
        "continuationPublication" => Ok(json!(store.continuation_publication(run)?)),
        "publicationSuperseded" => Ok(json!(
            store.publication_superseded(run, op["publicationId"].as_str().unwrap())?
        )),
        "cancelledSource" => store.cancelled_source(run, op["successorId"].as_str()),
        "continuationView" => store.continuation_view(run),
        "queueCancelledContinuation" => {
            store.queue_cancelled_continuation(run, &op["input"], roles, &op["expectedWork"])
        }
        "transferCancelledWork" => store.transfer_cancelled_work(
            run,
            &op["expectedWork"],
            &op["repository"],
            &op["checkpoint"],
            op["continuationTargetRef"].as_str(),
        ),
        "repositoryUpdateReplay" => Ok(json!(
            store.repository_update_replay(op["id"].as_str().unwrap(), &op["input"])?
        )),
        "updateRepository" => {
            let id = op["id"].as_str().unwrap();
            if let Some(replay) = store.repository_update_replay(id, &op["input"])? {
                return Ok(replay);
            }
            let validated = crewroom_core::repositories::validate(&op["validated"])?;
            store.update_repository(id, &validated, &op["input"])
        }
        "retireRepository" => store.retire_repository(op["id"].as_str().unwrap(), &op["input"]),
        "codingRecord" => Ok(json!(store.coding_record(run)?)),
        "updateCodingRecord" => store.update_coding_record(run, |current| {
            if op["throw"] == true {
                return Err(crewroom_core::Error::Conflict(
                    "Fixture update failed.".into(),
                ));
            }
            if op["merge"] == true {
                let mut current = current.unwrap_or_else(|| json!({"runId":run}));
                current
                    .as_object_mut()
                    .unwrap()
                    .extend(op["record"].as_object().unwrap().clone());
                Ok(current)
            } else {
                Ok(op["record"].clone())
            }
        }),
        "publications" => Ok(json!(store.publications()?)),
        "publication" => Ok(json!(store.publication(run)?)),
        "savePublication" => store.save_publication(&op["record"]),
        "savePublicationFeedback" => store.save_publication_feedback(
            run,
            op["prId"].as_u64().unwrap(),
            &op["read"],
            op["observedAt"].as_str().unwrap(),
        ),
        "jiraUpdates" => Ok(json!(store.jira_updates(op["runId"].as_str())?)),
        "saveJiraUpdate" => store.save_jira_update(&op["update"]),
        "jiraState" => Ok(json!(store.jira_state()?)),
        "commitJiraSync" => store.commit_jira_sync(
            &op["source"],
            &op["input"],
            op.get("retained").unwrap_or(&json!([])),
        ),
        "failJiraSync" => {
            store.fail_jira_sync(&op["source"], op["error"].as_str().unwrap())?;
            Ok(Value::Null)
        }
        "importJira" => store.import_jira(&op["input"]),
        "getTask" => store.get_task(op["taskId"].as_str().unwrap()),
        "adoptRequirements" => store.adopt_requirements(run, &op["input"]),
        "closeRefinement" => store.close_refinement(run),
        "refinementProposals" => Ok(json!(store.refinement_proposals(op["runId"].as_str())?)),
        "proposeRefinement" => store.propose_refinement(attempt, tool_call()?, &op["input"]),
        "decideRefinement" => store.decide_refinement(op["id"].as_str().unwrap(), &op["input"]),
        "acceptRequirements" => store.accept_requirements(
            op["taskId"].as_str().unwrap(),
            &op["input"],
            &op["provenance"],
        ),
        "researchArtifacts" => Ok(json!(store.research_artifacts(run)?)),
        "saveResearchArtifact" => store.save_research_artifact(&op["artifact"]),
        "requestResearchCompletion" => {
            store.request_research_completion(run, attempt, tool_call()?)
        }
        "completeResearch" => store.complete_research(
            run,
            op["requestId"].as_str().unwrap(),
            op["revision"].as_u64().unwrap(),
        ),
        "learningProposals" => Ok(json!(store.learning_proposals()?)),
        "getLearningProposal" => store.get_learning_proposal(op["id"].as_str().unwrap()),
        "proposeLearning" => store.propose_learning(&op["input"]),
        "decideLearning" => store.decide_learning(op["id"].as_str().unwrap(), &op["input"]),
        "completeLearningApproval" => {
            store.complete_learning_approval(op["id"].as_str().unwrap(), &op["outcome"])
        }
        "learningPropose" => {
            Learning::new(store, &catalog()?).propose(attempt, tool_call()?, &op["input"])
        }
        "learningDecide" => {
            Learning::new(store, &catalog()?).decide(op["id"].as_str().unwrap(), &op["input"])
        }
        "learningReconcile" => {
            Learning::new(store, &catalog()?).reconcile()?;
            Ok(Value::Null)
        }
        "rolesRead" => Ok(json!(catalog()?.read()?)),
        "rolesCreate" => Ok(json!(catalog()?.create(&op["input"])?)),
        "rolesUpdate" => Ok(json!(
            catalog()?.update(op["id"].as_str().unwrap(), &op["input"])?
        )),
        "teamProposals" => Ok(json!(store.team_proposals(run)?)),
        "insertionReplay" => Ok(json!(store.insertion_replay(run, &op["input"])?)),
        "proposeOperatorSpecialist" => {
            store.propose_operator_specialist(run, &op["input"], &op["role"], &op["model"])
        }
        "insertionBlockers" => Ok(json!(store.insertion_blockers(run)?)),
        "proposeTeam" => store.propose_team(
            attempt,
            tool_call()?,
            op["specialists"].as_array().unwrap(),
            op["retire"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        ),
        "rejectUnavailableTeamProposal" => Ok(json!(
            store.reject_unavailable_team_proposal(op["id"].as_str().unwrap(), roles)?
        )),
        "applyTeamProposal" => store.apply_team_proposal(op["id"].as_str().unwrap(), roles),
        "beginTeamTransition" => store.begin_team_transition(op["id"].as_str().unwrap()),
        "finishTeamTransition" => store.finish_team_transition(run),
        "beginCodingTransition" => store.begin_coding_transition(run, op["id"].as_str().unwrap()),
        "finishCodingTransition" => store.finish_coding_transition(run),
        "steer" => store.steer(run, &op["input"]),
        "steering" => Ok(json!(store.steering(run)?)),
        "prepareMessage" => store.prepare_message(attempt, tool_call()?, &op["input"]),
        "bindMailbox" => {
            store.bind_mailbox(assignment, op["name"].as_str().unwrap())?;
            Ok(Value::Null)
        }
        "getPeerMessage" => store.get_peer_message(op["id"].as_str().unwrap()),
        "recordMessageStored" => store.record_message_stored(
            op["id"].as_str().unwrap(),
            op["transportId"].as_u64().unwrap_or(0),
        ),
        "acceptMessage" => store.accept_message(op["id"].as_str().unwrap()),
        "markMessageUncertain" => {
            store.mark_message_uncertain(op["id"].as_str().unwrap())?;
            Ok(Value::Null)
        }
        "recordDispatch" => {
            store.record_dispatch(
                op["id"].as_str().unwrap(),
                op["received"].as_bool().unwrap_or(false),
            )?;
            Ok(Value::Null)
        }
        "deliveryNotices" => Ok(json!(store.delivery_notices(run)?)),
        "noticeDelivery" => store.notice_delivery(run, &op["payload"]),
        "dispatchDeliveryNotice" => store.dispatch_delivery_notice(
            attempt,
            op["id"].as_str().unwrap(),
            op["received"].as_bool().unwrap_or(false),
        ),
        "restoreUndispatchedInput" => {
            store.restore_undispatched_input(attempt, op["inputId"].as_str().unwrap())?;
            Ok(Value::Null)
        }
        "dispatchSteering" => {
            let ids: Vec<String> = serde_json::from_value(op["ids"].clone())?;
            store.dispatch_steering(attempt, &ids, op["inputId"].as_str().unwrap())?;
            Ok(Value::Null)
        }
        "receiveSteering" => {
            store.receive_steering(attempt, op["inputId"].as_str().unwrap())?;
            Ok(Value::Null)
        }
        "applySteering" => store.apply_steering(attempt, tool_call()?, &op["input"]),
        "queue" => store.queue_with_roles(op["taskId"].as_str().unwrap(), &op["input"], roles),
        "setQueueLaunch" => store.set_queue_launch(run, &op["input"], roles),
        "codingQueue" => store.coding_queue(),
        "codingSlot" => Ok(json!(store.coding_slot()?)),
        "getRun" => store.get_run(run),
        "reorderQueue" => store.reorder_queue(&op["input"]),
        "cancelQueued" => store.cancel_queued(run, &op["input"]),
        "dependencyMerges" => store.dependency_merges(&store.get_run(run)?),
        "eligibilityHold" => Ok(json!(store.eligibility_hold(&store.get_run(run)?)?)),
        "saveDependencyBaseHold" => Ok(json!(store.save_dependency_base_hold(
            run,
            op["merges"].as_str().unwrap(),
            op["reason"].as_str()
        )?)),
        "setLaunchHold" => Ok(json!(store.set_launch_hold(
            run,
            &op["launch"],
            op["reason"].as_str()
        )?)),
        "assertCodingSlotAvailable" => {
            store.assert_coding_slot_available(run)?;
            Ok(Value::Null)
        }
        "claimNextCodingSlot" => Ok(json!(store.claim_next_coding_slot()?)),
        "claimCodingSlot" => store.claim_coding_slot(run, op["manual"].as_bool().unwrap_or(false)),
        "releaseCodingSlot" => store.release_coding_slot(
            run,
            op["reason"].as_str().unwrap(),
            op["candidateId"].as_str(),
        ),
        "deferUnadmittedLaunch" => store.defer_unadmitted_launch(
            run,
            op["reason"].as_str(),
            op["dependency"].as_bool().unwrap_or(false),
        ),
        "canReplayInitialLaunch" => Ok(json!(store.can_replay_initial_launch(
            run,
            op["decisionId"].as_str().unwrap(),
            op["controlRevision"].as_u64().unwrap()
        )?)),
        "normalizeLaunchExecution" => store.normalize_launch_execution(run),
        "restartScheduledLaunch" => store.restart_scheduled_launch(run),
        "reclaimCodingSlot" => {
            store.reclaim_coding_slot(run, op["reviewRegressed"].as_bool().unwrap_or(false))?;
            Ok(Value::Null)
        }
        "setDesiredState" => store.set_desired_state(run, op["desiredState"].as_str().unwrap()),
        "prepareContinuation" => {
            store.prepare_continuation(run, op["controlRevision"].as_u64().unwrap())
        }
        "requireAttention" => store.require_attention(run, op["reason"].as_str().unwrap()),
        "startExecution" => store.start_execution(run),
        "setActiveLimit" => store.set_active_limit(run, &op["input"]),
        "queueFeedbackResume" => Ok(json!(store.queue_feedback_resume(run)?)),
        "holdQueuedFeedbackResume" => store.hold_queued_feedback_resume(
            run,
            op["reason"].as_str().unwrap(),
            op["terminal"].as_bool().unwrap(),
        ),
        "queueReviewResume" => Ok(json!(store.queue_review_resume(run)?)),
        "setResumeRequestState" => {
            store.set_resume_request_state(run, op["state"].as_str().unwrap())
        }
        "clearResumeRequest" => {
            store.clear_resume_request(run, op["disposition"].as_str().unwrap_or("held"))
        }
        "markFeedbackHandoff" => Ok(json!(
            store.mark_feedback_handoff(run, op["id"].as_str().unwrap())?
        )),
        "finishFeedbackContinuation" => {
            store.finish_feedback_continuation(run, op["id"].as_str().unwrap(), &op["outcome"])
        }
        "admitAssignment" => store.admit_assignment(run, &op["input"]),
        "admitInitialTeam" => store.admit_initial_team(run, op["team"].as_array().unwrap()),
        "claimAttempt" => store.claim_attempt(attempt),
        "recordAttemptIdentity" => store.record_attempt_identity(attempt, &op["identity"]),
        "recordOwnership" => {
            store.record_ownership(
                attempt,
                op["stopped"].as_bool().unwrap(),
                op["reason"].as_str(),
            )?;
            Ok(Value::Null)
        }
        "replaceAttempt" => store.replace_attempt(assignment, &op["input"]),
        "grantRepairCycle" => store.grant_repair_cycle(run, assignment, &op["input"]),
        "recordAgentEvent" => {
            store.record_agent_event(attempt, op["sequence"].as_u64().unwrap(), &op["event"])?;
            Ok(Value::Null)
        }
        "beginPause" => store.begin_pause(run, op["reason"].as_str()),
        "finishPause" => store.finish_pause(run),
        "accountTime" => store.account_time(run),
        "executingRuns" => store.executing_runs(),
        "unstoppedCodingRun" => Ok(store.unstopped_coding_run(run)?.unwrap_or(Value::Null)),
        "getAssignment" => store.get_assignment(assignment),
        "getAttempt" => store.get_attempt(attempt),
        "agentEvents" => Ok(json!(store.agent_events(run)?)),
        "commandEvents" => Ok(json!(store.command_events(run)?)),
        "agentEventPage" => store.agent_event_page(
            run,
            op["after"].as_u64().unwrap_or(0),
            op["limit"].as_u64().unwrap_or(100) as usize,
        ),
        "attemptForPane" => Ok(json!(store.attempt_for_pane(op["pane"].as_str().unwrap())?)),
        "hasAgentEvent" => Ok(json!(
            store.has_agent_event(attempt, op["type"].as_str().unwrap())?
        )),
        "observerCursor" => Ok(json!(store.observer_cursor(assignment)?)),
        "recordObserverPage" => {
            store.record_observer_page(
                assignment,
                op["events"].as_array().unwrap(),
                op["cursor"].as_i64().unwrap(),
            )?;
            Ok(Value::Null)
        }
        _ => Err(crewroom_core::Error::Invalid(
            "Unknown oracle operation.".into(),
        )),
    }
}
fn main() -> Result<()> {
    let path = std::env::args().nth(1).expect("disposable database path");
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let op: Value = serde_json::from_str(&input)?;
    let at = op["now"].as_i64().expect("explicit fixture clock");
    let store = Store::open(std::path::Path::new(&path))?.with_clock(move || at);
    let response = match execute(&store, &op) {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(crewroom_core::Error::RepositoryConfigurationChanged(message)) => {
            json!({"ok":false,"error":message,"repositoryConfigurationChanged":true})
        }
        Err(crewroom_core::Error::FeedbackResume { message, terminal }) => {
            json!({"ok":false,"error":message,"terminal":terminal})
        }
        Err(error) => json!({"ok":false,"error":error.to_string(),"kind":match error {
            crewroom_core::Error::Invalid(_) => "invalid",
            crewroom_core::Error::NotFound(_) => "not_found",
            crewroom_core::Error::Conflict(_) => "conflict",
            _ => "internal",
        }}),
    };
    if let Some(path) = std::env::var_os("CREWROOM_ORACLE_COVERAGE") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(
            file,
            "{}",
            json!({"method":op["method"],"ok":response["ok"]})
        )?;
    }
    println!("{response}");
    Ok(())
}
