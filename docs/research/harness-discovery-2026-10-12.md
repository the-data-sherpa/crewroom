# Harness model discovery: Claude Code and OMP

Observed 2026-10-12 in the `feat/multi-harness` worktree, starting from `dc85a54a24e36b2dc57779bb2b9e46f12fcb26db`. Research only: version/help queries, source inspection, one Claude initialization-only exchange and one OMP catalog command. No user prompt, inference turn, configuration edit or active Crewroom run was submitted. Codex discovery is covered by the parent implementation work.

## Installed executables

| Harness | Observed executable | Reported version |
| --- | --- | --- |
| Claude Code | `/home/crewroom/.local/share/claude/versions/2.1.292`, reached through `/home/crewroom/.local/bin/claude` | `2.1.292 (Claude Code)` |
| OMP | `/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js`, reached through `/home/crewroom/.bun/bin/omp` | `omp/17.3.5` |

These versions come from local `--version`; `--help` was inspected. OMP is older than the repository's recorded macOS 18.4.12 runtime, so its discovery result does not establish production adapter compatibility. [Recorded environment](../status.md#environment).

## Claude: verified initialization without inference

The installed CLI accepts this argv (the empty strings are actual empty arguments):

```text
claude --bare -p --input-format stream-json --output-format stream-json
  --verbose --no-session-persistence --tools "" --strict-mcp-config
  --mcp-config '{"mcpServers":{}}' --setting-sources ""
  --settings '{"disableAllHooks":true}'
```

