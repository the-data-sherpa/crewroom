# Independent review of the external message experiment

Date: 2026-09-26. Reviewed [the runner](../../scripts/probe-omp-external-messages.py), raw RPC input/output, saved OMP conversations, OS process snapshots, Relaycast histories, and independent observer records. Also queried the running local Relaycast service read-only to verify stored messages and acknowledgements separately from the runner's success flag. No additional model calls were made for this review.

## Verdict

**The final attempt, `external-roundtrip-3`, passes the requested four-hop communication test across three independent OMP processes.** Three fresh traces produced exactly 12 sends and corresponding recipient observations. The final run contains no exhausted provider retries, hidden native subagents, native-team frames, or unsuccessful tool results.

This supports Relaycast plus an external OMP RPC adapter as the first candidate for Crewroom's next contract experiments. It does not establish restart recovery, independent crash containment, pause semantics, or production reliability.

## Final-run evidence

| Role | OS process / process group | OMP session |
| --- | --- | --- |
| Coordinator | 95672 / 95672 | `01a0dc3d-f9e8-71a8-9811-8bbea42eb085` |
| Specialist 1 | 95673 / 95673 | `01a0dc3d-fb2b-7385-93bf-b997bb5efe8b` |
| Specialist 2 | 95674 / 95674 | `01a0dc3d-fc74-76bd-83a1-36aa3586dabc` |

The saved `ps` output identifies `/Users/crewroom/.bun/bin/omp` for all three distinct processes; each also has a distinct process group. RPC state responses independently match the three saved session identities. All report OMP 18.3.1 with `aperture-openai/openai.gpt-6-sol`. All three exited with code 0.

Each process started with no tools, then received only the externally implemented `send_message` host tool. Its actual model-generated tool call, `host_tool_call`, host result, and successful `tool_execution_end` match by tool-call ID. OMP's model transcript includes its additional tool-intent field `i`; the actual host-call recipient and body match after excluding that annotation.

For every one of the 12 sends, the independent verifier found exactly one:

- Model tool call and successful result bearing the Relaycast message ID.
- Matching message in the recipient's saved OMP user-message envelope, including sender, recipient, ID, and exact body.
- Stored DM with the same sender, body, ID, and conversation, both in the saved history and a fresh REST query.
- Passive observer event with the same stored message, both in the saved observer log and a fresh query with the separate observer token.
- Acknowledged delivery belonging to the intended recipient, from a fresh recipient-token REST query.

The final mailbox contained 12 acknowledged deliveries and no pending items. The runner's 12 pre-delivery observer checks found the delivery status unchanged after passive reads. No process had native subagents when queried, and the raw RPC logs contain no native-team events. Specialist 2 first encounters each actual trace in a received envelope from Specialist 1. The coordinator's three completion messages follow the relayed reply.

The three round trips took **3.63, 4.48, and 3.76 seconds**, measured by the runner from the initial coordinator request through completion. These timings describe this small experiment, not expected coding-task latency.

## Earlier attempts and corrections

| Attempt | Observed outcome | Interpretation |
| --- | --- | --- |
| `external-roundtrip-1` | One completed round, then HTTP 429 while polling deliveries; eight sends and receives were recorded before shutdown | Host polling exceeded the local server's request limit. The overall attempt correctly failed. This is a polling design problem, not evidence that stored messages disappeared. |
| `external-roundtrip-2` | All 12 message hops independently verified, but seven `auto_retry_end` events reported exhausted empty-final retries | Transport passed; clean runner completion did not. The earlier success flag ignored these events and needed qualification. |
| `external-roundtrip-3` | Twelve verified hops, three completions, zero exhausted retry events, all processes exit 0 | Clean result for the bounded communication test. |

The host now uses a stored send as a wake-up hint and limits idle reconciliation to one poll per agent every two seconds. The final prompts require a nonempty `SENT` response after sending; this is an ordinary final response, not another inter-agent message. The final runner also fails on an exhausted retry event. These changes corrected the observed adapter and completion issues without replacing the mailbox. The empty-final errors do not establish an underlying provider-side cause beyond what the OMP events report.

## Limits that remain material

The host acknowledges delivery after observing the input in the recipient's OMP conversation. That proves session receipt; it does not prove application by itself. The subsequent protocol sends and final relayed response provide application evidence for this test. A real coding assignment needs its own explicit disposition and revision-specific evidence.

The live monitor shows process identity, observed activity, and messages. This review checked the data source and saved HTML; it did not interactively test the browser rendering. Polling the monitor is separate from consuming the agents' message queues.

The adapter's local `seen` set and in-flight mapping are memory-only. Relaycast's ordinary local send deduplication is also memory-backed in the inspected runtime. This experiment does not prove safe replay after a host or mailbox crash. Those are required next checks before adopting the runner for the live Jira-to-PR milestone. The existing [mailbox assessment](external-mailbox-preparation.md) records the one-hour default delivery expiry and other scope limits.

Private raw evidence is retained under `.crewroom/experiments/external-roundtrip-2/` and `external-roundtrip-3/`. Independent verification results are saved as `independent-review.json` in each directory. The main experiment also publishes [sanitized evidence](evidence/omp-external-roundtrip-2026-09-26.json) and a [saved monitor](evidence/omp-external-roundtrip-2026-09-26.html). Credential files were read only to perform local GET checks and were not copied into this report. Temporary model-configuration copies were absent after each run. The reviewed final runner's SHA-256 was `592b5b102fbf913f40d828e680ed1666a1b58f7d956c5f1a98449ca05185b487`.

After verification, the owned Relaycast service received SIGTERM and exited with code 0. A process check confirmed PID 93740 no longer existed, and a TCP connection check confirmed `127.0.0.1:18787` was closed. No mailbox service remains running from this preparation/review subtask.
