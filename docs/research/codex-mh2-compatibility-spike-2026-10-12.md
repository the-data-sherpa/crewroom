# MH2 compatibility spike — FOR-7

The bounded spike passed. **Use each harness's existing authenticated model access**, as the operator clarified on 2026-10-12: “The harnesses should already be set up. The harnesses are the providers.” Codex's existing ChatGPT login completed a live addressed tool exchange and retained the result after a forced restart. A separate Crewroom API key is not a prerequisite for this route.

This supersedes the gateway-first recommendation in the earlier readiness review. It establishes an implementable Codex protocol/profile for the next slice, not a completed production adapter. FOR-7 remains In Progress; MH3 follows its acceptance. Runtime code, active installation and user configuration are unchanged.

## Evidence boundary

Application source: `77054f8690ad361f582892042323dae454568c0e`. Executable: pinned Linux `codex-cli 0.162.0`. The [machine-readable receipt](../testing/codex-mh2-spike-2026-10-12.json) binds executable and probe hashes, exact launch commands, individual assertions, initial fixture failures and local raw-evidence paths. The probe scripts are [synthetic compatibility](../../scripts/probe_codex_runtime.py) and [native authenticated check](../../scripts/probe_codex_native_auth.py).

| Evidence | Executed result | What it establishes |
| --- | --- | --- |
| Real Codex + synthetic Responses | **23/23 cases for gpt-6.1-sol; 23/23 for gpt-6-astra** | Request/tool compatibility, explicit native-tool denial, interruption and crash observations for the pinned restricted profile. Expected policy failures count as successful diagnostics. |
| Real Codex + existing ChatGPT login | **Two live turns passed**, gpt-6-astra at low effort | One addressed `crewroom_echo` call/result, then SIGKILL, fresh process, same thread and correct retained result without another tool call. |
| Native usage | Final cumulative snapshot: **15,421 total tokens**, including 15,330 input, 9,856 cached input and 91 output | Harness-reported thread accounting, not a strict token/spend cap. Cached input is a subset of input. No model-reroute notification was observed. |
| Cleanup | All recorded children terminated and reaped | Disposable probe cleanup; not Crewroom's production stopped-ownership acceptance. |

The synthetic stack used private filesystem, PID and network namespaces: only loopback, no routes, no home directory or credentials. Codex connected through a capture proxy to the **unchanged Rust ProviderGateway**, then to scripted Responses. One clearly labelled direct synthetic diagnostic bypassed the gateway. Neither path used a real upstream provider.

The live check used a private filesystem/PID namespace and shared network for native authentication/inference. It mounted only the existing Codex auth file read-only, system binaries/trust store and private probe output; no repository or ambient user configuration. The original auth file's size/mtime remained unchanged. Account identity/email/plan and credentials were not retained in the receipt. This proves access using the current login, not a production credential-refresh or concurrent-refresh design. Managed native authentication can refresh remotely; a read-only file mount is only a probe technique. [Official authentication contract](https://learn.chatgpt.com/docs/app-server#auth-endpoints).

## Tool and transport profile

Feature switches alone were insufficient: the initial catalog produced an `additional_tools` input item, which the gateway rejected before upstream delivery. The successful profile copies the pinned model's complete bundled row, preserving model instructions, with these explicit changes:

```json
{
  "apply_patch_tool_type": null,
  "supports_search_tool": false,
  "experimental_supported_tools": [],
  "node_repl_disabled": true,
  "tool_mode": "direct",
  "use_responses_lite": false
}
```

It supplies that restricted catalog at process startup, disables shell/unified exec, internal agents, apps/plugins/hooks, code mode, search/image/browser/computer tools and auxiliary plan/input tools, and disables compression for captured requests. Exact settings are in the executable probes. `use_responses_lite:false` is a tested compatibility setting, not an authentication requirement. Unsupported executable/catalog changes must fail admission until retested; these experimental details are not a universal Codex allowlist.

Only the flat `crewroom_echo` function was advertised in the successful synthetic request inventories. Injected `exec_command`, function/custom `apply_patch`, `spawn_agent`, `update_plan`, `request_user_input` and `web.run` calls produced explicit unsupported-tool results, no host dynamic call and no mutation marker. The custom-patch history was then rejected by the gateway's input policy, ending that turn as failed. Denial is established; graceful continuation from every unsupported history shape is not.

