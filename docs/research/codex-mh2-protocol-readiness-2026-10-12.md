# Codex MH2 protocol readiness

Research for MH2 / FOR-7 on 2026-10-12, against Crewroom `686c26b852a614e2f5a68f1c899f3a9bd91a6137` and installed `codex-cli 0.162.0` (`x86_64-unknown-linux-musl`). **The protocol is concrete enough to implement a bounded adapter spike; safe execution, provider compatibility and recovery remain unverified.** This work did not implement the adapter, run inference, use account credentials, or change the active installation.


**Follow-up:** the [compatibility spike](codex-mh2-compatibility-spike-2026-10-12.md) now records real synthetic turns, native-tool denial, durable recovery and two live turns through the existing ChatGPT login. The operator selected existing harness access, superseding the gateway-first recommendation below. This document retains the earlier non-inference evidence and installed protocol mapping; its unverified findings describe that earlier evaluation.

## Evidence and reproduction

The executable resolves to `/home/crewroom/.local/share/mise/installs/codex/0.162.0/bin/codex`. Its installed package has binaries/resources, not an inspected Rust source checkout. The primary version-specific evidence here is generated JSON Schema, not assumptions about upstream main.

Executed successfully:

```sh
codex --version
codex app-server --help
codex app-server generate-json-schema --help
codex app-server generate-json-schema --out /tmp/crewroom-codex-mh2-eq5kutam/stable
codex app-server generate-json-schema --out /tmp/crewroom-codex-mh2-eq5kutam/experimental --experimental
codex features list
python3 /tmp/crewroom-codex-mh2-eq5kutam/probe.py
```

Both schema generations exited 0 with empty stderr. Temporary evidence remains in `/tmp/crewroom-codex-mh2-eq5kutam`: `probe.py`, `probe-result.json`, both schema bundles, and isolated `home`, `state` and `cwd` directories. These temporary paths are inspection aids, not durable test artifacts. Regenerate the schemas from the pinned executable before implementation. SHA-256 of experimental `v2/ThreadStartParams.json`: `80a40a7fac15b4bf70efb7f893fb353acc0a0d30c68f54aee4f01923deca85de`; `DynamicToolCallParams.json`: `401bba20cfbd95762bef0467d840430c46be53369093ad9f26425ba757e34efc`.

The [checked-in readiness receipt](../testing/mh2-readiness-2026-10-12.json) retains the sanitized probe result, exact isolated configuration, binary identity and schema/probe hashes after the temporary files expire.

