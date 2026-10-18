# Native OMP message round trip

**Architecture status, 2026-09-26:** the operator subsequently required individual agent processes with externally observable progress and messages. This successful single-process test does not meet that requirement; native OMP team messaging is excluded from Crewroom's design. See [ADR 0004](../adr/0004-observe-independent-agent-processes.md). The evidence below is retained unchanged.

Tested 2026-09-25, America/New_York (2026-09-26 UTC). **Passed: one live round trip through three distinct agent sessions, with four explicit sends.** OMP 18.3.1 used the configured `aperture-openai/openai.gpt-6-sol` model for all three sessions. The successful run took 8.29 seconds from the probe's post-prompt timer to the coordinator's final acknowledgement.

The user proposed this experiment to test real coordinator and peer communication:

```text
Coordinator ── START ──► Specialist 1
                         │
                         └── FORWARD ──► Specialist 2
                                              │
                         ◄── S2_ACK ──────────┘
Coordinator ◄── S1_RELAY ─┘
```

## Evidence

The coordinator created two native OMP task agents. Each had its own session ID and persisted conversation. All three used the same OMP process; this experiment does not establish process or filesystem isolation. Each send used OMP's native `write` tool with an `agent://` destination. The host observed the RPC stream and did not forward any of the four messages for the agents.

The marker was `crewroom-e14cd839207d4a16`. Specialist 2's initial instructions and shared task context did not contain it. Its first recorded exposure was the incoming message from Specialist 1.

| Hop | Sender → recipient | Message | Runtime receipt | Persisted recipient message |
| --- | --- | --- | --- | --- |
| 1 | Coordinator → Specialist 1 | `START` and marker, with return instructions | `woken` | Verified |
| 2 | Specialist 1 → Specialist 2 | `FORWARD` and marker | `woken` | Verified |
| 3 | Specialist 2 → Specialist 1 | `S2_ACK` and marker | `woken` | Verified |
| 4 | Specialist 1 → coordinator | `S1_RELAY S2_ACK` and marker | `injected` | Verified |

The final coordinator acknowledgement follows the persisted incoming relay. Verification checks tool-call/result pairing, sender and recipient identities, exact payloads, distinct sessions, serving models, and clean shutdown. A model's completion claim alone is insufficient. See the [sanitized evidence](evidence/omp-roundtrip-2026-09-25.json), which includes session/message identifiers and hashes of the private source records.

OMP also emitted lifecycle events and automatic task-result messages. Four is the number of **explicit protocol sends**, not the total number of runtime notifications. An automatic task-result reply woke Specialist 2 again near shutdown. Crewroom must account for these events when deciding whether a conversation is finished and preventing acknowledgement loops.

## Setup findings

The first attempt failed before creating specialists because the temporary role placed under the overridden agent-state directory was not discovered. Moving it to the disposable project's `.omp/agents/` directory worked. The probe now checks role discovery before asking the model to begin. No global role configuration was changed.

The successful run included one invalid `read agent://` request, followed by successful delivery using the actual returned specialist names and incoming sender metadata. The reproduction prompt now uses that metadata and asks the coordinator to finish its turn while waiting instead of polling outputs. That prompt adjustment has not received another live run; the recorded success and evidence refer to the preceding prompt retained with the raw output.

The post-completion `get_subagents` snapshot contained only one specialist, while lifecycle events and session files retained both. A Crewroom adapter must retain identities from creation/lifecycle events rather than treating a late snapshot as a complete historical roster.

## Reproduce and verify

Run from the repository root with an existing OMP provider configuration. Choose a new output directory on each run:

```sh
python3 scripts/probe-omp-roundtrip.py \
  --model aperture-openai/openai.gpt-6-sol \
  --output .crewroom/experiments/roundtrip-next
python3 scripts/verify-omp-roundtrip.py \
  .crewroom/experiments/roundtrip-next \
  --output .crewroom/experiments/roundtrip-next/verified.json
```

The probe exit code indicates whether the coordinator claimed completion; the separate verifier establishes whether retained evidence supports it. Raw events, prompts, and sessions remain in ignored `.crewroom/experiments/roundtrip-live-2/`. A private copy of the existing model configuration was used and removed in cleanup; credentials are absent from the public evidence. Models received only the experiment instructions. The runtime tool surface was restricted and inspected, but the disposable working directory is not a security sandbox.

No Jira, PR, the pilot repository, or application-domain operations were requested or observed. OMP exited normally after the probe aborted remaining work and closed the RPC stream. No new messaging dependency was installed.

## What this establishes

Native OMP can wake specialists, deliver peer messages, retain incoming messages in separate conversations, and return the peer reply to a busy coordinator in this live configuration. These findings apply to the tested single-process arrangement, which the operator has since excluded from Crewroom's design.

It does not establish communication across independent processes, crash recovery, durable pending delivery, duplicate suppression, ordering under load, pause enforcement across all children, or useful coding/review execution. The next communication experiment should first repeat this exchange with three independent processes and external messaging, then interrupt one specialist and check recovery without duplicate work. No runner adapter is adopted by this result.
