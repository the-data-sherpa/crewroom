# External mailbox preparation

Date: 2026-09-26. Scope: a local Relaycast setup and API smoke test, with no OMP model calls. This prepares the independent-process experiment; it does not establish agent execution or recovery.

## Result

**Relaycast is ready for the first three-process OMP test.** Published `@relaycast/engine@8.13.0` installed in the ignored experiment directory. A loopback-only server accepted an addressed DM, retained a recipient delivery, exposed the message to a separate read-only observer, and removed the pending item only after an explicit recipient acknowledgement.

The source examined is Relaycast commit [`130c8204886acdb111ee2fd67774dacf5b4e4637`](https://github.com/AgentWorkforce/relaycast/commit/130c8204886acdb111ee2fd67774dacf5b4e4637), previously verified as the v8.13.0 release. This run used the published package, not a source build. Registry integrity was `sha512-OKeml5fHAmsVbooPVPg1p2TwHTRknC3FT0Pxrl7/mRY37VSQsg+D6j61lYbBU0XD1UDUDzEc4JwIMRdNhOTS+Q==`; the registry did not supply a `gitHead` field. The local package lock records the resolved dependency graph.

## API contract to use

All paths below are relative to `http://127.0.0.1:18787/v1`. Responses wrap the result in `{ "ok": true, "data": ... }`. Keep bearer tokens inside the host adapter; models need only the recipient and message body.

| Operation | Request | Relevant response |
| --- | --- | --- |
| New disposable workspace | `POST /workspaces`, body `{name}` | `workspace_id`, `api_key` |
| Register each agent | `POST /agents`, workspace bearer key, body `{name}` | `id`, `name`, individual `token` |
| Send as the actual calling agent | `POST /dm`, sender bearer token, body `{to, text, mode:"wait"}` | `id`, `conversation_id`, `from_agent_id`, `to`, `message` |
| Poll one agent's durable pending deliveries | `GET /deliveries`, recipient bearer token | Array of delivery records with embedded `message` |
| Acknowledge one received item | `POST /deliveries/:id/ack`, recipient bearer token, body `{}` | Delivery record with `status:"acked"` |
| Create a passive observer | `POST /observer-tokens`, workspace bearer key, body `{name, scopes:["stream:read","dms:read"], filters:{include_dms:true}}` | Separate observer `token` |
| Read observer history | `GET /workspace/events?since=0&limit=100`, observer bearer token | `{events, latest_seq, next_since}` |
| Read conversation history | `GET /dm/:conversation_id/messages`, participant bearer token | Array containing sender identity, message ID, and text |

Workspace and agent registration follow the [self-host guide](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/docs/self-hosting.md). DM creation and participant history are defined by the [DM routes](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/routes/dm.ts). Polling and acknowledgements use the [delivery routes](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/routes/delivery.ts). Observer creation and event backfill are defined by the [observer routes](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/routes/observerToken.ts) and [workspace routes](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/routes/workspace.ts).

A pending item includes `id` (delivery ID), `message_id`, `agent_id` (recipient), `status`, and `message:{id, agent_id, agent_name, text, ...}` (sender and body). Default polling returns queued and delivered items, oldest first; it does not acknowledge them. Explicit `status=acked` retrieves acknowledgements for evidence. The observer advances its own `next_since` cursor, which does not advance the recipient's delivery queue. [Delivery implementation](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/engine/delivery.ts)

## Observed smoke test

The successful corrected smoke exchange created message `229476287873273856` and delivery `del_229476287873273857`. The recipient was offline from Relaycast's built-in node transport, so the delivery stayed queued with a truthful connection-not-ready observation. The host adapter can consume this durable pending route and send the payload to OMP through RPC.

1. Send returned the stored DM and actual sender identity.
2. Recipient polling returned one queued delivery containing the same body and message ID.
3. The independent observer saw `dm.received`, its conversation ID, sender, and message text. Participant history also returned the same message.
4. Polling again returned an unchanged pending delivery.
5. Explicit acknowledgement changed its status to `acked`; the next pending poll returned an empty array.

The first observer token omitted `dms:read`, so it correctly returned no DM events. Repeating with both required scopes fixed visibility; the successful test asserts that the DM actually appears. Raw smoke evidence and private credentials are separate files in `.crewroom/experiments/external-mailbox-prep/`. The credentials file has mode 0600; the containing experiment directory has mode 0700. No external service, account, Jira issue, or repository work was involved.

## Runtime and reproduction

The installation is under `.crewroom/experiments/external-mailbox-prep/runtime/`. `serve-loopback.mjs` uses the official `createNodeRuntime` and `createEngine` exports with Hono's server adapter, explicitly setting `hostname:"127.0.0.1"`. It serves the REST API needed for this test; it does not wire WebSocket upgrades or file-upload routes. The packaged `startServer` omits the listen hostname, so using it unchanged would not meet the loopback-only requirement. [Official Node entry point](https://github.com/AgentWorkforce/relaycast/blob/130c8204886acdb111ee2fd67774dacf5b4e4637/packages/engine/src/entrypoints/node.ts)

Use the explicit Node 22 binary because this installation's native SQLite module was built for Node 22. An initial launch picked up Node 26 from the directory's environment and failed with a native-module ABI mismatch before opening a listener. Launching with Node 22.22.3 succeeded. This is an experiment runtime choice, not a change to Crewroom's Node requirement.

From the runtime directory:

```sh
mkdir -p data
/Users/crewroom/.local/share/mise/installs/node/22.22.3/bin/node serve-loopback.mjs
```

The wrapper accepts `CREWROOM_RELAY_PORT` and `CREWROOM_RELAY_DATA` for a separate port and database directory. Stop it with SIGTERM or Ctrl-C; the wrapper closes HTTP service, timers, and SQLite. The preparation instance initially started as PID 93740; `lsof` confirmed `127.0.0.1:18787` rather than a wildcard listener. After the independent-process experiment and read-only verification, it received SIGTERM and exited with code 0. A subsequent process check found no such PID, and a TCP connection check confirmed port 18787 was closed. The tested wrapper is also retained as [scripts/serve-relaycast-probe.mjs](../../scripts/serve-relaycast-probe.mjs).

## Boundaries for the OMP test

Expose a single `send_message` host tool to each independent OMP process. Bind the sender's bearer token to that process in the host; do not trust a model-supplied sender. Relaycast carries the message, and a separate host adapter polls the intended recipient's queue and supplies a structured OMP RPC prompt. OMP's internal team bus is unnecessary. Save process output, tool-call IDs, Relaycast message/delivery IDs, recipient transcripts, and the passive observer log separately.

An accepted send is not an agent receipt. A host acknowledgement is not proof that the model applied the message. The four-hop test must match each actual tool send with its stored DM, recipient transcript, and subsequent response. Ack after the chosen observable recipient boundary and name that boundary in the report.

The default local idempotency store remains memory-backed; ordinary DM retries after a mailbox restart can duplicate a send. The smoke test did not exercise restart deduplication, message expiry, crash after RPC acceptance, paused agents, or replacement ownership. Those remain separate contract checks. Default delivery expiry in this run was one hour; a production recovery design must set or reconcile that policy explicitly. [Existing source assessment](agent-relay-assessment.md#restart-and-duplicate-handling)
