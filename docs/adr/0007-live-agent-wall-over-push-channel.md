# Present agents as a live terminal wall over a push channel

Status: direction accepted by the operator on 2026-09-28; renderer adoption depends on the spike in [the MVP plan](../mvp-plan.md#design-review-remediation). Amends the observation parts of [ADR 0004](0004-observe-independent-agent-processes.md) and [ADR 0005](0005-use-omp-tmux-and-rust-agent-mail.md); their process, messaging and control contracts are unchanged.

## Context

The operator wants to watch every agent live, in a grid of terminals like firstmate's visible crew, with agents appearing as the coordinator needs them. They do not want the browser to poll. At `c124b56` the browser polls three endpoints (workspace 1 s, runtime 800 ms, evidence 1.5 s). tmux panes show only a short host log, because OMP runs as `--mode rpc --no-ui --no-pty`, and check output appears only after a check completes.

## Decision

1. **Live agent wall.** The run view is a grid with one terminal tile per live assignment and one per running check. Tiles follow the existing lifecycle: a coordinator team proposal appears as a placeholder labelled *Proposed · not started*; the tile goes live only when Crewroom admits the attempt and observes its process; a retired or replaced attempt collapses into a history strip, and its log stays readable. The grid can show any number of tiles; policy still limits a coding run to the coordinator plus three compatible specialists.
2. **Terminal stream.** Panes are 80×24. Crewroom attaches to its private tmux socket in control mode and forwards each pane's `%output` over a WebSocket to a `ghostty-web` terminal per tile, with xterm.js as the fallback. Closing or reloading the browser does not affect agents; `tmux -S <socket> attach` gives the same view outside the browser.
3. **Readable panes.** OMP keeps the validated RPC control contract. The host writes a formatted live stream into its pane: response text as it streams, tool calls, file edits and ownership events. Each admitted command's container output is followed live into a per-command log and shown in its own tile; it stays inside the ADR 0006 container boundary rather than moving to a separate tmux window. Running the native OMP TUI alongside RPC control is a separate spike, not an assumption.
4. **Push, not polling.** Run, assignment, attempt, message, steering and evidence changes are pushed over the same connection as cursor-numbered events. On reconnect the client sends its last cursor, receives the backlog through the R1 paged-event path, then resumes live. The WebSocket upgrade enforces the existing loopback-host and same-origin checks.
5. **Steering stays with the coordinator.** Tiles are read-only by default. Operators direct work through the coordinator conversation. A "take over" affordance that types into a specialist is deferred: today the pane runs the host, not an interactive OMP, so keystrokes have no defined meaning. If the native-TUI spike succeeds, take-over must be explicit, recorded as an operator intervention on that attempt, and pause coordinator dispatch to it until released. `tmux send-keys` remains excluded as the agent message transport.
6. **Durability.** Terminal output is written to per-attempt log files with a size/retention limit, not to SQLite rows. Terminal scrollback is never the source of truth for status, receipts or evidence; those stay in the workflow store.

## Consequences

- R3 becomes event-driven supervision and the live wall: the same tmux control-mode and RPC streams feed both the supervisor and the browser, replacing the 100 ms pump.
- R4 must keep high-volume terminal output out of the event tables; R7's allocated hosts map to one tmux session per run with a window per assignment.
- The ticket page is reorganised around the wall, the coordinator conversation and evidence. The polling loops in `src/` are removed.
- New failure modes to test: slow viewer back-pressure, viewer detach/reconnect, control-mode client loss, a long-running check, four concurrent agents and an intentional agent interruption.

## Implementation status

Implemented and tested with real tmux on 2026-09-28; see the [progress record](../testing/mvp-progress.md#u1-r4-and-the-r3-live-agent-wall--2026-09-28). `ghostty-web` 0.4.0 is the renderer; xterm.js has not been needed. Per-host independent supervision and a guarded real OMP 18.4.2/Rust Mail run are complete ([R3 record](../testing/mvp-progress.md#r3--event-driven-supervision-2026-09-28)). The native-TUI spike was negative. OMP's RPC mode has no `/collab` command. Only the interactive TUI can host a shared view, and that requires a relay and lacks the RPC control contract. Panes therefore keep the host's formatted stream, and take-over (decision 5) stays deferred.
