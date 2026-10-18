# Agent communication for Crewroom

Research date: 2026-09-25, America/New_York. This evaluates OpenAgentEmail, NTM/Agent Mail, IRC, existing OMP messaging, Agent Relay/Relaycast, ACP, A2A, and NATS JetStream against Crewroom's approved scope. It is a research recommendation, not a dependency selection or implementation change.

Recommendation updated 2026-09-26 for the operator's explicit requirement: independently observable agent processes and communication outside OMP's internal team bus. The underlying source snapshots remain dated 2026-09-25. See [ADR 0004](../adr/0004-observe-independent-agent-processes.md).

Subsequent live result: [local Relaycast with three independent OMP processes](omp-external-roundtrip-experiment.md) passed three clean four-hop trials after two bounded adapter/protocol corrections. It is now the preferred candidate for the next interruption/recovery experiment. This result does not establish production adoption or remove the documented restart/deduplication limitations.

## Recommendation

Run the coordinator and each specialist as separate, externally supervised processes. Route their messages through a service outside those processes, with visible history and delivery outcomes. OMP remains a possible individual runner through a tested control adapter; its internal team messaging is excluded by the operator's requirement. The [native round-trip experiment](omp-roundtrip-experiment.md) succeeded but did not test the required process boundary.

For an external reusable mailbox, shortlist **standalone Rust Agent Mail** and **Agent Relay/Relaycast**. Agent Mail has a strong agent-inbox fit and useful durable cursor/idempotency features; its API/version differences and current license terms need attention. Relaycast has a clean runtime integration seam and a local Node/SQLite deployment, but ordinary-message deduplication across a self-hosted restart needs an additional durable boundary. Relaycast now has a validated narrow OMP message-probe adapter; Agent Mail remains untested in Crewroom. [Agent Mail assessment](ntm-agent-mail-assessment.md), [Relay assessment](agent-relay-assessment.md)

Do not adopt NTM solely for messaging. Its OMP control path remains terminal based, while Crewroom has already partially tested OMP's structured RPC transport. Keep OpenAgentEmail for a possible future email-facing integration and network IRC for a possible shared chat surface. Evaluate ACP as an alternative runner interface, not as the mailbox. A2A and JetStream become more relevant if independently hosted specialists or multiple machines enter scope.

The baseline for comparison is a small outbox/inbox in Crewroom's already-required durable store plus an OMP adapter. This does not assume custom implementation wins. A candidate should earn adoption by removing more persistence, delivery, recovery, and operational work than it adds.

## Three responsibilities to keep distinct

| Responsibility | Example | Owner in the recommended design |
| --- | --- | --- |
| Message history and routing | Save QA's question, its recipient, and the reply relationship | Crewroom's message contract, optionally backed by a mailbox product |
| Live session delivery | Feed that question to the selected independent process/session; observe acceptance or failure | Tested per-process runner adapter, such as OMP RPC or later ACP |
| Execution authority | Decide whether the recipient may wake, write files, retry, or advance the ticket | Crewroom workflow service, with coordinator decisions and operator steering |

This separation is necessary because the inspected systems use “acknowledged,” “read,” and “delivered” for different facts. A saved email, an IRC echo, a fetched inbox row, or a successful RPC reply does not establish that a specialist reconciled an instruction into its work. The source-specific distinctions are documented in each assessment below.

## Candidate comparison

| Candidate | Useful capability found | Deciding gap for this project | Recommended place |
| --- | --- | --- | --- |
| **OMP native messaging** | Addressed sends, busy-session injection/steering, idle wake, parked-session revival, separate session histories | Process-local bus does not meet the operator's separate-process requirement | Excluded; retain research and experiment as historical evidence |
| **OpenAgentEmail** | Authenticated task threads, results, approvals, optional leases, recorded webhook retries | SMTP/IMAP task reconstruction; task creation lacks caller retry identity; downtime reconciliation and OMP wake/control remain ours | Optional future external mailbox |
| **NTM** | Agent session orchestration, observability, assignment ledger, Agent Mail nudges | OMP path uses terminal automation and overlaps Crewroom's workflow authority | Reference/reuse candidate for selected mechanisms, not the default message layer |
| **Standalone Agent Mail** | Inbox/thread/search/ack; Rust adds bounded keyed sends and durable event cursors | Python/Rust read semantics differ; runner wake and applied-state tracking remain ours; current custom license terms need review | Serious mailbox candidate behind a pinned adapter |
| **Network IRC + Ergo** | Channels, authenticated accounts, familiar conversation clients, optional persistent/offline history | History/echo/read markers do not give durable execution or applied acknowledgements; persistence requires configuration | Optional observer or cross-harness chat transport |
| **Agent Relay/Relaycast** | Channels/DMs, durable delivery rows, explicit session handoff receipts, custom delivery targets, local deployment | Custom OMP integration and workflow policy still needed; ordinary self-hosted send deduplication is not crash-safe by itself | Serious reusable messaging candidate |
| **ACP** | Standard prompt/progress/cancel/session interface | Optional recovery capabilities; no shared durable mailbox | Evaluate alongside OMP RPC at the runner boundary |
| **A2A** | Remote task/artifact protocol, discovery, streaming, notifications | Optional send idempotency/history retention; no local process supervision | Future independently hosted specialist adapter |
| **NATS JetStream** | Durable streams, replay, consumer acknowledgements, bounded producer deduplication | Still needs the complete agent/workflow adapter and an additional broker | Defer until distributed transport is justified |