The probe used a temporary empty working directory and temporary `CLAUDE_CONFIG_DIR`. The child environment contained only inherited `PATH`/`HOME`, that temporary config path, and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`; it did not inherit provider keys or routing settings. `--bare` skips ambient configuration/features and, according to the installed help, does not read OAuth/keychain authentication. No directories or plugins were explicitly added. The official CLI reference documents bare mode, explicit tool restriction, streaming formats and session persistence controls. [CLI reference](https://code.claude.com/docs/en/cli-reference).

Write exactly one newline-terminated JSON frame to stdin, then close stdin:

```json
{"type":"control_request","request_id":"discovery","request":{"subtype":"initialize","hooks":null}}
```

This is the official Python SDK's initialization control envelope, not an inferred natural-language command. That source returns initialization metadata before sending a user message. [Initialization](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L309), [control request envelope](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L706).

**Observed result:** exit 0, empty stderr, a `system/ui_invalidate` frame followed by the matching successful `control_response`. Models are at `response.response.models`; the outer response also has `subtype:"success"` and the matching `request_id`. No assistant/result/inference message appeared.

| `value` (selection alias) | `resolvedModel` observed |
| --- | --- |
| `default` | `claude-opus-5-5` |
| `opus` | `claude-opus-5-5` |
| `fable` | `claude-fable-5-1` |
| `sonnet` | `claude-sonnet-5-5` |
| `haiku` | `claude-haiku-4-5-20251001` |

Rows include `displayName`, `description`, and optional `supportsEffort`, `supportedEffortLevels`, `supportsAdaptiveThinking`, `supportsFastMode`, `supportsAutoMode`. The first four rows advertised efforts `low`, `medium`, `high`, `xhigh`, `max`; Haiku omitted those fields. Preserve alias and resolved ID separately: `default` and `opus` are distinct choices even when currently resolving identically. Missing capabilities are unknown, not proof of support.

**This is an isolated advertised menu, not authenticated entitlement.** It returned models with no credentials. Real settings can further restrict model selection; API `GET /v1/models` is a separate authenticated API availability surface, not proof that a Claude subscription can use a model through this harness. Do not label discovery as successful inference, valid billing or guaranteed access. The raw initialization reply also contains account/path/process metadata: retain only an explicit model-field allowlist. [Model policy](https://code.claude.com/docs/en/model-config), [API model listing](https://platform.claude.com/docs/en/api/models/list).

## OMP: verified catalog command

```text
omp models --json --no-extensions
```

Run from an empty temporary directory, this exited 0 with empty stderr and returned `{"models":[...]}` containing **109 rows across four provider identifiers**. It did not start an agent conversation. Its JSON projection includes only `provider`, `id`, `selector`, `name`, `contextWindow`, `maxTokens`, `reasoning`, `thinking`, `input`, `cost`; it omits credentials, endpoints and headers. Example shape:

```json
{"provider":"meta","id":"muse-spark-1.2","selector":"meta/muse-spark-1.2","name":"Muse Spark 1.2","contextWindow":1048576,"maxTokens":131072,"reasoning":true,"thinking":["minimal","low","medium","high","xhigh"],"input":["text","image"]}
```

Nonreasoning entries can have `thinking:null`; limits can also be null. Do not infer tool support or API family: this projection does not supply them. [Installed JSON projection](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/cli/models-cli.ts:106).

Listing uses `getAvailable()`: models with configured provider authentication or keyless providers, excluding disabled providers. This is stronger than an unfiltered catalog but still does not establish fresh credentials, account entitlement or a successful model request. The surrounding command performs `online-if-uncached` discovery, which can contact providers, update caches and resolve authentication; it is **not strictly offline or filesystem-read-only**. Explicit `refresh` forces online discovery and is unnecessary here. `--no-extensions` suppresses ambient/configured extension discovery. Never forward raw stderr: custom configuration errors can include private details. [Availability filter](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/config/model-registry.ts:1633), [command/discovery](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/cli/models-cli.ts:349), [extensions](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/cli/models-cli.ts:298).

## Future execution-adapter constraints

- **Claude lifecycle:** streaming input supports long-lived multi-turn processes, queued input and interruption. The SDK control methods send `interrupt`, `set_permission_mode`, and `set_model`; session IDs support explicit resume/fork. The CLI subprocess wire protocol should be version-tested; this research verifies initialization only, not those control operations or recovery. [Streaming](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode), [controls](https://github.com/anthropics/claude-agent-sdk-python/blob/b6e9d12fe1cc98dde988ab7b7713c1feeee50c6c/src/claude_agent_sdk/_internal/query.py#L792), [sessions](https://code.claude.com/docs/en/agent-sdk/sessions).
- **Claude tool boundaries:** `allowedTools` auto-approves matching tools; it is not the available-tool allowlist. Restrict with `tools`/`--tools`, and use `PreToolUse` for a gate covering every tool call: `canUseTool` can be skipped by earlier approvals. Crewroom tools can be exposed through MCP. Preserve separate externally supervised role processes instead of substituting Claude's internal subagents. [Permission ordering](https://code.claude.com/docs/en/agent-sdk/permissions), [MCP integration](https://code.claude.com/docs/en/agent-sdk/mcp), [Crewroom process contract](../../AGENTS.md).
- **OMP control:** installed RPC supports `get_available_models`, `set_model` with `provider`/`modelId`, `get_state`, `abort`, `steer`, `follow_up`, `new_session`, `switch_session`, and explicit retry/compaction controls. `get_available_models` waits for background discovery and returns `{type:"response",command:"get_available_models",success:true,data:{models:[...]}}`; prefer the narrower CLI JSON projection for public discovery. Host tools use `set_host_tools` then correlated `host_tool_call`/`host_tool_result`, with cancellation/update frames. These are source-verified, not newly live-tested here. [RPC commands](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/modes/rpc/rpc-types.ts:29), [model response](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/modes/rpc/rpc-mode.ts:1214), [host-tool frames](/home/crewroom/.bun/install/global/node_modules/@oh-my-pi/pi-coding-agent/src/modes/rpc/rpc-types.ts:434).

Recommended discovery record: executable/version, observation time, discovery scope, alias and resolved selector, explicit capabilities, and `entitlementVerified:false`. Treat discovery, authentication status, successful inference, and safe execution-adapter validation as distinct evidence.
