# FOR-8: Claude Code streaming and control protocol

Research on 2026-10-13 for MH3. Initial work inspected version/help, the installed executable and official SDK source. The later authentication experiment below sends fixture prompts to a synthetic provider inside an isolated network namespace. No real inference request was sent, operator credentials were read or copied, user configuration was changed, or active installation was modified. This is bounded protocol/authentication evidence, not full adapter acceptance.

## Source identity and integration choice

- Installed native executable: `/home/crewroom/.local/share/claude/versions/2.1.296`, reached through `/home/crewroom/.local/bin/claude`; reports `2.1.296 (Claude Code)`.
- Executable SHA-256: `24972e3bc859fab2b46ed4c1e51f7d6130f06d3bd550811a114640de3370d0de`.
- Official Python SDK inspected at [`b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c`](https://github.com/anthropics/claude-agent-sdk-python/tree/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c), package version `0.2.165`. Its subprocess transport and control types provide concrete reference behavior.

The supported SDK architecture runs one native `claude` subprocess per session, communicating over stdio. A Rust adapter can implement that same exchange without introducing a Python or Node bridge. **That is an implementation choice, not a claim that the private wire protocol has an independent compatibility guarantee.** Pin the tested CLI contract and reject incompatible startup responses. An official SDK bridge offers maintained protocol handling but introduces another runtime and still requires Crewroom's ownership, tool authorization and recovery ledger. [Official hosting architecture](https://code.claude.com/docs/en/agent-sdk/hosting), [SDK subprocess construction](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/transport/subprocess_cli.py#L569).

## Launch and initialization

The installed help exposes `-p`, both `stream-json` formats, `--verbose`, `--tools ""`, `--strict-mcp-config`, `--mcp-config`, `--setting-sources ""`, `--replay-user-messages`, `--session-id`, `--resume`, and `--safe-mode`. Empty tool arguments remove the built-in tool set. `--bare` explicitly disables OAuth/keychain authentication, so it is unsuitable for the accepted native-login route. `--safe-mode` retains authentication but disables customizations; test explicit SDK MCP servers and SDK hooks under the exact launch combination. Do not carry MH1's `disableAllHooks:true` into runtime without proving the required SDK gate still executes. [CLI reference](https://code.claude.com/docs/en/cli-reference), installed `claude --help`.

SDK-hosted tool configuration uses this shape; the SDK removes its in-process server instance before serializing it:

```json
{"mcpServers":{"crewroom":{"type":"sdk","name":"crewroom"}}}
```

The host must keep stdin open and service incoming control requests while awaiting its own initialization response. [MCP argument serialization](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/transport/subprocess_cli.py#L666).

```json
{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize","hooks":{"PreToolUse":[{"hookCallbackIds":["crewroom-pre-tool"]}]}}}
```

The reply is `type:control_response`, with nested `response.subtype`, `response.request_id`, and `response.response`. Treat initialization metadata as potentially private; allowlist fields rather than persisting the whole response. Preserve requested model selector separately from any observed model identity. [Initialization builder](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L309), [control types](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/types.py#L2790).

## Messages, interruption and recovery

User input is a newline-terminated frame:

```json
{"type":"user","uuid":"<message-uuid>","session_id":"<session-uuid>","parent_tool_use_id":null,"client_composed":true,"message":{"role":"user","content":"<assignment or steering>"}}
```

The SDK sends this basic user envelope. Its `verbatim_prompts` option adds `client_composed:true`, suppressing slash-command dispatch, file-mention expansion and the normal attachment pass. Use this for host-composed assignments and steering. `--disable-slash-commands` alone is described as disabling skills. [User frame](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/client.py#L273), [verbatim semantics](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/types.py#L2569).

`--replay-user-messages` supplies an input acknowledgment. This research does **not** establish that receiving it means the JSONL transcript has been durably flushed. Recovery needs revision-bound tests for acknowledgment, transcript persistence and host delivery receipts. [CLI reference](https://code.claude.com/docs/en/cli-reference).

Interrupt uses a correlated control request with `request:{"subtype":"interrupt"}`. The SDK does not terminate the process for this operation. A response acknowledges the control operation; Crewroom must separately preserve interruption intent and confirm stopped process ownership when replacing an agent. Incoming `control_cancel_request` names the original request ID; the SDK cancels that handler and suppresses a late reply. [Interrupt](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L792), [cancellation](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L420).

Resume with the explicit recorded UUID, passing the frozen MCP, model, settings and permission arguments again. Do not use latest-session continuation for independent assignments. A stopped mid-tool transcript can contain a call without a result; Claude's restored narrative is not evidence that the external effect failed. Disable `CLAUDE_CODE_RESUME_INTERRUPTED_TURN` explicitly until Crewroom reconciles receipts. [Resume behavior](https://code.claude.com/docs/en/sessions), [resume environment variables](https://code.claude.com/docs/en/env-vars).

The official `get_session_messages` function is a filesystem helper: it reads JSONL, reconstructs `parentUuid` links and returns visible user/assistant messages. No corresponding public CLI control request was established. A Rust reader can inspect only the owned transcript with bounded input and validate session/message/tool IDs; a raw line count or arbitrary matching text is insufficient. SDK-visible history is also not necessarily the entire pre-compaction record. [Reader implementation](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/sessions.py#L1059), [history after compaction](https://code.claude.com/docs/en/agent-sdk/session-storage).

## MCP routing and gates

The CLI wraps MCP JSON-RPC in a control request:

```json
{"type":"control_request","request_id":"outer-1","request":{"subtype":"mcp_message","server_name":"crewroom","message":{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"<tool>","arguments":{}}}}}
```

The matching host response nests the MCP response:

```json
{"type":"control_response","response":{"subtype":"success","request_id":"outer-1","response":{"mcp_response":{"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"<result>"}]}}}}}
```

Outer control IDs and inner MCP IDs are distinct. For a notification, the official implementation acknowledges the wrapper using `mcp_response:{"jsonrpc":"2.0","result":{}}`. It can process controls concurrently. Preserve correlations and keep the reader live while an external tool is executing. [SDK MCP routing and wrapper](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L656), [MCP bridge](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/sdk_mcp_bridge.py).

`allowedTools` pre-approves tools; it does not restrict tool availability. Earlier allow rules and permission modes can bypass `canUseTool`. `dontAsk` denies calls that would prompt without invoking that callback. The documented gate that runs before other tool-permission steps is `PreToolUse`; a hook denial applies even in bypass mode. [Permission ordering](https://code.claude.com/docs/en/agent-sdk/permissions).

A hook control request carries `callback_id`, `input` and `tool_use_id`. Reply with `response.response` containing:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"<reason>"}}
```

The hook input includes the tool name, input and session identity. Register an unfiltered matcher for all tools; reject unexpected names and identities. The host's MCP dispatcher must also authorize every call against the frozen assignment and durable effect ledger: no callback timing assumption should grant an external effect. Unknown tools, malformed IDs, changed arguments and missing authorizations must fail closed. [Hook inputs and outputs](https://code.claude.com/docs/en/agent-sdk/hooks), [hook dispatch](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L640), [Crewroom architecture](../architecture.md).

## Native authentication and assignment state

Official authentication documentation binds Linux credentials to `<CLAUDE_CONFIG_DIR>/.credentials.json`; macOS Keychain identity is also scoped by that directory. Therefore a fresh assignment config directory cannot reuse the native login merely by inheriting `HOME`. The native CLI should own refresh; do not snapshot refresh tokens into independent assignment directories. [Credential management](https://code.claude.com/docs/en/authentication).

Settings sources can be disabled while supplying explicit settings; managed policy still has higher priority. This is configuration isolation, not an OS sandbox. [Configuration precedence](https://code.claude.com/docs/en/agent-sdk/configuration).

Session files normally live under `<config>/projects/<encoded-cwd>/<session-id>.jsonl`. The SDK sessions documentation also describes `CLAUDE_CODE_PROJECT_DIR_NAME` alongside explicit `CLAUDE_CONFIG_DIR` to select the project namespace. Installed code accepts 1–64 ASCII letters/digits/underscore/hyphen, excluding reserved Windows device names, and only enables this override when `CLAUDE_CONFIG_DIR` is set. A shared native config directory plus a unique project namespace is a candidate that preserves native refresh coordination without credential copying; prove the path and permissions on the installed CLI. [Session storage](https://code.claude.com/docs/en/agent-sdk/sessions), installed executable byte offset `207256615`.

There is a metadata-path complication: installed global-config construction uses `<CLAUDE_CONFIG_DIR>/.claude.json` when the variable is explicitly set, versus `~/.claude.json` when unset; a legacy `.config.json` fallback also exists. Thus explicitly setting the default `~/.claude` directory preserves the credential location but can select different account metadata. Account binding must use the correct native source and fail closed on mismatch or missing identity. This is source inspection, not a claim that either file exists locally. Installed executable byte offset `207518863`.

Native refresh implementation observed in the shipped binary:

| Code location | Observed behavior |
| --- | --- |
| `RBn`, byte `210434636` | Config-directory `.oauth_refresh.lock`, `realpath:false`, 60-second stale threshold, 5-second update interval. |
| `zEs`, byte `210435068` | Acquires that lock and a legacy lock named from the config directory's real path plus `.lock`; records owner identity and releases both. |
| `fyo`, byte `210436403` | Retries contention up to five attempts, invalidates cached authentication and re-reads credentials under the lock before invoking refresh. |
| Credential storage, byte `209495829` | Credential filename is `.credentials.json` under the native config directory; successful file writes request mode `0600`. |

These are evidence from the version/hash above, not a public locking API. The later experiment below checks concurrency with fake credentials. Credential symlinks across distinct config directories would not unify the observed directory-based lock identities. Storage code also has a `refused-symlink` result, so symlink support must not be assumed. A shared native directory is the narrower source-supported approach; keep assignment prompts, callbacks, receipts and working files private and restrict transcript reads to the recorded session.

The official `SessionStore` feature mirrors local transcripts; restoring from it may seed a temporary config directory by copying credentials. It is not evidence that multiple copied refresh-token owners are coordinated. Avoid adopting that behavior implicitly for Crewroom's native-login path. [Session-store behavior](https://code.claude.com/docs/en/agent-sdk/session-storage).

## Result, usage and remaining validation

Check `is_error` as well as subtype: official SDK source explicitly handles API failures arriving as `subtype:"success", is_error:true`. A result ends a user turn; it is not necessarily process exit or proof all background work has stopped. [Error/result handling](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L88), [turn/run distinction](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L454).

For usage, assistant messages sharing the same `message.id` must be deduplicated. In streaming mode, result `usage` covers that turn's main loop; `modelUsage` and `total_cost_usd` accumulate across turns and include restored session spend. Use cumulative deltas across resume, never sum successive totals. Cost is an estimate, not authoritative billing or a subscription charge. Missing counts remain unavailable, and crash totals can be zeroed. No hard native-login budget guarantee follows from these fields. [Usage semantics](https://code.claude.com/docs/en/agent-sdk/cost-tracking).

Required FOR-8 evidence remains: actual supported model inference; every-call authorization including auto-approved and denied calls; explicit SDK hooks under safe mode; addressed tool/mail delivery; independent processes; busy interruption; stopped-owner confirmation; same-session recovery; missing or corrupt transcript holds; duplicate-effect suppression; native auth account binding and refresh ownership; alias versus observed model identity. Initialization and source inspection alone pass none of those runtime acceptance gates. [MH3 acceptance](../multi-harness-plan.md#ordered-work-and-acceptance).

## Follow-up: measured isolated authentication

[`scripts/probe_claude_auth.py`](../../scripts/probe_claude_auth.py) runs the installed executable in a fresh Linux user/network namespace, verifies that the only interface is loopback, and uses a local TLS fixture to answer native HTTPS OAuth and model requests. Each case creates an exclusive private `HOME`, fake `.claude.json` account metadata and fake `.claude/.credentials.json` tokens. `CLAUDE_CONFIG_DIR` stays unset. The child environment is explicitly constructed; no provider credentials, token helpers or real user settings are inherited. The TLS key and fake credentials are confined to the disposable output directory. No real authentication or inference service is reachable.

```sh
python3 scripts/probe_claude_auth.py \
  --claude /home/crewroom/.local/share/claude/versions/2.1.296 \
  --output .crewroom/mh3-probe/auth-final --case all
```

The output directory must not already exist. The script exits nonzero if a case's explicit assertions fail, records its own source hash and the native executable hash, and stops/reaps every native child. Runtime dependencies for this Linux probe are Python 3, `unshare`, `ip` and `openssl`; it does not install anything. See the [retained receipt](../testing/mh3-claude-auth-2026-10-13.json).

| Case | Measured result |
| --- | --- |
| Missing login | Exit 1, `is_error:true`, no OAuth refresh and no model request. The result subtype is still `success`. |
| Expired login rejected by fixture | One refresh attempt receives synthetic `invalid_grant`; exit 1 and error result, with no model request. |
| Two concurrent native processes | Hold the first refresh response while the second starts. The native lock exists and no second refresh begins. Release the response: one successful rotation, both processes finish synthetic model turns using the fresh bearer. |
| Native owner dies before rotation | SIGKILL/reap the lock owner while the fixture holds its request. The fixture detects disconnection and performs no rotation for that attempt. The waiting process recovers the native lock, performs one successful replacement refresh and completes a model turn; no lock remains. |
| Launcher dies while native owner lives | SIGKILL only an intermediate launcher. Native Claude survives and retains the lock; the second process does not start another refresh. Release the first response: both native processes finish, with one rotation in total. |

Every credential file checked remained mode `0600`. The fixture rejects reuse of its prior refresh token after a successful rotation, so the concurrency case would fail if a contender independently reused the stale token. The owner-death case deliberately covers **death before remote rotation**. It does not establish recovery when a real OAuth server has rotated a token but the owner dies before persisting the response. These synthetic checks also do not establish real refresh-token lifetime, server policy, macOS Keychain behavior, different installed CLI versions, or long-running reliability. The parent implementation separately owns real-login inference/refresh evidence.

The launcher-death result reinforces Crewroom's process contract: death of a supervising process is not evidence the native agent has stopped. Keep ownership of the actual native process and its group until it is confirmed reaped.

## Follow-up: queued input and interrupt receipts

The installed executable's protocol schema at byte offsets `208291524`, `208370300`, `208429180` and `208437900` adds the following semantics beyond the simpler official Python `interrupt()` wrapper. These are **source-inspected**, not newly executed by this researcher; feature-detect the capability strings from `system/init` and test the adapter's chosen combination.

| Capability/request | Contract in the installed schema |
| --- | --- |
| `interrupt_receipt_v1`; plain `interrupt` | Queued UUID-stamped main-thread commands survive. The successful control response includes `still_queued`, captured synchronously with abort processing. |
| `interrupt_cancel_queued_v1`; `cancel_queued:true` | Queued and pending-dispatch commands are removed or marked cancel-pending, reported in `cancelled`, and emit terminal `command_lifecycle` cancellations. Native-local `still_queued` becomes empty for those UUID-stamped commands. |
| `interrupt_send_now_v1`; `send_now:true` | Makes room for waiting user input without cancelling the queue or latching a stop on a later turn. Optional `message_uuid` targets an exact message string. `cancel_queued:true` overrides this behavior. |

The `send_now` receipt value is `stopped`, `delivering` or `nothing_waiting`. `delivering` does not prove delivery: it can mean pending admission, a message being folded into the running turn, a wait moving to the background, a turn already stopping, or a process exiting. The turn's result, rather than the receipt, establishes that turn's end. Without the advertised capability an older CLI can treat this request as a plain interrupt.

`command_lifecycle` has `command_uuid`, a separate event `uuid`, `session_id`, and state `queued`, `started`, `completed`, `cancelled`, `discarded` or `refused`. Queued means admitted; started means drained into a turn. A fresh turn's completed event follows its result; a folded input's completed event precedes that result. There is no strict one-event/one-result pairing. Cancelled can mean explicit removal **or** consumption by a turn later interrupted/failed. Correlate the host's own cancellation requests and interrupt `cancelled` receipt before deciding whether a command needs recovery. Never blindly resend every cancelled UUID.

The schema documents limits: a thrown failure can leave started without a terminal event; some queue admission guards can leave queued without one; internal commands can have UUIDs unknown to the host; completed after certain turn limits or deferrals can leave content unanswered until continuation. Process-exit settlement must therefore reconcile Crewroom's recorded inputs and transcript/effect receipts. A post-result queue query is too late to recover the interrupt snapshot because draining can immediately begin the next turn. This research does not establish a stronger exactly-once input or effect guarantee.

## Follow-up: multiple sends in one turn

The pinned native implementation has two distinct ways to consume multiple host inputs in one model turn. These findings come from the executable identified above; byte offsets identify the exact inspected implementation, and are not a compatibility promise.

| Path | Correlation and lifecycle behavior | Source byte offsets |
| --- | --- | --- |
| Initial queue batch | Compatible prompt commands are combined before execution. Text is joined in member order. The representative command takes the **last** member's UUID. The outer headless driver emits `started` for **every** batch UUID in member order before running the turn, and settles every member afterward. | `Lu`: `233158456`; `ep.startBatch`: `233156176`; intake `start`: `233216501`; batch settlement: `233225823` |
| Input folded while busy | At a boundary between tool rounds, deliverable commands are consumed with reason `absorbed_mid_turn`; each UUID emits `started` while the original turn is still running. Their terminal lifecycle events precede the consuming turn's result. | `224279994`, `224280286`, `224209604` |
| Dropped candidate during fold | The loop can emit `started` immediately followed by `completed` for a dropped candidate. A started event alone therefore does not prove the input reached a model request. | `224278481` |

The correlation tracker is `KS` at byte `233260033`. The initial batch seeds it through `Fk` at `233292820`. Mid-turn inputs enter it only after a `queued_command` attachment with `commandMode:"prompt"`, non-meta content and `source_uuid` is followed by the corresponding started event. The headless loop records those events at `233452580`.

`user_message_uuid` identifies the model turn's primary input, not necessarily the first submitted input. A normal typed-prompt turn keeps its opening representative UUID even when later input folds into it. A synthetic/meta turn can change its primary to the latest folded user input. `user_message_uuids` lists tracked consumed client inputs in consumption order: initial batch members, followed by qualifying folds. Every emitted list contains the primary. The final result uses the current tracker, so its list can be longer than the first assistant frame's list. Folded lifecycle completion does not clear that tracker. Source: assistant schema at `208223100`/`208225185`, result schema at `208279815`, result assembly at `233454384`.

**The tracker is capped at 64 UUIDs.** For an ordinary typed turn it keeps the first 64 consumed IDs, ensuring that the primary is included. A turn without an ordinary primary can replace the last slot with newer IDs. Thus the result list is not an unlimited receipt for all started commands. Bound outstanding consumption per turn or hold overflow for reconciliation. `user_message_uuids` can also be absent on delivery failures, zeroed session-level failures and older producers; falling back to the primary cannot settle a known multi-input batch. Source: `KS` and result schema above.

The primary/list pair is stamped on the first top-level assistant frame and independently on the first non-ping stream event. For a typed turn, later frames normally omit it even if the consumed list has grown. The loop stamps again if the primary changes. A `system/status` frame with `status:"requesting"` can carry the current list without a primary. Source: `233457449`.

Implementation implication, derived from the inspected paths: keep a stable **local** turn identifier anchored to the first known host-input started event, plus a set of the host input UUIDs associated with that turn. Subsequent started events need not open a new turn. Validate any model primary/list against known host sends and the current group, and reconcile all listed members at result rather than only the primary. This local anchor can own the tools/effects for the combined turn; it does not claim that a tool was caused by one particular member. Keep member state through result even if a folded member already completed, then retain terminal mappings for the initial batch's post-result lifecycle events. Reject or separately classify unknown/internal UUIDs. The result list supplies correlation, not proof that each instruction was answered, persisted or successfully executed.

## Follow-up: resume branch and compaction

A matching tool receipt anywhere in the raw JSONL does **not** establish that native `--resume` will load that tool into the active conversation. The pinned loader honors `last-prompt` checkpoints containing `leafUuid`; an `explicit:true` checkpoint can select an older branch, and an explicit null leaf can represent a conversation cleared to empty. It then selects a main-chain leaf and reconstructs its ancestry. Source: checkpoint writer `SEt` at `220195016`; `JFn` parser/leaf selection at `220284016`–`220292815`; resume materialization `wLs` at `220299325`.

The public Python `get_session_messages` reader indexes transcript entries, chooses a recent main-chain leaf and follows `parentUuid`. It deliberately does not follow `logicalParentUuid`, and includes the compact summary. This supports the distinction between raw history and active context. **It is not full parity with this native loader:** the inspected Python reader does not implement native `last-prompt` checkpoint selection or the native compaction-preservation relinking described below. [Pinned Python reader](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/sessions.py#L931). Official documentation also states that earlier turns become a summary after compaction. [Session storage and post-compaction history](https://code.claude.com/docs/en/agent-sdk/session-storage#getsessionmessages-returns-the-post-compaction-chain).

The native writer treats a `system/compact_boundary` specially: `parentUuid` becomes null and `logicalParentUuid` records the prior chain position. Subsequent summary/context rows continue from the new boundary. That null parent is valid compaction, not a broken chain. Source: `insertMessageChain` at `220182854`; boundary constructor `cG` at `220103667`.

Compaction has more than one supported shape. Ordinary compaction lets the loader skip pre-boundary rows, and on-disk compaction can physically remove them. A boundary with `compactMetadata.preservedMessages` names an anchor and retained UUID list; the older `preservedSegment` shape names head, tail and anchor. `U2r` rewrites these retained rows into a chain after the anchor, rewrites affected child links, and removes other earlier rows before native resume. Source: streaming scanner `GFn` at `220270484`; relinker `U2r`/`j2r` at `220223707`; file compactor `UZr` at `214440000`.

Native reconstruction also restores sibling assistant rows sharing a provider `message.id` and corresponding tool-result rows; parallel tool results can share an assistant parent rather than forming a simple linear list. It can use `sourceToolAssistantUUID` and tool-use IDs to recover missing results. On a missing/cyclic parent, the native chain walker even has a timestamp-based fallback within five seconds. A conservative Crewroom verifier should not silently imitate that fallback as proof of ancestry. Source: `EEt`/`z2r`/`q2r` at `220225701`, `220227884`, `220229126`; writer tool-result parent selection at `220182573`.

Source-derived bounded validation guidance:

1. Identify the active main-chain leaf using the applicable checkpoint and row order, excluding sidechains. Treat conflicting branches, invalid checkpoints, cycles and missing required parents as a recovery hold. Receipt presence on another branch is insufficient.
2. Support an ordinary compact boundary as a deliberate new root. Validate its post-boundary summary and active chain; do not append `logicalParentUuid` ancestry back into model context. Unsupported or malformed preserved-message/segment metadata must hold until the verifier implements the native relinking contract.
3. Validate post-boundary effect calls and their sibling tool results against the selected conversation, including provider message grouping where needed. Do not insist that a parallel tool-result row be a literal ancestor of the latest result.
4. Treat pre-compaction receipts as historical evidence owned by Crewroom's durable effect ledger. Compaction can legitimately remove their raw rows. Allowing their absence requires a Crewroom-verified compaction watermark/coverage record and reconciliation of pending effects; a summary's prose is not an effect receipt. If that evidence is unavailable, hold instead of assuming either execution or non-execution.

These are source findings and implementation guidance, not an executed branch/compaction acceptance test. The earlier authentication probe and parent-owned normal resume probe do not establish this broader recovery contract.

## Follow-up: interruption during text output

The pinned terminal-reason enum contains `aborted_streaming` and `aborted_tools`; it does **not** contain a bare `aborted`. The native interruption predicate `dH` recognizes exactly those two values, while the hard-failure predicate `Ect` explicitly excludes them. Source: enum and predicates at bytes `208139747`–`208140855`; schema `iy` at `208246934`.

| Terminal category | Pinned values |
| --- | --- |
| Interruption | `aborted_streaming`, `aborted_tools` |
| Hard failures | `blocking_limit`, `rapid_refill_breaker`, `prompt_too_long`, `image_error`, `model_error`, `api_error`, `malformed_tool_use_exhausted`, `budget_exhausted`, `structured_output_retry_exhausted`, `tool_deferred_unavailable`, `turn_setup_failed` |
| Other termination | `stop_hook_prevented`, `hook_stopped`, `tool_deferred`, `max_turns`, `background_requested`, `completed` |

The streaming loop checks its abort signal after model streaming, drains remaining tool results, emits an interruption marker where applicable, and returns `aborted_streaming` (byte `224259768`). The post-tool abort handler returns `aborted_tools` (`224274425`). An abort before prompt checking also explicitly produces `aborted_streaming` (`233446634`). These reasons name where the native loop stopped; they do not prove the process exited or that external tools produced no effects.

An interruption can legitimately be `subtype:"error_during_execution"` with `is_error:true`: engine result assembly uses the interruption predicate and the trailing message shape to decide whether to emit diagnostic errors or a success-shaped result. Thus neither `subtype` nor `is_error` alone classifies an interruption. Source: result assembly `224425841`–`224427350`; trailing-message predicate `fqo` at `223467723`; diagnostic builder `hqo` at `223468379`. Native batch lifecycle maps either interruption reason to `cancelled` through `Aqn` at `224388617`.

Implementation implication: normalize these two known terminal reasons as interruption before generic error classification, correlate Crewroom's recorded interrupt intent, and retain the native subtype/error fields as evidence. A reason can be absent when the query loop was bypassed, and unrecognized reasons are not proof of interruption. The parent-owned live text-interrupt probe supplies execution evidence; this subsection is source inspection only.