Sources: [OMP/IRC](irc-agent-communication-assessment.md), [OpenAgentEmail](openagentemail-assessment.md), [NTM/Agent Mail](ntm-agent-mail-assessment.md), [Relaycast](agent-relay-assessment.md), [ACP/A2A/JetStream](agent-protocols-assessment.md). “Found” means verified in source or specification; it does not mean a live Crewroom integration passed.

## Findings that change the decision

**OMP already contains relevant work.** Its internal facility named IRC is not a network IRC client/server. It is an in-process bus between registry-held agent sessions. The inspected 18.3.1 source supports parent steering and peer message injection, but the pending failed-delivery buffer is in memory with a 100-message cap. Peer exchanges displayed in the main UI are not thereby saved or added to the coordinator model's context. [Bus](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/irc/bus.ts), [session bridge](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/session/irc-bridge.ts)

**The operator requires separate processes as well as separate conversations.** One independently controlled OMP process per agent would give Crewroom a root-session RPC connection to each agent. This is the topology to evaluate if OMP is retained; the native team topology is no longer an open choice. The inspected RPC interface does not expose a dedicated addressed child-send command. Independent processes still need an external message adapter and do not make a shared checkout a filesystem sandbox. [RPC command union](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/modes/rpc/rpc-types.ts#L24-L94), [registry](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/registry/agent-registry.ts)

**Mailbox monitoring must not consume messages.** Python Agent Mail fetch leaves read state alone. Rust's ordinary fetch marks it read by default. NTM's current fetch client omits the opt-out, while its nudge logic counts unread entries. Source inspection therefore identifies a risk of a monitor consuming unread state and suppressing a nudge. The Rust event-cursor feed is the more promising non-consuming integration boundary. This is an integration hypothesis to test, not a reproduced live defect. [Detailed source chain](ntm-agent-mail-assessment.md#receipt-semantics-and-a-concrete-compatibility-risk)

**OpenAgentEmail is a task system as well as mail.** It implements meaningful identity, approvals, and results, but a fresh create request generates a fresh task ID. Its notification watcher also requires reconciliation for mail received during API downtime. Those specifics matter more than the general reliability of email. We would still need our own durable intent and OMP control adapter. [Task-create implementation](https://github.com/openagentemail/openagentemail/blob/5ce4ae2b055a0ba5bf092da283190e79efd78938/packages/api/src/lib/tasks-internal.ts#L2143-L2161), [watcher](https://github.com/openagentemail/openagentemail/blob/5ce4ae2b055a0ba5bf092da283190e79efd78938/packages/api/src/lib/notification-watcher.ts#L216-L238)

**Relaycast is worth a bounded comparison.** Its SDK accepts a custom session `receiveMessage` implementation and distinguishes accepted, delivered, deferred, and failed results. That is a useful OMP-adapter seam. The default delivery mode is immediate, so Crewroom must enforce its pause gate explicitly. Self-hosted Node code also separates durable delivery storage from an in-memory generic idempotency store; a retained message does not prove a retry after restart cannot create another. [Session contract](https://github.com/AgentWorkforce/relay/blob/635cfacd78000b2c023d51bd587ed1ea6a5601db/packages/sdk/src/session/types.ts#L124-L171), [delivery runner](https://github.com/AgentWorkforce/relay/blob/635cfacd78000b2c023d51bd587ed1ea6a5601db/packages/sdk/src/delivery/runner.ts), [Node runtime](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/adapters/node/index.ts#L125-L144)

**Licensing differs from some metadata.** Current NTM, Python Agent Mail, and Rust Agent Mail license files contain an OpenAI/Anthropic restriction rider. Their actual terms need consideration before reuse or redistribution; this research does not determine the operator's eligibility. Relay/Relaycast and OAE have Apache-2.0 licenses in the inspected source. The individual assessments cite exact license files rather than relying on repository badges.

## Proposed message flow

The recommended contract permits addressed specialist questions and findings while preserving coordinator oversight. Example: QA asks the implementer whether an observed behavior is intentional. Crewroom records the exchange and its assignment/revision references, checks that delivery is authorized, routes it to the correct session, and records the reply. The coordinator receives relevant findings and decisions. A peer message cannot grant itself a new scope, write ownership, or a passed quality gate.

Keep these facts separate in storage and UI:

1. **Stored:** Crewroom has durably accepted the message.
2. **Dispatch state:** held, eligible, submitted, failed, or outcome unknown.
3. **Runner acceptance:** the intended session/attempt accepted the delivery at a known boundary.
4. **Agent disposition:** applied, rejected, deferred, or superseded, with a response or plan reference.
5. **Work result:** the associated assignment eventually completed with independently checked evidence.

Not every informational message requires every acknowledgement. Assignment and steering messages do. An observer opening a thread must not advance any specialist receipt. Avoid broadcasting every message to every model; route by assignment and provide the coordinator with the relevant decision context. Preserve full history for operator inspection without requiring every agent to carry it in its prompt.

After replacement, the workflow decides which pending instructions apply to the new attempt. A logical specialist/assignment identity should outlive a process, while the delivery record retains the exact attempt/session it targeted. This prevents a late reply from an old agent being treated as the new agent's acceptance.

## Smallest experiment that can choose a path

Start with one coordinator process and two specialist processes, one controlled fixture repository, and no Jira/PR writes. Repeat the four-hop round trip through an external message service, with independently visible process activity and a passive message observer. Evaluate a reusable mailbox against a small Crewroom-owned implementation at this boundary; do not repeat the native internal-bus experiment as a candidate architecture.

| Case | Required observation |
| --- | --- |
| Assignment and peer question | Three distinct live processes and histories; addressed exchange; external progress/message visibility; no implicit shared memory |
| Specialist process exit | Other agents remain observable; exit is recorded; ownership and pending deliveries are reconciled before replacement |
| Message while paused | Stored and visible, with no model wake, tool execution, or ownership change |
| Steering during work | Distinct transport acknowledgement and agent disposition; safe interrupt when requested |
| Observer reads | Repeated monitoring does not mark specialist messages consumed or suppress delivery |
| Lost send reply | Retry/reconciliation retains one logical message and does not blindly repeat execution |
| Worker replacement | Attempt generation changes; stale receipts rejected; relevant context and pending work explicitly handed off |
| Controller/service crash | Accepted messages, delivery uncertainty, and ownership survive; completed work is preserved |
| Duplicate/out-of-order delivery | Causal identity and deduplication preserve intent; expired or superseded steering cannot silently take effect |
| Buffer or retention limit | Sender/operator see a rejection or gap; important messages are not silently discarded |
| Claimed completion | A message saying done cannot bypass required checks, independent review, or revision matching |

This experiment should record the installed binary/source identities, distinct process IDs and generations, actual delivered messages, recipient session IDs, failure points, and durable receipts. Network failures may be simulated, but the agent-session delivery and interruption must be real. Measure integration complexity and recovery behavior; do not use a successful chat demo as proof of the full workflow.

## Research evidence and remaining uncertainty

The supporting reports include inspected commits/releases, primary code/specification links, and source-versus-runtime distinctions. No third-party package was installed or executed, no chat/mail message was sent, and no provider account was configured during the source research. Existing upstream tests and CI are evidence of upstream coverage, not newly passing Crewroom tests. Subsequent local evidence includes the partial OMP control probe, the historical native three-session round trip, and the successful external Relaycast experiment across three independent OMP processes. Only the last tests the required external communication boundary. Useful coding and crash recovery remain unvalidated.

Open decisions are the per-process runner adapter, the external message-service implementation, and detailed receipt/recovery semantics. Independent agent processes and external observability are settled requirements. The approved product purpose, coordinator steering, and human merge gate remain intact.
