# Steering across reconnect

Key: `steering-reconnect`. Aliases: steering, reconnect. Covers UI-02 and the steering/client-reconnect portion of E2E-05. Read [shared setup, identity, isolation, outcomes and cleanup](README.md) first. Platform: Linux source/debug, headless native GPUI backed by the production Rust daemon; scripted RPC and Mail, no live provider delivery. Docker is not required.

```sh
python3 scripts/verify-feature.py check steering-reconnect
python3 scripts/verify-feature.py run steering-reconnect --evidence-dir .crewroom/test-results/f12s1
```

## Actions and observable outcomes

1. The versioned fixture creates its own task/run and starts one separately supervised scripted coordinator. It holds delivery using the fixture Mail latch while keeping the daemon available. This is controlled delivery unavailability; submission to an offline daemon is not claimed.
2. In the native workbench, select the fixture ticket, open **Conversation**, enter `FOR-12: retain this direction across reconnect; receipt is separate from application.` and activate **Send** (`save-steering`). The result must contain exactly one stored instruction with its ID/body and pending receipt presentation (`native-stored-pending`).
3. Close that native client and open another against the same private daemon descriptor. Select the same ticket and **Conversation**. The same ID/body must still be pending, with no duplicate (`reconnect-preserves-pending`).
4. Release the fixture delivery latch. Observe an actual RPC receipt from the admitted host and durable `received`, then reopen native **Conversation** to inspect received presentation (`host-received`, `native-received`).
5. Ask the scripted coordinator through its authenticated host tool to submit an applied steering disposition. A plain assistant reply is insufficient. Observe durable application (`host-applied`), reopen the native client, and compare the same ID, recorded transition times and applied presentation without duplicate messages (`reconnect-preserves-applied`).

Only setup/latch control and the scripted coordinator's explicit disposition are harness actions. Ticket selection, text entry, Send and state readback use native controls. Exact input text and generated record IDs are retained in native action plans/results and the journey receipt.

## Evidence and diagnosis

The helper requires all six assertion IDs above, actual native test executions and successful owned cleanup. Compare journey before/after messages with private `native-*.json`/`native-*-result.json`, `native-*.log`, host RPC/events and `feature-cleanup.json` below the receipt's fixture root. Missing receipt/application remains incomplete; stored state alone is not delivery. Reconnect is client close/reopen, not machine-crash or offline-write certification.

Follow [receipt recovery and bounded rerun rules](README.md#evidence-cleanup-and-reruns). Preserve this attempt and use a new evidence directory after correcting a stale selector or failed prerequisite. Cleanup stops only this run's registered resources and preserves its evidence.
