# A complete local runtime for Crewroom

Date: 2026-09-27. Proposed response to the operator's tmux/backend question. This supplements the approved architecture; it does not adopt a runtime dependency or report an executed tmux integration.

Subsequent execution: the operator authorized testing Rust Agent Mail. [Nine mailbox checks and five OMP/tmux checks passed](../testing/rust-mail-results-2026-09-27.md), establishing a bounded part of this proposal with real independent processes and scripted providers. General supervision, child-tool interruption, and complete coding workflow tests below remain outstanding. No fallback or custom mailbox was required.

Subsequent decision: the operator approved adding the validated stack to the architecture. [ADR 0005](../adr/0005-use-omp-tmux-and-rust-agent-mail.md) and the [selected runtime section](../architecture.md#selected-runtime-technologies) now govern implementation. This document preserves the preceding proposal and its rationale.

## Recommendation

Evaluate **tmux + a durable message backend + Crewroom supervision** as one operating stack. Each coordinator and specialist remains a separate OMP process. tmux supplies attachable views; the message backend stores addressed exchanges; Crewroom owns the workflow, control policy, and recovery decisions. The coordinator agent reasons about work while the workflow service enforces ownership, limits, and evidence requirements.

The next reusable mailbox candidate remains standalone Rust Agent Mail, subject to the [source findings, retention/read semantics, license facts, and contract test](agent-inbox-followup-2026-09-27.md). A narrow inbox in Crewroom's planned SQLite store is the fallback. This proposal needs neither NTM nor OMP's internal team messaging.

## What tmux contributes

tmux runs programs in separate panes and lets clients detach and reattach while the server retains the running sessions. It provides an additional way to inspect the team when the browser is closed. Detaching a client is different from a tmux server crash, computer restart, or interrupted agent; those require recovery outside tmux. [Official introduction](https://github.com/tmux/tmux/wiki/Getting-Started).

`send-keys` injects terminal input. A successful tmux command does not establish that OMP durably received a structured message or applied it. Control mode exposes pane output and tmux command results; those are terminal/process observations rather than agent delivery receipts. Use those facilities for terminal management, while agent messages use the recorded backend and runner interface. [tmux manual](https://man.openbsd.org/tmux.1#send-keys), [control mode](https://github.com/tmux/tmux/wiki/Control-Mode).

The installed executable reports tmux 3.7c. This turn did not start a tmux server, launch agents, install a mailbox, or run a combined-stack experiment.

The [tmux source assessment](tmux-supervision-assessment-2026-09-27.md) pins these boundaries to the installed release and covers pane/process identity, output buffering, and descendant handling in more detail.

## Concrete process arrangement

```text
Browser workbench
    |
Crewroom local backend
    |-- Workflow store: runs, assignments, attempts, steering, evidence, actions
    |-- Supervisor: ownership, start/pause/recovery, model validation
    |-- Message adapter ---- Durable message service
    |
tmux session: one visible pane per agent
    |-- Coordinator host ---- OMP process A
    |-- Specialist 1 host --- OMP process B
    |-- Specialist 2 host --- OMP process C
```

Each proposed host adapter owns one OMP child's RPC pipes, handles its permitted message tool, captures structured events, and renders readable activity in its pane. The supervisor communicates with these hosts over a local control connection rather than sending terminal keystrokes. Local IPC admission must bind each host to its run/assignment/attempt; neither a pane name nor a caller-supplied sender field establishes identity. Mailbox credentials remain in the backend.

OMP's tested RPC mode uses `--no-ui` and dedicated stdin/stdout pipes. A tmux pane in this arrangement would show the host's live activity display, **not an assumed simultaneous native OMP TUI**. The browser and terminal would display the same recorded observations. A native interactive OMP terminal plus structured control would require a separate runner contract. [Existing adapter fixture](../../scripts/probe-omp-external-messages.py), [executed external-message experiment](omp-external-roundtrip-experiment.md).

The host must persist or forward acknowledged event records independently of terminal scrollback, identify child tools, and expose reconnectable control. A lost supervisor connection must stop admission of further work and trigger the agreed pause behavior. Expiring a lease does not stop an already running shell command: replacements must wait until old ownership and descendants are resolved. These are proposed requirements, not capabilities established by the existing message experiment.

## End-to-end message path

1. Specialist 1 invokes its allowed message tool with the intended recipient, payload, and reply reference.
2. Crewroom derives sender identity from the current host/attempt, checks the destination, and durably records the outbound intent with a stable logical ID.
3. The message adapter submits that ID to the mailbox and retains the returned transport identity. A lost reply causes reconciliation; it does not justify an unqualified resend after retry records expire.
4. Crewroom records inbound acceptance durably before acknowledging transport delivery. Dispatch waits for the recipient's allowed state; paused work stays queued. Idle wake and busy-agent steering use the tested runner interface and an explicit delivery policy.
5. Recipient progress, applied/rejected steering, and completed work are separate observations. A message receipt does not certify any code or external action.
6. The browser and tmux displays read observations without consuming the recipient's acknowledgement state. A peer exchange remains visible even when the coordinator is not part of that conversation.

An external mailbox may remain the authoritative message-body store. Crewroom records logical IDs, workflow decisions, delivery/disposition mappings, and external-action intents; it need not maintain an unnecessary second complete mailbox. Recovery inputs and referenced artifacts must remain available for the run's required retention period.

## Coverage of the accepted needs

| Need | Responsible component and acceptance boundary |
| --- | --- |
| Separate observable agents | Per-agent OMP process, host event stream, browser view, and tmux pane; process identity survives pane renaming and distinguishes replacement attempts |
| Coordinator and peer communication | Durable message service plus authorized host-tool adapter; addressed messages, reply correlation, duplicate handling, and independent observer |
| Optional specialist roles | User role catalog and snapshotted assignments; inserting a role creates explicit work/dependencies |
| Different models per specialist | Role/model policy and runner preflight; record requested and resolved model/effort, then verify runner state before work |
| Operator steering | Coordinating conversation and durable steering records; specialist inspection does not require direct specialist control |
| Busy, idle, paused, stalled agents | Supervisor state and delivery policy; activity, liveness, and completed milestones remain distinct |
| Safe pause/cancel | Stop admission, interrupt, verify agent/tool descendants stopped, reconcile uncertain effects; retain pausing when unconfirmed |
| Agent or backend restart | Durable inputs/checkpoints, attempt ownership and reconnect policy, bounded resume/replacement; no concurrent old/replacement writer |
| One writer per worktree | Workflow admission and verified process ownership; tmux panes alone provide no filesystem isolation |
| Tests and independent review | Executed checks and separate reviewer evidence tied to the exact code/base revision; changed revision invalidates readiness |
| Jira and PR updates | Persisted intents, provider-specific readback/reconciliation, confirmed external identities; a mailbox ACK is insufficient |
| Time/retry limits | Persisted execution accounting, two-cycle repair/replacement bound, explicit operator escalation |

The retired OMP team provides reusable ownership, revision-evidence, completed-phase continuation, and publication-reconciliation mechanisms. Its fixed phase order, old model pins, and full installation remain inactive. [Reuse assessment](../research/omp-team-reuse-assessment.md).

## Next combined contract experiment

Use a disposable repository, a dedicated tmux socket/session, independent OMP processes, scripted loopback providers, and synthetic Jira/PR endpoints. Preserve the offline suite's external-network guard. No live ticket, PR, or infrastructure writes are required.

1. Complete coordinator → Specialist 1 → Specialist 2 → Specialist 1 → coordinator. Observe distinct OMP PIDs/sessions, correlated messages, actual model bindings, and matching terminal/browser event views.
2. Detach and reattach the terminal; reconnect the browser; restart the observer. Require continuous records without consuming messages or starting another worker.
3. Lose a send reply, restart the mailbox, and retry. Require stable logical/transport identity within the declared contract; test concurrent keys, payload conflict, pagination, and expired retention explicitly.
4. Send while idle, busy, and paused. Idle work wakes once; busy work follows the recorded delivery policy; paused work creates no model/tool execution.
5. Kill OMP during its first unfinished turn and after a persisted checkpoint. Recover required inputs, choose resume versus replacement, and preserve uncertainty about effects.
6. Restart the Crewroom supervisor while hosts remain alive. Reconnect to the existing owners; reject a second writer. Separately fail a host and the dedicated tmux server and reconcile their descendants before replacement.
7. Pause during a real disposable child-tool command. Show paused only after both OMP work and child work stop; preserve changes and record unresolved outcomes accurately.
8. Exercise supported and unsupported model/effort assignments. Record normalization and reject it when the assignment requires an exact setting.
9. Run one synthetic coding job through a real local edit, failed check/rework, independent review, and revision-bound evidence. Lose simulated Jira/PR responses and demonstrate reconciliation without duplicate writes.

Passing this slice would establish the combined local mechanism. It would still not certify live provider quality, Jira/Bitbucket credentials or remote semantics, or completion of the full application acceptance suite. Use the [existing execution report](../testing/results-2026-09-26.md) to preserve current pass/failure scope.
