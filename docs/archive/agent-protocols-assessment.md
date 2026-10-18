# Agent control protocols and durable transport alternatives

Research date: 2026-09-25 (America/New_York). This is primary-source research, not an integration test or adoption decision. It complements the candidate messaging assessments. Installed OMP's `acp --help` was read; no ACP session, model request, or remote service was started.

## Agent Client Protocol: evaluate at the runner boundary

ACP standardizes the client-to-coding-agent conversation: initialize capabilities, create/load a session, send prompts, receive progress/tool events, and cancel a turn. The documented v1 stdio transport uses newline-delimited JSON-RPC between a client and agent subprocess. This closely matches Crewroom's runner-adapter boundary. It does not provide a shared, durable coordinator/specialist inbox. [Overview](https://github.com/agentclientprotocol/agent-client-protocol/blob/322cb3767498fc75bbb4ebc9b57ef2b6ec958898/docs/protocol/v1/overview.mdx), [transport](https://github.com/agentclientprotocol/agent-client-protocol/blob/322cb3767498fc75bbb4ebc9b57ef2b6ec958898/docs/protocol/v1/transports.mdx#L6-L46)

Two semantics matter for Crewroom. Loading an old session is optional and must be negotiated; possessing a session ID is insufficient. Cancellation is a notification, and the original prompt's cancelled response establishes the protocol's completed interruption boundary. Crewroom should retain its own pausing state until that result and applicable process/tool observations support stopping. [Session loading](https://github.com/agentclientprotocol/agent-client-protocol/blob/322cb3767498fc75bbb4ebc9b57ef2b6ec958898/docs/protocol/v1/session-setup.mdx#L83-L104), [cancellation](https://github.com/agentclientprotocol/agent-client-protocol/blob/322cb3767498fc75bbb4ebc9b57ef2b6ec958898/docs/protocol/v1/prompt-turn.mdx#L334-L367)

The installed OMP 18.3.1 help advertises `omp acp` as an Agent Client Protocol server over stdio. That is discovery evidence only. The previous [OMP experiment](../research/omp-contract-experiment.md) exercised OMP's own RPC interface, which must not be described as an ACP test. Compare both interfaces with the same useful-execution, steering, interrupt, session-reopen, and abrupt-failure cases before preferring either. The potential benefit of ACP is a standard runner boundary for additional harnesses; it is not a reason to delay the known OMP RPC experiment or assume complete capability parity.

Scope/version: inspected ACP repository commit `322cb3767498fc75bbb4ebc9b57ef2b6ec958898`, v1 documentation. Latest listed schema release was [schema-v1.23.0](https://github.com/agentclientprotocol/agent-client-protocol/releases/tag/schema-v1.23.0). Schema/package release numbers and negotiated protocol versions are different concepts. Do not silently substitute a draft protocol for the version OMP supports.

## A2A: useful if specialists become independently hosted services

A2A specifies messages, stateful tasks, artifacts, discovery, task queries/cancellation, streaming, and push notifications. It is a better fit for a specialist exposed as a service than for supervising a local coding-agent process. That is a Crewroom fit assessment, not a protocol limitation. [Specification](https://github.com/a2aproject/A2A/blob/72b3761bd84c59291da694dcd97cdfc2c010df39/docs/specification.md)

The inspected specification makes Send Message idempotency optional and leaves message-history retention to the agent. A compliant interface therefore does not by itself prove duplicate-safe dispatch or a complete recovery log. Crewroom still needs persistent dispatch identity, actual evidence, and reconciliation of Jira/PR side effects. [Idempotency](https://github.com/a2aproject/A2A/blob/72b3761bd84c59291da694dcd97cdfc2c010df39/docs/specification.md#L488-L497), [history](https://github.com/a2aproject/A2A/blob/72b3761bd84c59291da694dcd97cdfc2c010df39/docs/specification.md#L750-L760)

Recommendation: retain as a future adapter for remote specialists. It adds little to the first local, single-operator milestone. Inspected main was `72b3761bd84c59291da694dcd97cdfc2c010df39`; latest listed release was [v1.0.1](https://github.com/a2aproject/A2A/releases/tag/v1.0.1). Main and released implementations need separate conformance checks.

## NATS JetStream: durable transport if multiple machines become necessary

JetStream provides retained streams, durable consumers, acknowledgement/redelivery, and producer deduplication by message ID. Those are useful building blocks for workers that reconnect. Core NATS has different delivery guarantees, so selecting NATS without explicitly selecting JetStream would not satisfy the same requirement. [Consumer documentation](https://docs.nats.io/learn/jetstream/pull-consumers), [JetStream delivery and deduplication](https://github.com/nats-io/nats.docs/blob/master/using-nats/jetstream/model_deep_dive.md)

The documented exactly-once mechanisms concern broker publication/acknowledgement. They do not atomically include a Git modification, model turn, or remote PR creation. Crewroom must still make those operations idempotent or reconcile ambiguous outcomes. This is a boundary inference, not a claim that broker guarantees are invalid. [Guarantee scope](https://github.com/nats-io/nats.docs/blob/master/using-nats/jetstream/model_deep_dive.md)

Recommendation: defer. A separate broker has operational cost and does not supply agent context, session control, evidence gates, or the workbench. The current one-host, three-specialist limit provides no established need for it.

## Local durable outbox plus a runner adapter: the baseline

Crewroom already needs durable runs, assignments, attempts, steering, evidence, and external-operation intents. A small message/outbox component in that store is therefore a useful comparison baseline. It would commit an addressed message and its pending delivery with the workflow transition, then dispatch through a tested OMP RPC or ACP adapter. This is a proposed design, not an implemented feature or proof that custom code will be cheaper.

Minimum behavior to compare against reusable messaging products:

- Stable message ID, sender/recipient assignment, attempt generation, causal reply ID, requirements revision, and optional code/evidence references.
- Separate durable storage, dispatch, runner acceptance, agent disposition, and associated work completion. No single read/ACK flag stands for all five.
- Observer queries do not consume an agent's inbox or advance its acknowledgement cursor.
- Deferred messages remain visible while paused; receipt cannot wake an unauthorized attempt.
- A replaced agent cannot acknowledge or apply work addressed to the previous execution generation. Handoff explicitly determines which messages remain relevant.
- On crash, reconcile dispatch and external effects before retrying. Do not promise exactly-once execution merely because the message store deduplicates a row.
- Specialist questions can travel directly through the recorded service. The coordinator receives relevant decisions/blockers; task assignment, worktree ownership, and scope changes stay under workflow control.

Recommendation: use this boundary to evaluate Agent Mail or Relaycast reuse. Prefer one authoritative record of each fact and reference external message IDs when an adapter owns the mailbox. Avoid adopting a competing workflow engine merely to obtain inboxes. No protocol, broker, or messaging product in this review replaces the approved human merge gate or revision-specific evidence.
