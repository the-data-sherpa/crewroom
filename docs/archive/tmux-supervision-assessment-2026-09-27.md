# tmux, supervision, and external agent messaging

Date: 2026-09-27. Research only. No services, agent sessions, or tmux sessions were started. The only local tmux command was `tmux -V`, which returned `tmux 3.7c`.

## Recommendation

Use tmux as an optional visible home for independent agent processes, paired with a durable backend and a runner adapter. tmux alone cannot satisfy Crewroom's message receipts, recovery, ownership, or evidence gates. Adding a backend is the right architectural direction; selecting a mailbox is one part of that backend, not a replacement for supervision.

The existing architecture already separates these responsibilities. Keep one authority for workflow state and offer both the browser workbench and terminal views over the same observations. Each coordinator or specialist remains a distinct OMP process. A shared backend or tmux server does not make the agents a single OMP process. This recommendation follows the [accepted architecture](../architecture.md#agent-processes-and-communication) and [product direction](../product.md#operator-control).

## What tmux provides

The official manual describes detachable terminal sessions, multiple clients, pane IDs stable for each pane's lifetime, a read-only client mode, and `remain-on-exit` for retaining an exited pane. This supports a useful live agent dashboard and later reattachment. Persistence through client detachment is not a documented promise of recovery after server failure or host reboot. [Official manual](https://man.openbsd.org/tmux.1), [released manual source](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/tmux.1).

`send-keys` converts strings to key events and sends them into the target pane or its current mode; its success is not an acknowledgement from the application running there. Thus typing a prompt followed by Enter can be a convenience, but Crewroom cannot interpret command success as recipient storage, execution, or applied steering. A busy application, copy mode, or changed foreground program makes the destination semantics different. These are conclusions from the input path, not locally reproduced failures. [Released input implementation](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-send-keys.c#L61).

Control mode improves programmatic terminal control: commands have framed responses and pane output has notifications. Those responses describe tmux commands; they do not add application-level message IDs or acknowledgements. Its output code has per-client buffering, discard paths, and slow-client handling. Consequently a control-mode stream is suitable for a terminal viewer, not the only record of work or a durable inbox. In particular, pausing a control client's output is not pausing the agent's execution. [Released control implementation](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/control.c#L269), [output age handling](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/control.c#L439).

tmux records the first pane process and checks its exit. This does not certify that every shell/tool descendant stopped. Pane destruction closes the pseudo-terminal; that is not an ownership or side-effect reconciliation protocol. A dead pane must not by itself authorize a replacement writer. [Pane creation](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/spawn.c#L391), [child exit handling](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/server.c#L485), [pane destruction](https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/window.c#L1090).

## A concrete combined design

```text
Browser workbench                       tmux terminal views
       |                                 |       |       |
       v                                 v       v       v
Crewroom backend <------------------ agent host wrappers
  workflow authority                  |       |       |
  durable assignments + actions       OMP     OMP     OMP
  external mailbox                    coordinator  specialists
  observations + evidence
```

This is a proposed integration, not delivered code:

1. **Backend:** durably record accepted work, agent assignments, steering, message identities, delivery disposition, ownership, and external action outcomes. Publish an observer feed with its own cursor. The mailbox can be an adopted service behind this boundary; it must not become a second authority for workflow completion.
2. **Agent host:** run one small host wrapper in each tmux pane. The wrapper launches an independent OMP child in RPC mode with pipes, captures structured events, and renders readable progress in the pane. Use a dedicated tmux socket and controlled configuration for Crewroom. The browser consumes the same external observations; terminal scrollback is supplemental.
3. **Message adapter:** give agents an explicit send/reply operation. Persist before reporting acceptance. Address logical run/assignment identities and record the current attempt separately. The receiver host records the input before dispatching it to OMP; only the allowed delivery state may trigger a turn. Monitor reads must not acknowledge the recipient's work.
4. **Runner adapter:** select and verify each model/effort, issue prompt/steer/abort commands, and associate events with the exact process generation and session. Do not assume the native OMP TUI and RPC run simultaneously: the tested mode is RPC with `--no-ui`, so terminal output would be the host's rendering. An interactive TUI alternative would need a separate contract experiment.
5. **Supervisor:** record backend instance identity, pane identity, host/OMP process identity including start identity, attempt, and checkout ownership. Reconnect to a surviving host when ownership is established; otherwise stop/reconcile it before replacement. Preserve exit observations and do not blindly `respawn-pane` into an old attempt.

A stricter separation is possible later: hosts run independently of tmux while panes only render their feeds. That makes loss of the terminal server a viewer failure. For a first experiment, the per-pane host is an understandable topology, provided tmux-server death is explicitly tested and recovery does not assume its descendants stopped.

These proposed responsibilities follow the [approved recovery and evidence contracts](../architecture.md) and the gaps in [executed offline tests](../testing/results-2026-09-26.md). They are not guarantees supplied by tmux.

## What existing OMP evidence does and does not establish

The local experiments establish correlated RPC control, independent OMP processes, model identity binding, observable scripted turns, and abort/reuse after a held model stream. The harnesses already launch OMP through pipes with `start_new_session=True`. They did not run OMP inside tmux and did not prove stopping shell/tool descendants. [RPC report](../research/omp-contract-experiment.md), [offline results](../testing/results-2026-09-26.md), [fixture](../../scripts/omp_offline_fixture.py).

They also found three independent issues: Relaycast retry deduplication resets after restart; an unfinished first OMP turn can be visible through RPC without being persisted; and unsupported effort settings can be normalized. tmux does not repair any of these. The backend needs retained inputs and action reconciliation; the runner adapter needs strict setting validation. A different mailbox can address the first issue if its contract passes. [Recorded failures](../testing/results-2026-09-26.md#failures-and-required-changes).

An expired ownership token can reject stale mailbox or provider calls, but cannot fence direct filesystem writes by an old tool descendant. A replacement must establish that the prior writer stopped, or quarantine its checkout and reconcile changes before proceeding. This is a design consequence of direct repository access and the accepted one-writer rule, not a claim that a tmux flag solves it. [Ownership and recovery direction](../architecture.md#records-and-ownership).

## Minimum combined acceptance tests

Run these with real independent OMP processes, a real candidate mailbox, and scripted loopback model/provider boundaries. Retain the same guarded offline environment as the existing suite. Each result must identify process, attempt, model, message, and revision where applicable.

| Test | Required observation |
| --- | --- |
| Three-agent route | Coordinator → S1 → S2 → S1 → coordinator through the backend; distinct processes/sessions and correct assigned models; no terminal typing or internal OMP team bus |
| Two passive viewers | Browser and tmux show output/tool/message progress concurrently; disconnect/reconnect either viewer without consuming inbox state or duplicating execution |
| Busy/paused delivery | Persist a peer message and operator steering while busy or paused; dispatch at the defined safe point, with stored/received/applied states distinguished |
| Lost send response | Commit a send, drop its reply, kill/restart mailbox, retry same logical ID; one message/delivery; changed payload with same ID rejected; retention limits explicitly exercised |
| First-turn crash | Kill OMP before its first durable checkpoint; reconstruct input from Crewroom records into a recorded replacement without silently losing work |
| Host/backend crash | Crash before and after durable input receipt and dispatch; restart without double ownership, lost steering, or treating an uncertain effect as absent |
| tmux failures | Detach a client, kill a pane host, then separately terminate the dedicated tmux server; verify peer visibility, real survivor inventory, and bounded recovery |
| Tool interruption | Pause during an actual local shell tool with a child process; show pausing until no owned mutating descendant remains; test forced exit and escaped descendants |
| Stale writer | Old host returns after replacement is considered; refuse a second writer until the old generation is stopped or its checkout quarantined and reconciled |
| Model enforcement | Unsupported effort fails before execution or resolves through an explicit policy; actual OMP model/effort readback matches the authorized assignment |
| Evidence and publication | Resume a disposable coding task after interruption, avoid repeating completed actions, bind tests/review to its exact revision, and reconcile lost synthetic Jira/PR replies |

These extend the existing [suite specification](../testing/strategy.md); passing a mailbox round trip alone does not certify the complete design. Nothing in this research note advances the implementation or acceptance status.

## Source and validation record

- Installed binary: `tmux 3.7c`, version query only.
- Latest GitHub release checked: `3.7c`, published 2026-08-17. Released source pinned above: `e476c1230b958df0cb12977517d24b3dc931375b`.
- Current upstream master also inspected: `94796f6b1182507efac8a272fc309a79e22e58a5`, dated 2026-09-25. Findings use release-pinned links to avoid treating newer master behavior as installed behavior.
- Primary sources: [official project](https://github.com/tmux/tmux), [3.7c release](https://github.com/tmux/tmux/releases/tag/3.7c), [OpenBSD manual](https://man.openbsd.org/tmux.1).
- Source inspection and existing experiment review only. No tmux lifecycle, process-descendant, or combined backend test was run for this note.
