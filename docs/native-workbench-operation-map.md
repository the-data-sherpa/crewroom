# Native Workbench operation inventory

Inventoried from `server/workbench.ts` at `c5ebbb2` on 2026-10-02. This is a scope map, not acceptance evidence. Each row names the iteration responsible for completing its methods. The [execution ledger](native-migration-tasks.md) and [progress record](testing/mvp-progress.md) determine which boundaries have passed. N2.2, N2.3, N2.4a–c and N2.5a–c/N2.6 pass their bounded transaction checks; N2.7a–c closure passes; schema-26 durable parity is accepted within the recorded boundaries. The [executed map](testing/rust-native-operation-map-2026-10-02.json) records all native entry points and successful dispatch counts.

The inventory covers every public Workbench method, including read projections and control methods. Private helpers belong to the caller transaction and are not separate capabilities. Equivalent Rust names and module boundaries may differ. A recorded identity, ownership proof or provider observation in a store fixture never establishes an actual process, Git operation or delivery; N3–N5 must supply and validate those facts.

| Completion boundary | Workbench methods |
| --- | --- |
| N1: SQLite lifetime | `close` |
| N1: intake and persistence | `reorderQueue`, `cancelQueued`, `getTask`, `requirementRevisions`, `assignRepository`, `addNote`, `steer`, `retireRepository`, `repositoryReplay`, `isOwnedRepositoryWorktree`, `saveRepository`, `createTask`, `importJira`, `refine` |
| N1: stored run read | `getRun` |
| N2.1: requirements, dependencies and alternate plans | `setDependencies`, `closeRefinement`, `queueRefinement`, `queueResearch`, `adoptRequirements` |
| N2.2: assignment and attempt ledger | `runtime`, `admitAssignment`, `admitInitialTeam`, `validateInsertionTeam`, `observerCursor`, `getAttempt`, `claimAttempt`, `executingRuns`, `unstoppedCodingRun`, `replaceAttempt`, `grantRepairCycle`, `recordAttemptIdentity`, `configuredCheckIds`, `accountTime`, `beginPause`, `recordOwnership`, `finishPause`, `agentEvents`, `agentEventsOfType`, `attemptForPane`, `commandEvents`, `agentEventPage`, `hasAgentEvent`, `recordAgentEvent`, `recordObserverPage` |
| N2.3: launch, slots, lifecycle and Resume (bare queue is N1) | `codingQueue`, `codingSlot`, `eligibilityHold`, `dependencyMerges`, `saveDependencyBaseHold`, `setLaunchHold`, `claimNextCodingSlot`, `assertCodingSlotAvailable`, `claimCodingSlot`, `releaseCodingSlot`, `deferUnadmittedLaunch`, `deferDependencyLaunch`, `canReplayInitialLaunch`, `normalizeLaunchExecution`, `restartScheduledLaunch`, `reclaimCodingSlot`, `queueFeedbackResume`, `holdQueuedFeedbackResume`, `queueReviewResume`, `setResumeRequestState`, `clearResumeRequest`, `markFeedbackHandoff`, `finishFeedbackContinuation`, `setQueueLaunch`, `setDesiredState`, `prepareContinuation`, `requireAttention`, `startExecution`, `setActiveLimit`, `queue` |
| N2.4c: learning | `learningProposals`, `getLearningProposal`, `proposeLearning`, `decideLearning`, `completeLearningApproval` |
| N2.4a: controller notices | `deliveryNotices`, `noticeDelivery`, `dispatchDeliveryNotice` |
| N2.4b: team changes | `teamProposals`, `insertionReplay`, `proposeOperatorSpecialist`, `insertionBlockers`, `proposeTeam`, `rejectUnavailableTeamProposal`, `applyTeamProposal`, `beginCodingTransition`, `finishCodingTransition`, `beginTeamTransition`, `finishTeamTransition` |
| N2.4a: peer messages | `prepareMessage`, `bindMailbox`, `getPeerMessage`, `recordMessageStored`, `acceptMessage`, `markMessageUncertain`, `restoreUndispatchedInput`, `recordDispatch` |
| N2.4a: operator steering delivery | `steering`, `dispatchSteering`, `receiveSteering`, `applySteering` |
| N2.5c: cancelled continuation and transfer | `continuationReplay`, `continuationPublication`, `publicationSuperseded`, `cancelledSource`, `continuationView`, `queueCancelledContinuation`, `transferCancelledWork` |
| N2.5b: coding records | `codingRecord`, `updateCodingRecord` |
| N2.5c: complete projections (intake projection is N1) | `snapshot` |
| N2.5b: durable Jira sync state (transport is N5) | `jiraState`, `commitJiraSync`, `failJiraSync` |
| N2.5b: provider intents and observations | `publications`, `publication`, `savePublication`, `savePublicationFeedback`, `jiraUpdates`, `saveJiraUpdate` |
| N2.5a: refinement and research outcomes | `refinementProposals`, `proposeRefinement`, `decideRefinement`, `researchArtifacts`, `saveResearchArtifact`, `requestResearchCompletion`, `completeResearch` |
| N2.5a: refinement proposal provenance (operator refinements are N1) | `acceptRequirements` |
| N2.5c: repository updates | `repositoryUpdateReplay`, `updateRepository` |
| N2.5c: retained ownership read | `repositoryWork` |
| N2.6: repository ownership and recovery records | `reserveRepositoryWork`, `confirmRepositoryWork`, `claimRepositoryWriter`, `releaseRepositoryWriter`, `checkpointRepository`, `preflightTargetRefresh`, `beginTargetRefresh`, `recordTargetRefreshWorkspace`, `beginTargetConflictContinuation`, `finishTargetConflictContinuation`, `beginTargetPromotion`, `rejectUnpromotedConflict`, `interruptUnpromotedConflict`, `recordTargetRebaseRecovery`, `finishTargetRefresh` |
| N3.3: controller observation wiring (HTTP/WS notifications are N1) | `onChange` |
| N7.1: full legacy schema upgrades (fresh/schema-26 open is N1) | `constructor` |

Total: **154 public methods**. Constructor migration support and change-listener wiring have explicit later boundaries; they are not omitted from the native release. N2.6 completes the transaction inventory; N2.7 closes input/clock/replay/preflight/source-date/serialization boundaries. The executed map covers 151 methods directly, lifetime through process reopen, and the two explicit later boundaries. At each new slice, reconcile added or changed Workbench methods against this inventory.
