# T3 Code and Crewroom assessment

Inspected 2026-10-19. This assessment is not yet tracked in Linear. It is a static inspection of source and documentation. T3 Code dependencies were not installed, and its scripts, tests and builds were not run. No runtime was changed. One local check was made outside T3 Code: the installed Codex CLI (0.163.0) generated its app-server protocol schema, without model inference, to confirm that a probe check proposed below is feasible.

**Recommendation:** keep Crewroom's architecture. T3 Code is a single-user, chat-thread control surface for many harnesses. It has no ticket workflow, coordinator, separate specialist processes, external mailbox or revision-bound readiness. Its value to Crewroom lies in its harness-integration engineering. The three ideas to adopt first all feed FOR-69 (newer harness versions run after a compatibility probe passes): a protocol-schema diff for Codex, a bundled range policy that can mark versions as known-broken, and version-stamped recorded protocol transcripts. The code is MIT-licensed, but it is TypeScript/Effect running on Node and Electron. Reuse the ideas; copying code has no practical value.

## Source identity

T3 Code source: [`pingdotgg/t3code` at `1451e7cb23034d29774338608368a15d8845df36`](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36), shallow-cloned from `main` on the inspection date (commit dated 2026-10-18). License: [MIT, © 2026 T3 Tools Inc.](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/LICENSE) The repository was created on 2026-02-08. Current releases are nightly and preview pre-releases at `0.1.1-*` (for example `v0.1.1-nightly.20261019.3348`; see [releases](https://github.com/pingdotgg/t3code/releases)). The README says: "We are very very early in this project. Expect bugs." It also says that contributions are mostly not accepted. The project's [AGENTS.md](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/AGENTS.md) claims more than 500,000 users. That number was not verified.

Crewroom source inspected: `3b4cb0624c54943c56d1ad5d964f2794e0831d70` on `main`. The [product direction](../product.md), [architecture](../architecture.md) and [status](../status.md) remain authoritative.

## What T3 Code is

- **A Node WebSocket server that wraps provider CLIs and serves web, desktop (Electron) and mobile (React Native) clients.** The server owns provider processes, terminals, Git and project files. Clients control it over authenticated RPC, including remotely through Tailscale or a T3 relay. Sources: [README](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/README.md), [AGENTS.md](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/AGENTS.md), [architecture overview](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/overview.md), [remote access](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/user/remote-access.md).
- **Stack:** a TypeScript monorepo built on Effect, using pnpm and Vite+. It includes small native helpers: a Rust `sysinfo` resource monitor and `libghostty-vt` for terminals. Sources: [package.json](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/package.json), [native/](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/native).
- **Harnesses:** Codex, Claude Code, Cursor, Grok Build, OpenCode (1.x and 2.x), Pi, Antigravity, Muse, and generic ACP agents ([provider packages](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/packages)).
  - **Codex:** `codex app-server` over JSON-RPC, through a typed client whose schemas are generated from the upstream protocol at a pinned commit ([generator](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/packages/effect-codex-app-server/scripts/generate.ts), [client](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/packages/effect-codex-app-server/src/client.ts)).
  - **Claude Code:** through `@anthropic-ai/claude-agent-sdk` (`^0.3.276`) in-process, pointed at the user's `claude` executable ([server package.json](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/package.json), [ClaudeProvider.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/ClaudeProvider.ts)).
  - **Pi:** RPC mode ([provider-pi](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/packages/provider-pi/src/server)). OMP is not supported.
- **Domain:** project → thread → turn. An event-sourced v2 orchestrator commits events, projections, command receipts and outbox effects in one transaction, and an effect worker performs side effects afterwards ([overview: durable intent](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/overview.md#durable-intent-and-side-effects)). Checkpoints use hidden Git refs. Provider switching uses lossy textual handoffs ([context handoffs](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/context-handoffs.md)).
- **Source control:** GitHub, GitLab, Bitbucket, Azure DevOps, Forgejo and others, for PR linking and review ([source-control packages](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/packages)). There is no Jira integration and no ticket workflow.

## Comparison

| Concern | T3 Code | Crewroom |
| --- | --- | --- |
| Unit of work | Chat thread per project, operated by the user | Ticket run with coordinator, assignments and attempts |
| Agent processes | One provider session per thread. Subagents are provider-native | Coordinator and each specialist run as separately supervised OS processes, with external Rust Agent Mail |
| Durable state | Event log, projections and outbox in SQLite | Transactional SQLite workflow store, host/runner journals and Mail receipts, with reconciliation |
| Version policy | Advisory semver ranges (`supported`, `graceful`, `unsupported`, `broken`, `unknown`). Launch is not blocked | Exact verified versions for discovery and launch. FOR-69 adds "newer than verified, if the probe passes" |
| Completion | A turn settles, then checkpoint/PR follow-up | Readiness requires the exact revision, required checks and independent review |
| Network posture | Remote-first: LAN, Tailscale, relay, mobile | Loopback only, same-origin mutations, credentials server-side |

## Candidates, ranked by value and effort

| # | Item | Value | Effort | Reuse |
| --- | --- | --- | --- | --- |
| 1 | Codex protocol-schema diff as a FOR-69 probe check | High | S–M | Idea |
| 2 | Bundled version-range policy with a known-broken range | High | S | Idea |
| 3 | Version-stamped recorded protocol transcripts with replay gates | High | M–L | Idea |
| 4 | Typed capability record per harness version, with degradation policies | Medium | M | Idea |
| 5 | Subscription rate-limit windows (session and weekly headroom) | Medium | M | Idea |
| 6 | Version per attempt: re-probe at resume and replacement, not only at first launch | Medium | S | Idea |
| 7 | Per-model minimum harness version hint ("update unlocks this model") | Low–Medium | S | Idea |
| 8 | Terminal replay hygiene and history bounds | Low | S | Idea; verify need first |

### 1. Codex protocol-schema diff (FOR-69)

**T3 Code:** it generates typed schemas for every Codex app-server request, notification and server request from the upstream `app-server-protocol` crate at a pinned commit ([generate.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/packages/effect-codex-app-server/scripts/generate.ts), [generated output](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/packages/effect-codex-app-server/src/_generated)). Unknown notifications and server requests go to explicit fallback handlers ([client.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/packages/effect-codex-app-server/src/client.ts)).

**Crewroom problem:** FOR-69's probe must establish compatibility without model inference. A handshake alone does not detect a new server-request method or a newly required parameter. Crewroom's Codex runner rejects any server request other than `item/tool/call` and the credential refresh as "an unadmitted native operation" ([codex.rs](../../crates/crewroom-runtime/src/runner/codex.rs)). That fails closed, which is correct. However, a new request kind would only surface mid-turn on a live attempt.

**Fit:** the installed binary produces the same information offline. `codex app-server generate-json-schema --out <dir>` is an experimental subcommand. On Codex 0.163.0 it wrote 39 schema files without inference: `ServerRequest.json` lists 10 methods and `ServerNotification.json` lists 85. The probe can compare these against a baseline stored for the verified version:

- Fail on any new `ServerRequest` method.
- Fail when a request Crewroom sends gains a required field.
- Fail when a notification field Crewroom reads is removed or retyped.
- Report added notifications without failing.

Store the schema digest in the probe receipt. Handle the experimental status explicitly: if the subcommand is missing or fails, the probe fails and names this check. Effort: S–M.

### 2. Bundled range policy that can mark versions known-broken (FOR-69)

**T3 Code:** a policy maps harness version ranges to `supported`, `graceful`, `unsupported` or `broken`. It also names a recommended range or version, and is scoped to the T3 Code version range it applies to ([providerCompatibility.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/providerCompatibility.ts), [model-manifest.json `compatibility`](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/model-manifest.json)). The policy is evaluated for both the installed version and the latest available version, so the UI does not recommend updating into a broken release. The bundled policy uses open-ended upper bounds, for example Codex `>=0.156.0` and Claude Code `>=2.1.280`. This is the same stance the operator chose for FOR-69.

**Crewroom problem:** under FOR-69, a probe pass admits any newer version. Without a policy like this, Crewroom cannot withdraw a specific release that passes the probe but is known to misbehave live, other than by shipping code.

**Fit:** add a bundled, revision-controlled list of refused ranges with reasons. Evaluate it before the probe, and record the verdict in the receipt. Do not adopt two T3 behaviours. Its policy is advisory, so a `broken` version still runs; Crewroom must refuse. Its policy can also be replaced at runtime by a manifest fetched from `main`, which conflicts with revision-bound admission (see "Do not adopt"). Effort: S.

### 3. Version-stamped recorded protocol transcripts

**T3 Code:** each scenario has one NDJSON transcript per provider, recorded against the real harness. Each transcript has a header naming the provider, protocol and harness version, for example Codex `0.156.1` and Claude Agent SDK `0.2.111`. Each entry is either an `expect_outbound` frame, an `emit_inbound` frame or a `runtime_exit`. There are 93 scenarios, covering restart resume, interrupt, steering, rollback and subagents ([fixtures](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/orchestration-v2/testkit/fixtures), [replay format](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/packages/effect-codex-app-server/src/replay.ts)). Labelled gates hold the replay at chosen frames so a test can inject a crash or a steering message at an exact boundary ([replayGate.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/packages/provider-testing/src/replayGate.ts)). Recording scripts regenerate the transcripts ([Claude recorder](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/scripts/record-claude-agent-sdk-replay-fixture.ts)).

**Crewroom problem:** Crewroom's runner tests use hand-written doubles ([claude-rpc.py](../../crates/crewroom-runtime/tests/fixtures/claude-rpc.py), and inline JSON frames in the `codex.rs` tests). They encode what the authors expected rather than what a given harness version emits. When a new version is accepted under FOR-69, nothing compares its real frame sequence against the verified version's.

**Fit:** record a small set of transcripts against each verified version. The scenarios should be the recovery boundaries Crewroom already tests: first-turn interruption, busy steering, tool-call admission, terminal-before-queued input, and resume after restart. Replay them in Rust tests, and re-record them on a candidate version to diff the semantics. Recording uses live inference, so it is an evidence step attached to a version, not part of the no-inference probe. Replayed results remain simulated checks and must be labelled separately from live evidence. Redact account identifiers before committing transcripts (public repository). Effort: M–L.

### 4. Typed capability record per harness version

**T3 Code:** each adapter emits capabilities at session start. Examples are `supportsActiveSteering`, `supportsSteeringByInterruptRestart`, `terminalStatusQuality: strong|weak|none`, native ID strength, `pendingRequestsSurviveRestart` and `providerCanRollbackConversation`. Each missing capability has a declared degradation policy ([provider capability system](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/orchestration-v2/provider-capability-system.md)).

**Crewroom problem:** the capability limits of each native route are recorded in prose ([MH2](../testing/mh2-runtime-acceptance.md), [MH3](../testing/mh3-runtime-acceptance.md)). FOR-69 needs a probe receipt that states what a version supports. A typed record would let the receipt name exactly which capability changed between versions, and let admission check it.

**Fit:** this is compatible with the separate-process contract. Keep it a description of measured behaviour, never a self-report from the harness. Effort: M.

### 5. Subscription rate-limit windows

**T3 Code:** for Codex it reads `account/rateLimits/read` and listens for `account/rateLimits/updated` ([CodexProvider.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/CodexProvider.ts), [codexUsageLimits.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/codexUsageLimits.ts)). For Claude it reads the five-hour and seven-day windows through the SDK's usage call, which is marked experimental, and through streamed rate-limit events ([claudeUsageLimits.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/claudeUsageLimits.ts)).

**Crewroom problem:** Crewroom records token usage for each response, and marks cost as estimated or unavailable. It does not show how much of the subscription window remains before Run, or whether an attempt stopped because a window was exhausted.

**Fit:** show this as advisory, actual-or-unavailable information beside usage. It must never become an enforced budget, and missing data must never read as zero, in line with the product rule on accounting. Treat the Claude source as unstable. Effort: M.

### 6. Re-probe at resume and replacement

**T3 Code:** installed Antigravity versions are immutable, and an atomic pointer selects the version for new processes. Running processes hold leases, so an update never replaces an executable under a running agent ([provider constraints](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/providers.md#process-and-account-isolation)). After an update, the provider's version is read again; this catches an updater that exits 0 without updating ([same document, provider updates](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/providers.md#provider-updates-run-only-through-the-owning-installer)).

**Crewroom problem:** FOR-69 re-probes when the binary changes between admission and launch. Harnesses update themselves, so the binary can also change during a long run. Resuming a session or launching a replacement attempt would then use a different version from the admitted one.

**Fit:** record the version and probe receipt for each attempt. Apply the same change-detection and re-probe rule at resume and replacement. Crewroom already compares the binary's SHA-256 across discovery ([discovery.rs](../../crates/crewroom-runtime/src/model_catalog/discovery.rs)), so this extends an existing check; it is not a new mechanism. Effort: S.

### 7. Per-model minimum harness version hint

**T3 Code:** manifest entries carry `adapter.codex.minVersion` and `adapter.claudeCode.minVersion`. Snapshots list `updateRequiredModels`, and the picker tells the user which update unlocks a model ([model manifest](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/model-manifest.md), [ClaudeModelCatalog.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/ClaudeModelCatalog.ts)).

**Crewroom problem:** when a role's model is missing from the installed harness's catalogue, the operator gets no explanation.

**Fit:** a bundled hint is sufficient; do not add a remote fetch. Effort: S.

### 8. Terminal replay hygiene

**T3 Code:** server history is capped at 5,000 lines and 8 MiB per terminal, and eviction never splits a code point. Terminal query and reply traffic is stripped from retained history, and the renderer detaches its PTY writer during replay, so historical device queries do not produce junk at the prompt ([terminal runtime](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/terminal-runtime.md)).

**Crewroom problem:** this would matter only if the agent wall replays raw PTY bytes. Crewroom already renders through `gpui-libghostty`. Check its replay path before doing any work here. Effort: S.

## Do not adopt

- **The codebase or runtime** (TypeScript, Effect, Node, Electron, React Native, and the Claude Agent SDK in-process). The architecture says the packaged app and service need no Node runtime, and Crewroom already controls Claude Code directly in Rust. The MIT license permits reuse, but none of this code can be used in Crewroom.
- **Remote access, the T3 Connect relay, mobile pairing and outside agents over MCP.** These conflict with loopback binding, same-origin mutations and keeping credentials server-side.
- **A manifest and compatibility policy fetched remotely from `main` between releases** ([model manifest](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/model-manifest.md)). This changes admission behaviour without a reviewed revision, which breaks the revision-bound evidence contract. Keep the policy bundled.
- **Advisory-only version status.** T3 Code shows a warning and still runs a `broken` version. Crewroom's contract refuses with the failing check named; FOR-69 keeps that.
- **The orchestrator MCP server that lets agents create and steer threads** ([orchestrator MCP server](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/orchestration-v2/orchestrator-mcp-server.md)). It would create a second workflow authority, and agent messages would bypass the external Mail route.
- **Lossy portable handoffs (240-character item summaries) as the context for continuation or recovery.** Crewroom's replacement attempts carry the full saved inputs, steering, confirmed mail and evidence. A lossy summary would weaken that.
- **Native conversation fork and rollback, and checkpoint revert.** These conflict with Crewroom's conservative native-history recovery and its preserved attempt history. Crewroom already isolates work in owned worktrees and `refs/crewroom/...` refs.
- **Automatic harness updates through package managers.** These mutate the operator's tools; FOR-69 probes whatever is installed. At most, show the update command.
- **Thread auto-settle and the PR sync that runs after an agent merges.** In Crewroom, agents never merge; the human reviews and merges.
- **The event-sourced orchestrator with an outbox.** Crewroom already persists intent before effects and reconciles PR and Jira actions ([Pi Durable assessment](pi-durable-assessment-2026-10-17.md)). Rebuilding this would add nothing.
- **Usage totals scanned from the harnesses' global local history** ([usage](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/user/usage.md)). This reads state outside Crewroom's ownership, and it cannot attribute usage to an attempt.

## Relevance to FOR-69 (probing newer harness versions)

- **Codex:** item 1 is the most useful no-inference check T3 Code points to. T3 Code itself pins the protocol at build time from an upstream commit. Crewroom can instead read the installed binary's own schema at probe time. T3 Code also opts out of notifications it does not use (`optOutNotificationMethods: ["turn/diff/updated"]`, [CodexAdapterV2.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts#L228)), which reduces exposure to protocol change. The probe should also confirm that the `initialize` capabilities Crewroom sends are still accepted.
- **Claude Code:** T3 Code's capability probe passes a prompt iterator that never yields, so initialization completes and returns account details and slash commands without any API request. It disables hooks, uses strict empty MCP configuration, and does not persist the session ([ClaudeProvider.ts](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/apps/server/src/provider/ClaudeProvider.ts)). Crewroom's discovery already does the equivalent, with stronger isolation: `--bare`, empty setting sources, `--no-session-persistence`, disabled hooks, a private `HOME`, and a check that the binary was unchanged ([discovery.rs](../../crates/crewroom-runtime/src/model_catalog/discovery.rs)). Two things are new:
  - T3 Code also sets `ENABLE_CLAUDEAI_MCP_SERVERS=false`, `CLAUDE_CODE_AUTO_CONNECT_IDE=0` and `CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL=1`. Check whether `--bare` and the private `HOME` already cover these.
  - The principle that "setup must not happen as a health-check side effect" ([provider constraints](https://github.com/pingdotgg/t3code/blob/1451e7cb23034d29774338608368a15d8845df36/docs/internals/providers.md#setup-must-not-happen-as-a-health-check-side-effect)) should be written into the probe contract.

  Claude Code has no schema generator, so its protocol drift is best caught by item 3's recorded transcripts. The probe can still check the shape of the `initialize` control response and the control-request subtypes Crewroom handles.
- **OMP:** T3 Code does not support OMP. Its Pi RPC driver ([provider-pi](https://github.com/pingdotgg/t3code/tree/1451e7cb23034d29774338608368a15d8845df36/packages/provider-pi/src/server)) reads `get_state` for defaults. It has no version handshake that Crewroom lacks.
- **Policy:** T3 Code's open-ended ranges are evidence that a widely used client runs newer Codex and Claude Code versions without exact pins. T3 Code pairs them with a quick retraction path (a remotely updated `broken` range). Crewroom's equivalent is item 2: a bundled refusal list evaluated before the probe. Item 6 extends the receipt to resume and replacement.

## Validation boundary

This work read T3 Code source and documentation at the commit above, and Crewroom source at `3b4cb06`. T3 Code tests, replays and builds were not run, so its fixtures and policies are recorded as design evidence, not passing results. The one executed command was `codex app-server generate-json-schema` on the locally installed Codex CLI 0.163.0, to confirm the feasibility of item 1. It made no model call and changed no Crewroom state. No adapter, probe or roadmap change is accepted by this research. It does not change accepted product or architecture decisions.