The live native route used the same restrictions with built-in `model_provider="openai"`, file-backed existing authentication and fallback disabled. No custom provider URL, gateway credential or API key was supplied. The effective start reply reported the selected model/low effort/read-only sandbox. The live model called the registered function with the expected arguments, received a host-generated unpredictable marker and returned it. This is stronger than checking for a successful initialization or plausible assistant text. Live encrypted requests were not intercepted; synthetic request captures are the tool-inventory evidence. [Official app-server tool contract](https://learn.chatgpt.com/docs/app-server#dynamic-tool-calls-experimental).

## Recovery result that must shape implementation

| Crash point | Stand-in effect happened | Host receipt exists | Resumed Codex history |
| --- | --- | --- | --- |
| Before effect | No | No | Tool output `aborted` |
| After effect, before receipt | Yes | No | Tool output `aborted` |
| After receipt, before reply | Yes | Yes | Tool output `aborted` |
| After reply reached the next provider request | Yes | Yes | Tool output `SAVED_RESULT` |

Fresh app-server processes resumed the same durable thread and did not automatically reissue the original server request. **`aborted` does not mean the side effect did not happen.** The resumed synthetic model was scripted to report history; a real model could request an equivalent operation with a new call ID. These observations do not prove exactly-once effects.

Crewroom must persist the assignment/thread/turn/call-to-operation mapping and reconcile its own durable receipt or actual effect before allowing replacement work. Recover a known result from the original operation; uncertain effects remain held. Do not convert a new attempt or RPC ID into permission to execute an unresolved operation again. The live restart proved retained completed history, not recovery from an ambiguous live mutation.

Streaming interruption and interruption while waiting for a host tool both produced terminal `interrupted`. After an actual completed turn, history and dynamic-tool registration survived SIGKILL without re-registering tools on `thread/resume`. Two simultaneously living app-server processes had distinct thread/PID identities and received correctly addressed results. They were not production host/runner pairs and did not exercise external Mail.

## Gateway findings and native-route implications

Captured flat-function turns passed the unchanged gateway. Wrong effort and excessive serialized context were blocked with zero upstream requests. A scripted HTTP 429 terminated without fallback under the fixture's zero-retry configuration. Custom-provider compaction used `/responses`, completed with a scripted text summary, and retained working dynamic tools on the following turn. This does not establish native remote-compaction or retry behavior.

The operator-selected native harness route does not traverse that gateway. Preserve the architecture's truthful accounting and supervision rules by making enforcement capabilities explicit:

- Freeze model/effort and adapter profile, compare effective start/resume identity, and handle `model/rerouted` visibly. No observed reroute is not proof that upstream rerouting is impossible.
- Consume `thread/tokenUsage/updated` totals as cumulative snapshots, preserving original thread identity across attempts. Do not sum cumulative totals or substitute account-wide usage for assignment usage.
- Retain Crewroom's persistent active-time limit and safe-pause/ownership checks. The installed turn schema exposes no verified hard output-token ceiling; usage snapshots and timeouts cannot be labelled a strict monetary or token cap.
- Represent unavailable pre-request gateway controls accurately. Do not silently claim identical model/request/history/retry accounting for native inference. Gate settings that require unavailable enforcement.
- Keep native login and refresh ownership server-side. Isolate assignment configuration/session history without importing ambient hooks, plugins, MCP servers or tools. Design refresh serialization separately from per-assignment state.

These changes fit the accepted architecture: usage/cost can be actual, estimated or unavailable; an unenforceable monetary budget must not be presented as enforced. They require implementation and acceptance evidence before enabling the adapter.

## Next implementation work

1. Extract the existing OMP seam with additive adapter/identity records and unchanged workflow authority. Preserve old sessions, ownership roots and untagged records.
2. Add the pinned Codex profile, native-auth availability checks, JSONL correlation and normalized terminal/usage/reroute events. Use existing Crewroom host tools and external Mail.
3. Integrate durable logical-call mapping and reconciliation, then production interruption, host/daemon reattachment and stopped-ownership checks. Reject wrong/stopped assignment calls and unknown native requests.
4. Prove native credential-refresh behavior and truthful budget capabilities, then run both adapters' real contract/regression gates. Resolve the installed OMP 17.3.5 versus recorded v2 fixture baseline and Linux fixture portability without changing global tools.

The existing 23 Rust and nine discovery baseline checks remain historical readiness evidence. This spike built the unchanged Rust gateway/oracle examples, passed Python syntax checks and executed the matrices above. Full domain/build/browser suites were not rerun because no production domain, build or user-flow code changed. Linux browser 29/31 limits remain unchanged.

Initial fixture failures are retained: oversized prompt rejected by Codex before reaching the gateway, missing queued compaction response, and missing CA-bundle target in the first native-auth mounts. Corrected runs passed; the failed attempts are not attributed to product defects.
