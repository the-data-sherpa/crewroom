# Agent inbox follow-up — 2026-09-27

Execution follow-up: the operator authorized the candidate test after this source review. [Fourteen bounded mailbox and OMP/tmux checks passed](../testing/rust-mail-results-2026-09-27.md). The source-only findings below describe the preceding assessment; the execution report now supplies runtime evidence and its limits. No fallback or custom mailbox was needed.

## Decision

**Rust Agent Mail is a more promising mailbox candidate for the specific restart-deduplication failure. It is not yet a proven replacement for Relaycast, and Python Agent Mail does not offer the same improvement.** This is a source review, not a runtime result. No upstream programs, installers, model requests, or services were run.

Crewroom's [executed tests](../testing/results-2026-09-26.md) found that Relaycast preserved pending messages but lost send deduplication across service restart. The other failures concern OMP's first-turn persistence and model-effort normalization. Replacing a mailbox cannot itself fix those runner behaviors. A mailbox can retain the first input independently; Crewroom must still decide whether to resume, replace, replay, or reconcile the interrupted attempt.

The useful distinction is between **durable agent inboxes**, which Crewroom needs, and **a particular inbox product**, which remains replaceable. In either design, workflow ownership, execution attempts, external-action reconciliation, and revision-bound evidence remain Crewroom responsibilities.

## Versions checked

GitHub's commit and latest-release APIs were checked on September 27, 2026. Main and release are deliberately distinguished:

| Implementation | Current main inspected | Latest published release returned by GitHub |
| --- | --- | --- |
| Python | [`4b11f26277f611e60bcfdb3858c305d1af2fcc53`](https://github.com/Dicklesworthstone/mcp_agent_mail/commit/4b11f26277f611e60bcfdb3858c305d1af2fcc53), September 22 | [v0.3.2](https://github.com/Dicklesworthstone/mcp_agent_mail/releases/tag/v0.3.2), April 16; tag resolves to `32783f6848bd63c425c4b5004cee3350016635fb` |
| Rust | [`35fe543a03d4ba981f5110826a1009b54d0551e0`](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/commit/35fe543a03d4ba981f5110826a1009b54d0551e0), September 27 | [v0.3.36](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/releases/tag/v0.3.36), September 16; tag resolves to `071e1c85d06b6247bbddab11cc8139b38cc885bf` |

Python main declares 0.3.4, and Rust main still declares 0.3.36. Therefore a version string alone does not identify the inspected behavior. [Python metadata](https://github.com/Dicklesworthstone/mcp_agent_mail/blob/4b11f26277f611e60bcfdb3858c305d1af2fcc53/pyproject.toml#L5), [Rust metadata](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/Cargo.toml#L19).

## Retry durability

Rust's database path writes the message and the idempotency key, argument fingerprint, and original result in **one transaction**. A repeated unexpired key returns the original message ID; a changed fingerprint returns a conflict. This directly addresses the mechanism behind the observed Relaycast failure. It is a source-derived expectation pending a kill/restart experiment against the selected binary. [Transaction implementation](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/crates/mcp-agent-mail-db/src/queries.rs#L8695).

The default key lifetime is **24 hours**, configurable with `AM_IDEMPOTENCY_RETENTION_SECS`. Keys are scoped by project and tool. Expired records are removed and the same key becomes fresh; zero or invalid retention values revert to the default. Consequently this is bounded retry protection, not unlimited deduplication or exactly-once agent work. Crewroom still needs durable logical message IDs and a recovery policy for an outage longer than the retention window. [Retention implementation](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/crates/mcp-agent-mail-db/src/idempotency.rs#L21).

These APIs also exist in the v0.3.36 tag source; they are not solely current-main documentation: [released send signature and contract](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/071e1c85d06b6247bbddab11cc8139b38cc885bf/crates/mcp-agent-mail-tools/src/messaging.rs#L1825), [released retention implementation](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/071e1c85d06b6247bbddab11cc8139b38cc885bf/crates/mcp-agent-mail-db/src/idempotency.rs#L37). Release assets were not executed or independently attested.

Python main has no caller idempotency key in `send_message`. Its inbox is a limited newest-first query with a strictly-newer timestamp filter; no equivalent recipient delivery cursor was found in the inspected API. It persists mail but does not supply the retry and replay contract needed to replace Crewroom's own reconciliation logic. [Send signature](https://github.com/Dicklesworthstone/mcp_agent_mail/blob/4b11f26277f611e60bcfdb3858c305d1af2fcc53/src/mcp_agent_mail/app.py#L7132), [inbox query](https://github.com/Dicklesworthstone/mcp_agent_mail/blob/4b11f26277f611e60bcfdb3858c305d1af2fcc53/src/mcp_agent_mail/app.py#L4466).

## Receipts and observation

Rust has a persistent, oldest-first `fetch_inbox_events` feed with page continuation and explicit expired/ahead cursor errors. It is body-free and does not mark mail read. Consumers save their cursor after processing a page. `get_message_delivery_receipt` separately reports persisted, signaled, and acknowledged facts. None establishes that OMP durably accepted an input, applied steering, or completed a task. [Current feed and receipt code](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/crates/mcp-agent-mail-tools/src/messaging.rs#L4503).

Its ordinary `fetch_inbox` **marks returned messages read by default**, including header-only fetches; monitoring must use the event feed or `mark_read=false`. Python's fetch leaves read/ack timestamps unchanged but clears an enabled notification signal. Observation and wakeup must therefore be tested independently. [Rust fetch](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/crates/mcp-agent-mail-tools/src/messaging.rs#L4137), [Python fetch](https://github.com/Dicklesworthstone/mcp_agent_mail/blob/4b11f26277f611e60bcfdb3858c305d1af2fcc53/src/mcp_agent_mail/app.py#L9592).

## Integration cost

Rust documents HTTP and stdio MCP transports, macOS release targets, an OMP configuration helper, browser mail views, and a substantial TUI. Main's OMP helper writes project and user MCP configuration; it is not required for a Crewroom-controlled adapter. Main also has an ATC operator enabled by default in shadow mode. A mailbox-only experiment should disable it with `AM_ATC_ENABLED=false`, keeping Crewroom as the sole workflow authority. [Integration and transport](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/README.md#L474), [ATC settings](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/README.md#L1140).

For Crewroom, my recommendation is to retain the already tested per-process OMP RPC/host-tool boundary. The host translates a restricted send/read/ack contract into mailbox requests, owns credentials, persists execution state, and exposes messages to the workbench. Each coordinator and specialist stays a separate observable OS process. Direct MCP access may be possible, but it is not a substitute for Crewroom's authorization and recovery mapping.

Rust is a larger dependency than a tiny local queue: current source uses FastMCP, Asupersync, FrankenSQLite/SQLModel, search, and TUI components, with a pinned nightly toolchain. Python uses Python 3.12+, FastMCP/FastAPI, SQLAlchemy, SQLite, and GitPython. Neither ecosystem has been installed or timed in this follow-up. [Rust dependencies](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/Cargo.toml#L28), [Rust toolchain](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/rust-toolchain.toml), [Python dependencies](https://github.com/Dicklesworthstone/mcp_agent_mail/blob/4b11f26277f611e60bcfdb3858c305d1af2fcc53/pyproject.toml#L25).

## Relaycast and other options

Relaycast was rechecked on September 27. GitHub still identifies v8.13.0 and main `130c8204886acdb111ee2fd67774dacf5b4e4637`, the revision used in Crewroom's experiments. No newer upstream fix was found in that check. Its local Node adapter still constructs an in-memory key-value store. Ordinary DM retry results are saved after the message operation, outside the message transaction. Merely persisting that cache would leave a crash window between accepting the message and saving its retry result. [Release](https://github.com/AgentWorkforce/relaycast/releases/tag/v8.13.0), [runtime wiring](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/adapters/node/index.ts#L132), [idempotency middleware](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/middleware/idempotency.ts#L246).

Relaycast remains the only external mailbox with a successful Crewroom independent-process round trip and fault-test evidence. Its retained pending deliveries and passive observer are useful. The new result changes the next experiment's priority; it does not establish that the entire service is unsuitable. See the [executed results](../testing/results-2026-09-26.md) rather than treating this source comparison as another runtime test.

Two further options deserve different priorities:

- **A narrow Crewroom-owned SQLite inbox/outbox:** keep logical message IDs, recipient deliveries, and workflow transitions in the existing planned local database. Atomic transactions can avoid splitting a local transition from its outgoing message record. This is an architectural inference from [SQLite's transaction guarantees](https://www.sqlite.org/atomiccommit.html), not an implemented or tested Crewroom capability. Crewroom would own retry policy, concurrency, attempt fencing, retention, migrations, and the passive event feed. It may reduce dependencies for one operator on one host, but it adds code we must maintain. All OMP agents would still run in separate processes outside this store.
- **NATS JetStream:** a credible broker if Crewroom later needs distributed delivery. It provides stored streams, consumer acknowledgements/redelivery, and duplicate-publication controls. It would still need the agent adapter, operator view, and workflow/action records. I would defer its additional broker configuration for the accepted local scope. This is a fit judgment, not a comparative reliability benchmark. [Publishing and retry controls](https://docs.nats.io/learn/jetstream/advanced-publishing), [acknowledgements and redelivery](https://docs.nats.io/learn/jetstream/acknowledgment).

**Next choice:** test pinned Rust Agent Mail against the same mailbox fault contract before investing in Relaycast-specific repair code. If its contract or integration cost is unsuitable, evaluate the narrow local SQLite inbox. Retain Relaycast's working adapter as a comparison baseline. No production dependency selection has been made.

## License facts

Both inspected current license files are titled MIT License with an OpenAI/Anthropic rider. The rider defines restricted parties to include the named companies, affiliates, and parties acting on their behalf, for their benefit, or under their direction. It denies rights to those parties absent prior written permission and also restricts providing access. Its defined uses include executing, testing, benchmarking, and analyzing. Python's package metadata still says MIT; Rust metadata says `LicenseRef-MIT-Rider`. These are factual file contents, not a determination of the user's eligibility or legal rights. [Python license](https://github.com/Dicklesworthstone/mcp_agent_mail/blob/4b11f26277f611e60bcfdb3858c305d1af2fcc53/LICENSE#L12), [Rust license](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/LICENSE#L12).

## What would justify switching

Run the same bounded offline contract against a pinned Rust build, with no live models or external services:

1. Persist a keyed send, lose its reply, kill the mailbox, restart, and resend. Require one message ID and one delivery event.
2. Repeat concurrently and with a changed payload. Require one winner and an explicit conflict for changed content.
3. Advance beyond configured key retention. Require Crewroom reconciliation instead of blind resend.
4. Produce several inbox pages, restart the observer, and prove complete recovery without changing specialist read/ack state.
5. Kill OMP during its first turn. Recover the saved input through Crewroom while fencing the dead attempt and reconciling effects.
6. Fail storage/archive work and restart. Verify the actual database, cursor, receipt, and backup boundaries instead of inferring durability from a successful tool response.

The upstream repository contains idempotency, conflict, concurrency, and retention tests, but their presence is not evidence that our adapter or selected binary passes. [Upstream tests](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/35fe543a03d4ba981f5110826a1009b54d0551e0/crates/mcp-agent-mail-db/tests/idempotency_integration.rs#L258).

**Recommendation:** prefer Rust Agent Mail over Python Agent Mail for the next mailbox comparison, if evaluating an existing agent inbox product. Do not switch solely on source claims, and do not expect any mailbox replacement to remove Crewroom's durable workflow and action ledger. A small Crewroom-owned inbox may still have lower total integration cost for the accepted single-operator, single-host scope; that is an architectural judgment, not a benchmark result from this review.
