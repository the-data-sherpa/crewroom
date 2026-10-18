# IRC and OMP agent communication assessment

Research date: 2026-09-25, America/New_York. This is primary-source inspection, not an implementation or runtime validation. No IRC server, agent session, network chat connection, or integration was started. No credentials were inspected.

## Recommendation

Keep Crewroom's durable message and workflow records as the authority. Evaluate OMP's existing peer messaging before building another live delivery mechanism. Consider a real IRC server as an optional shared conversation surface if connecting independent processes, other harnesses, or ordinary chat clients becomes useful. For the accepted single-operator pilot, a new IRC server is unlikely to remove enough application work to justify making it a core dependency.

The important discovery is that **OMP calls its internal agent messaging “IRC,” but the inspected implementation is not the IRC network protocol**. It is a process-local bus connecting distinct agent sessions in a registry. Separate OMP processes do not become peers merely by enabling this facility. That limitation follows from the singleton registry and in-memory bus; it does not mean the agents share conversation histories. [OMP bus, pinned v18.3.1](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/irc/bus.ts#L1-L54)

Neither a chat-server acknowledgement nor an OMP delivery receipt proves that an agent followed an instruction. Crewroom still needs explicit assignment ownership, message correlation, controlled activation, and evidence tied to the code revision. This recommendation follows from the source behavior below and the [accepted product requirements](../product.md).

## Source identity and limits

- The existing Crewroom experiment observed an OMP executable reporting **18.3.1**. The global JavaScript package inspected locally reports **17.4.0** and is not a version-matched description of that executable. See [the experiment record](../research/omp-contract-experiment.md) and [local package manifest](/Users/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/package.json:4).
- Upstream tag **v18.3.1** resolves to `6204b7508014bcdf12d95f0b3fd470905fd99344`. All OMP GitHub links in this report use that commit. Source inspection does not verify that the local binary was built without modification from this tag. The GitHub API also returned v18.3.2 as a newer release, published at 2026-09-26 00:00:25 UTC; this report deliberately uses the version matching the previously observed binary.
- The latest published Ergo release returned during this investigation was **v2.19.1**, published 2026-08-05. Its annotated tag resolves to `63c743a70644f0f19109508ade8580bf37f7d23d`. [Release](https://github.com/ergochat/ergo/releases/tag/v2.19.1)
- IRCv3 specifications were read from their official site. `draft/chathistory` and `draft/read-marker` remain draft capabilities in those documents; support must be negotiated and tested against the selected implementation.
- No upstream tests were executed. Test names below identify relevant existing coverage, not newly passing checks.

## What a real IRC stack provides

IRC supplies addressed messages and channels. IRCv3 adds useful primitives, but their scope matters:

| Primitive | Useful guarantee or behavior | What Crewroom still needs |
| --- | --- | --- |
| `msgid` | Server-assigned message identifier; history replay retains it. IDs are opaque and are not sortable sequence numbers. [Specification](https://ircv3.net/specs/extensions/message-ids) | Its own stable message/attempt ID, retry key, and code/brief references. A resent request can be a new IRC message with the same logical intent. |
| `echo-message` | The server echoes the accepted representation to the sender. The specification even permits a synthetic echo when a message was silently filtered. [Specification](https://ircv3.net/specs/extensions/echo-message) | Recipient acknowledgement and application outcome. An echo is not proof of recipient processing or durable commit. |
| `labeled-response` | Correlates a client request with the server's reply or reply batch. Its generic `ACK` can mean that a command otherwise had no response. [Specification](https://ircv3.net/specs/extensions/labeled-response) | An application-level request/reply protocol with deadlines and explicit states. Server correlation does not mean the specialist accepted an assignment. |
| `draft/chathistory` | Retrieves retained messages before/after a cursor and discovers conversation targets. Live/history overlap requires deduplication; retention, permissions, limits, and ordering still apply. [Specification](https://ircv3.net/specs/extensions/chathistory) | Durable outbox/inbox, restart cursor, detection of missing history, and handling for unknown delivery outcomes. |
| `batch` | Groups related IRC events and marks the end of a history response. [Specification](https://ircv3.net/specs/extensions/batch) | Transactional workflow state. A completed history batch is not a transaction committing an agent action. |
| `draft/read-marker` | Synchronizes read position among one user's clients; it is not normally disclosed to other users and is not a socket-delivery receipt. [Specification](https://ircv3.net/specs/extensions/read-marker) | Agent-specific `received`, `applied`, `rejected`, or `superseded` records. A read marker cannot substitute for these. |
| Account identity | `account-tag` carries the authenticated account of the sender. [Specification](https://ircv3.net/specs/extensions/account-tag) | Binding an authenticated account/session to a Crewroom run, assignment, permitted sender, and current attempt. A display nickname alone is insufficient. |

The history specification explicitly describes ambiguity in global order and clock skew across linked servers. For a single local server that complexity is smaller, but application sequence numbers and explicit causal references remain preferable to treating chat order as workflow order. History also retrieves only messages that the server still retains. [IRCv3 history considerations](https://ircv3.net/specs/extensions/chathistory#implementation-considerations)

Assignments and evidence can exceed IRC's conventional line framing. A bridge should send a small structured envelope and an artifact reference rather than an entire repository diff or validation log. Optional extensions and client support affect larger messages; there is no reason for Crewroom's evidence model to inherit those constraints. [IRC message format](https://modern.ircdocs.horse/#message-format)

## Ergo is a credible local IRC candidate

Ergo combines server, account management, history, and bouncer behavior in one program. Its always-on accounts can remain present and receive direct messages while no client is attached. Version 2.19.1 supports persistent history using SQLite, PostgreSQL, or MySQL; SQLite avoids a separate database service. These features make it a reasonable candidate for a self-hosted experiment. [Pinned manual](https://github.com/ergochat/ergo/blob/63c743a70644f0f19109508ade8580bf37f7d23d/docs/MANUAL.md#persistent-history)

Configuration is significant: default history is RAM-only, persistent history is disabled, and default expiry is one week. Direct-message persistence applies to logged-in clients, while registered-channel and unregistered-channel policies differ. Thus “IRC with Ergo” does not imply durable delivery without specifying the configuration. [Pinned defaults](https://github.com/ergochat/ergo/blob/63c743a70644f0f19109508ade8580bf37f7d23d/default.yaml#L1062-L1142)

The SQLite implementation selects WAL with `synchronous(NORMAL)` and inserts a history item and its lookup records through separate operations. That is useful persistent chat storage, but this inspection did not establish a write-ahead acknowledgement contract for message transport or prove behavior under abrupt power loss or a partial database error. Do not promote server receipt into a Crewroom durable-action guarantee on this evidence. [SQLite implementation](https://github.com/ergochat/ergo/blob/63c743a70644f0f19109508ade8580bf37f7d23d/irc/sqlite/history.go#L102), [history insertion](https://github.com/ergochat/ergo/blob/63c743a70644f0f19109508ade8580bf37f7d23d/irc/sqlite/history.go#L587-L613)

A suitable experiment would use loopback binding, authenticated identities, persistence explicitly enabled, and a retention policy consistent with the run record. Only the server-side bridge would hold credentials. These are proposed experiment settings, not changes made during research.

## What OMP already implements

The native design is considerably closer to agent coordination than raw IRC. Source inspection of the matching tag establishes these behaviors:

| Area | Source finding | Consequence for Crewroom |
| --- | --- | --- |
| Session boundary | Registry entries carry separate session references and histories. Its visible peer roster is a flat namespace of running/idle agents, excluding advisors. [Registry](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/registry/agent-registry.ts#L330-L345) | Separate specialist sessions can exist within one OMP process. Session isolation is not process isolation or a permission boundary. |
| Addressed sends | `executeSend` accepts a recipient or broadcast, supplies the sender from execution context, attempts persisted-roster restoration when needed, and returns per-recipient delivery outcomes. [Messaging](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/irc/messaging.ts#L44-L101) | Useful mechanism to reuse, but scope checks and durable correlation must come from Crewroom. |
| Live delivery | Parent-to-busy-child messages become steering; peer-to-busy-peer messages queue for injection. Idle recipients ordinarily start a turn. Idle plan-mode recipients receive a persisted custom message without a wake. [Bridge](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/session/irc-bridge.ts#L175-L232) | Crewroom's paused state must be enforced before delivery. OMP plan mode is not a substitute for workflow pause. |
| Receipt | Outcomes distinguish injected, woken, revived, and failed. The bus explicitly says these describe delivery rather than what the recipient did. [Receipt semantics](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/irc/bus.ts#L56-L66) | Map these to transport facts, never automatically to `applied`. |
| Buffer | Pending bus mail is held in process memory; only failed live handoffs are buffered. The cap is 100 messages and overflow drops the oldest. [Buffer and cap](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/irc/bus.ts#L24-L45) | This is not the durable Crewroom inbox/outbox. |
| Recovery | Lifecycle management can park and revive agent sessions, coalesces concurrent revival, and binds operations to an exact registry reference. Persisted-roster logic restores session-backed entries from disk. [Lifecycle](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/registry/agent-lifecycle.ts#L1-L20), [persisted roster](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/registry/persisted-agents.ts#L615) | Reuse candidate for continuity; restoring a transcript is different from replaying all pending messages and reconciling external actions. |
| Operator visibility | Peer-to-peer traffic relays to the main UI as a display-only card. The receiving bridge emits that observation without persisting it. [UI relay](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/irc/bus.ts#L350-L375), [observation](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/session/irc-bridge.ts#L235-L238) | Seeing a card does not establish that the coordinator model received the exchange or that Crewroom retained it. |

An additional version difference matters: the local 17.4.0 `irc-bridge.ts` contains ephemeral side-channel auto-reply behavior. The inspected 18.3.1 bridge instead marks idle specialist wake turns for output relay and guards against relay loops. Do not carry the local package's auto-reply assumptions into a binary contract. [Local older bridge](/Users/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/session/irc-bridge.ts:111), [version-matched relay test](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/test/session/irc-bridge-relay.test.ts)

The version-matched RPC command union exposes subagent roster/history reads and subagent subscriptions. Its ordinary prompt/steer/abort commands target the root session; the union contains no dedicated addressed child-send command. Therefore, native OMP peer messaging should not be assumed directly callable from Crewroom over the existing RPC control interface. A host tool, extension, SDK seam, or separate session routing may be needed and must be tested. [RPC types](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/modes/rpc/rpc-types.ts#L24-L94)

Existing source tests cover delivery failures, park/revive races, concurrent sends, FIFO waiters, the 100-message overflow policy, idle wake, peer injection, and parent steering. They provide useful cases to adapt; their presence is not evidence that Crewroom or the installed binary passes them. [OMP messaging tests](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/test/tools/irc.test.ts)

## Architecture implication

There are two viable OMP arrangements, both compatible with distinct agent conversations:

1. **One OMP process managing a native team.** Native peer messaging and lifecycle management do more of the work. Crewroom must prove interception/persistence of every significant exchange, coordinator visibility, pause enforcement across child sessions, and assignment-level controls. A single process failure can affect the whole team.
2. **One controlled OMP process per agent session.** Crewroom owns the routing through each process's RPC interface. It has clearer per-agent scheduling and lifecycle control, but the native process-global peer bus does not connect these processes. A Crewroom message tool and durable router are still needed; a real IRC transport could be inserted behind that boundary later.

These are design inferences from the inspected source, not a recommendation to replace the already approved workflow or to adopt a runner now. The next contract experiment should settle which arrangement meets Crewroom's supervision requirements with the least custom code.

For either arrangement, a proposed durable envelope should include a stable message ID; run, assignment, and attempt IDs; authenticated sender and intended recipient; message kind; causal/reply reference; requirement/code revisions when relevant; body or artifact reference; and delivery policy. The workflow service should separately record saved, transport-accepted, recipient-accepted, and applied/rejected/superseded outcomes. Retries should preserve logical identity. Neither chat history nor an agent saying “done” should advance a quality gate.

## Contract experiments before adoption

1. Use a coordinator and two specialists with separate sessions. Verify addressed questions, replies, and coordinator visibility with actual source/recipient identity, without terminal injection.
2. Queue a message while the recipient is busy, idle, paused, and parked. Prove that a Crewroom pause prevents any model or tool execution even if native delivery ordinarily wakes a session.
3. Stop the bridge after durable save, after dispatch, and after recipient receipt. Reopen it and prove duplicate-safe handling or a truthful unknown state. Do not equate a restored transcript with pending-message recovery.
4. Restart the agent process. Verify intended session identity, worktree ownership, retained context, and pending messages; reject old-attempt messages against a replacement assignment unless explicitly transferred.
5. For an Ergo variant, restart both client and server, exhaust history retention, overlap live traffic with replay, and lose a send acknowledgement. Verify cursor recovery, deduplication, and detection of gaps.
6. Deliberately delay a response until a later message has been sent. Correlate by logical message ID, not merely sender or arrival order. Confirm that a displayed peer exchange does not get mislabeled as coordinator-model context.
7. Exercise an unknown recipient, cancelled assignment, over-capacity inbox, and a disallowed cross-run sender. Preserve the reason instead of dropping or silently rerouting messages.

No experiment in this list was performed by this research task. The existing partial RPC controls remain documented separately in [the OMP experiment report](../research/omp-contract-experiment.md).