Current official documentation confirms JSONL over stdio, initialization before other requests, and version-specific schema generation. Dynamic tools require experimental opt-in. Its general resume guidance describes restoring tools from rollout metadata; the installed resume schema has no `dynamicTools` field, so do not assume it accepts replacement tools. [Official app-server documentation](https://learn.chatgpt.com/docs/app-server).

## Observed non-inference exchange

The probe spawned three individually owned app-server processes with `--listen stdio:// --strict-config`. Each received an allowlisted environment containing `PATH`, temporary `HOME`, temporary `CODEX_HOME`, and a non-secret placeholder `CREWROOM_PROBE_TOKEN`. No real auth file, API key, parent configuration or repository working directory was supplied. The configured inference endpoint was loopback port 9; no `turn/start` or user prompt was sent. Stderr was not published. Each process was terminated and reaped by the probe.

| Case | Observed result |
| --- | --- |
| Initialize without experimental APIs, then start with dynamic tool | Initialization succeeded; `thread/start` rejected with `-32600`, `thread/start.dynamicTools requires experimentalApi capability`. |
| Initialize with experimental APIs, then same start | Succeeded; returned requested model `gpt-6.1-sol`, provider `crewroom_probe`, `approvalPolicy:"never"`, and sandbox `{type:"readOnly",networkAccess:false}`. This is configuration acceptance, not model access. |
| Immediate resume of that new thread | Failed `-32600`, `no rollout found for thread id …`. |
| Stop process, initialize fresh process using same temporary state, resume | Same missing-rollout failure. |

The successful start returned `ephemeral:false` and a rollout path. **Those fields did not establish a resumable session before a turn was run.** Persist the identity immediately, but distinguish allocated identity from verified durable history. The timing/durability boundary needs a real disposable turn; this probe does not establish why the rollout was absent.

Exact isolated configuration accepted by `--strict-config`:

```toml
model = "gpt-6.1-sol"
model_provider = "crewroom_probe"
web_search = "disabled"
[model_providers.crewroom_probe]
name = "Crewroom non-inference probe"
base_url = "http://127.0.0.1:9/v1"
env_key = "CREWROOM_PROBE_TOKEN"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
[features]
shell_tool = false
unified_exec = false
shell_snapshot = false
multi_agent = false
apps = false
plugins = false
hooks = false
code_mode_host = false
image_generation = false
browser_use = false
computer_use = false
view_image = false
```

This is a research configuration, **not a proven complete native-tool deny policy**. Acceptance does not demonstrate the outbound tool catalog, hook isolation, absence of helper descendants, or enforcement of every setting. In particular, this work has not identified and proven a complete `apply_patch` disable mechanism. A read-only sandbox and approval policy are not equivalent to routing every action through Crewroom.

## Installed wire contract

All schema filenames below are relative to the generated experimental directory.

| Concern | Version-specific contract and adapter implication |
| --- | --- |
| Initialize | `v1/InitializeParams.json`: required `clientInfo` with `name` and `version`; optional `title`; set `capabilities.experimentalApi:true`. Await the matching response, then send `{"method":"initialized"}`. Track RPC request IDs separately from domain IDs. |
| Start and frozen identity | `v2/ThreadStartParams.json`: set explicit `model`, `modelProvider`, `cwd`, instructions, sandbox/approval policy and tools. Explicitly set experimental `allowProviderModelFallback:false`. Start response contains effective model/provider/effort, cwd and sandbox: compare with the frozen assignment before admitting work. `thread.id` is the durable identity to retain; `thread.path` is marked unstable. |
| Assignment instructions | Start accepts `developerInstructions` and `baseInstructions`. Prefer injecting Crewroom's frozen assignment through developer instructions and initial input; replacing base instructions is a separate behavioral choice. Preserve role/assignment identity in Crewroom, not only prompt text. |
| Dynamic tool definitions | Start's `DynamicToolSpec` is a tagged union. Flat function form requires `type:"function"`, `name`, `description`, `inputSchema`; namespace form also exists. Use flat functions initially to match Crewroom's provider input policy. `deferLoading` is optional. |
| Tool call routing | `DynamicToolCallParams.json`: `arguments`, `callId`, `threadId`, `tool`, `turnId`, optional nullable `namespace`. `ServerRequest.json` identifies method `item/tool/call`. Respond to the **server request's RPC `id`**, not its `callId`. Validate tool name, arguments and assignment/thread/turn ownership before invoking existing host tools. |
| Tool result | `DynamicToolCallResponse.json` requires `success` and `contentItems`. Text content is `{type:"inputText",text:"…"}`; image/audio forms also exist. Crewroom's first adapter can keep the accepted text-only boundary. This is different from Responses API `input_text`. |
| Turn start/steering | `v2/TurnStartParams.json` requires `threadId` and `input`; text input uses `{type:"text",text:"…"}`. Optional `model`, `effort` and policy overrides can affect subsequent turns. `collaborationMode` can override model/effort/instructions, so do not admit unconstrained overrides. `turn/steer` requires `expectedTurnId`; use it for addressed busy steering. |
| Completion/failure | `v2/TurnCompletedNotification.json` includes thread ID and turn with ID/status. Status is `completed`, `interrupted`, `failed` or `inProgress`. `ErrorNotification.json` includes `willRetry`; a retrying error is not independently terminal. A final assistant message or `item/completed` is not the assignment's completion gate. |
| Interruption | `v2/TurnInterruptParams.json` requires both thread and turn IDs. An RPC success is an interrupt acknowledgment, not proof that the owned OS process and descendants have stopped. Keep Crewroom's existing stop/ownership reconciliation. |
| Resume | `v2/ThreadResumeParams.json` requires `threadId`; supports model/provider/cwd/policy overrides. Prefer ID-based resume. Path/history alternatives have unstable warnings and precedence rules. Recheck the effective configuration and reconcile history before delivery. Do not set `ephemeral:true` for recoverable assignments. |

Minimal dynamic-tool response, with illustrative IDs:

```json
{"id":42,"method":"item/tool/call","params":{"threadId":"thread-a","turnId":"turn-a","callId":"call-a","tool":"crewroom_read_file","arguments":{"path":"README.md"}}}
{"id":42,"result":{"success":true,"contentItems":[{"type":"inputText","text":"file contents"}]}}
```

Use streamed text deltas for display and final item/turn notifications for authoritative state. The documented dynamic-tool lifecycle is start item → server request → client result → completed item. Neither ordering nor the schemas promise exactly-once execution across restart. [Official event and dynamic-tool contract](https://learn.chatgpt.com/docs/app-server#dynamic-tool-calls-experimental).

## Gateway, authentication and tool confinement

**Recommendation pending the operator's authentication-route decision:** first prove a custom Responses provider through Crewroom's existing gateway. Retain its budget, model and opaque-history controls. Keep the gateway credential in a private child environment and use an isolated, persistent assignment state directory. New process attempts need that same session store; they must not borrow the interactive user's entire Codex home.

The official configuration supports custom provider `base_url`, `env_key`, `requires_openai_auth` and `supports_websockets`; `wire_api` supports Responses. Setting `supports_websockets:false` fits Crewroom's current HTTP/SSE gateway. The documented shell, multi-agent, apps and MCP switches govern different surfaces. MCP supports per-server tool allowlists and `required:true` startup failure, but is an alternative transport, not a substitute for host authorization. [Configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference).

Existing-login authentication is a separate route: it has not been tested against Crewroom's gateway, model/account restrictions, credential refresh, or budget accounting. Do not infer subscription entitlement from MH1's catalog or copy the user's authentication into a shared runtime directory. Whichever route is chosen needs a concrete credential/session ownership contract before live tests.

Repository constraints found during this review:

- [`provider_input.rs`](../../crates/crewroom-runtime/src/provider_input.rs) currently admits only top-level Responses `function` tools, selected input item types, and no `previous_response_id`, `conversation` or `prompt`. Codex's actual outbound request must be captured in an isolated local test. Native custom tools, namespace tools, tool search, compaction items or incremental history may require deliberate compatibility work; do not weaken the input gate speculatively.
- [`provider_gateway.rs`](../../crates/crewroom-runtime/src/provider_gateway.rs) admits the `/responses` route and compares frozen model/effort. Catalog and compaction endpoints, request compression, retries and opaque history need explicit checks against this policy.
- Installed `codex features list` reports broad native capabilities enabled in the ambient configuration, including shell, unified exec, multi-agent, hooks, plugins, apps, browser, computer and image tools. An isolated state/configuration is necessary. Generated start/resume response schemas explicitly describe `disabledPluginIds` as a saved list that **does not yet filter plugin capabilities**. It cannot be the isolation boundary.
- Native approval requests are different from dynamic tool calls. The schemas include command/file approvals, permission requests and MCP elicitation. Respond conservatively to unexpected requests; do not assume every native action must request approval. Prove that all mutating/model-access/network paths remain within Crewroom's authority.

## Disposable tests required before MH2 can pass

1. **Provider and confinement:** a locally captured request, then real bounded inference through the selected gateway route. Prove exact model/effort, allowed tool set, native-tool rejection, no ambient hooks/MCP/plugins, enforced budget termination, retry accounting and no direct upstream bypass. Exercise ordinary turns and context compaction. Discovery/initialization cannot pass this gate.
2. **Addressed tool roundtrip:** two separately supervised role processes call host tools. Bind each request to its assignment/thread/turn/call, reject cross-addressing and malformed arguments, return both success and failure, and verify the requesting model receives the correct result.
3. **Busy interruption and stop:** interrupt during streaming and during a host-tool call. Observe interrupted status, cancel/settle host work, terminate descendants, and verify ownership is actually stopped before releasing writer authority. Test terminal/process loss without a completion notification.
4. **Durable recovery and replay:** crash after model request, after tool admission, after side effect, after persisted result and before reply. Resume the saved thread from the same isolated store. Existing host replay uses attempt plus tool-call identity; persist a mapping that survives process attempts and reconciles original calls/results. A new RPC request ID is not a new logical operation. The protocol supplies no verified cross-attempt exactly-once guarantee; uncertain side effects require reconciliation before redelivery.
5. **Regression:** run existing OMP adapter/supervisor contracts unchanged, then required domain/build/user-flow suites for affected code. Preserve one dedicated externally supervised app-server process per coordinator/specialist; do not replace Crewroom roles or external mail with Codex internal subagents.

These are implementation acceptance gates, not findings of successful live delivery. The readiness decision should keep provider/confinement and durable tool replay as explicit early stop points.
