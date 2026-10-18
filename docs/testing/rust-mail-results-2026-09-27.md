# Rust Agent Mail and tmux contract results

Executed 2026-09-27 on macOS ARM64. **14 distinct checks passed; none failed.** Nine exercise the real mailbox API and five exercise real independent OMP processes hosted in tmux. One mailbox check deliberately characterizes retry-key expiry; its pass does not mean duplicate protection is unlimited.

**Decision:** use Rust Agent Mail as the preferred mailbox for the next Crewroom integration slice. The candidate met this bounded contract, so no fallback was tested and no custom mailbox was built. Production supervision and full Jira-to-PR acceptance remain outstanding.

The [machine-readable report](rust-mail-results-2026-09-27.json) contains individual results, source hashes, process/session identities, model observations, guard checks, and cleanup evidence. These experimental adapters are not production workflow services.

## What ran

- Published Rust Agent Mail **v0.3.36**, downloaded into ignored experiment state. The ARM64 archive matched SHA-256 `2b90585878dd3c8509c21b177450f9f975caebb27a2f6a8601efed0ffb016523`; the executable hash is recorded in the JSON report. [Release](https://github.com/Dicklesworthstone/mcp_agent_mail_rust/releases/tag/v0.3.36).
- Installed **OMP 18.3.1** and **tmux 3.7c**. Each pane ran a test host with a separate OMP RPC child. Native OMP team messaging was excluded.
- Scripted loopback provider responses across three API families. The final combined run made **18 scripted provider requests**. No live model availability or response quality was evaluated.
- macOS OS-level network restrictions allowed only allocated loopback ports and denied external connections, other loopback destinations, and child-process external connections. The test processes used synthetic configuration and credentials; no user provider configuration was copied.
- Disposable mailbox data, sessions, tmux socket, and per-process logs. Rust Mail's ATC operator was disabled. Mailbox credentials remained in the test controller.

## Mailbox results

| Check | Result and evidence |
| --- | --- |
| Store, observe, acknowledge | Message body and identity retrieved through MCP; repeated event observation and `mark_read=false` inbox reads leave recipient state unchanged; explicit acknowledgement is recorded |
| Repeat a keyed send | Same message ID returned; exactly one matching inbox message |
| Reuse a key with changed content | Explicit idempotency conflict |
| Concurrent retry | Four concurrent requests return one message ID and create one recipient event |
| Lose send result and kill mailbox | MCP result body discarded; independent recipient read confirms persistence; SIGKILL and restart; retry returns original message **4**, with one message and one event |
| Acknowledge and restart | Acknowledged receipt survives SIGKILL/restart and repeated acknowledgement |
| Resume observer cursor | Seven messages recovered across pages of two after saving a cursor and restarting the service; no missing/duplicate events or consumed recipient state |
| Invalid destination/credential | Unknown recipient rejected; invalid service bearer rejected |
| Expire retry keys | Separate two-second retention fixture creates a new message after expiry and restart, as documented; both original and later messages remain readable |

The executable groups same-key replay with payload conflict, and splits invalid destination and invalid credential into separate cases: nine mailbox cases total. Normal cases explicitly configure 24-hour retry retention. After that window, Crewroom must reconcile retained logical identity rather than blindly resend. Message storage and duplicate-prevention retention are different facts.

This fixes the particular mailbox behavior observed with local Relaycast 8.13.0: the equivalent unexpired-key restart test produced two different message IDs there. See the [prior results](results-2026-09-26.md). It does not repair unrelated OMP settings or session behavior.

## Independent OMP and tmux results

| Role | Assigned and observed selector |
| --- | --- |
| Coordinator | `aperture-openai/openai.gpt-6-sol` |
| Specialist 1 | `aperture-anthropic/anthropic.claude-sonnet-5` |
| Specialist 2 | `aperture-google/gemini-3.8-flash` |

These selectors came from the saved local catalog. Each matched both OMP state and the request received by the scripted provider.

1. **Process topology and routing:** three distinct OMP PIDs, each parented by its pane's host PID, with distinct sessions and the assigned model.
2. **Four-hop exchange:** coordinator → Specialist 1 → Specialist 2 → Specialist 1 → coordinator. Actual OMP host-tool calls invoked the mailbox adapter; recipient OMP turns received stored content. All four messages were acknowledged and visible through the passive event API. No OMP internal team event was observed.
3. **Terminal observation:** two real tmux control clients attached and detached. Pane captures contained actual host/RPC activity, agent sessions stayed the same, and observer activity did not consume unread messages. This is a rendered RPC activity view, not OMP's native interactive TUI.
4. **Held dispatch:** stored mail made no provider request until the test adapter explicitly resumed the recipient; resume made exactly one request. The mailbox does not independently wake or pause OMP. Production pause policy remains a Crewroom responsibility.
5. **First-turn recovery:** a fresh OMP process was killed while its first model response was held. Its input was RPC-visible but absent from the session file, reproducing the existing runner gap. Rust Mail retained the unacknowledged input. After confirming termination, the test launched a fresh process/session, delivered the retained body once, observed its response, and acknowledged the mail. This establishes explicit replacement using saved input, not automatic general recovery of unknown side effects.

## Limits and remaining work

These results support the mailbox choice and tmux/RPC host arrangement. They do not establish:

- Confirmed pause during a real shell/tool child, escaped-descendant handling, or recovery while an old writer may still be alive.
- Supervisor crash/reconnect ownership, automated bounded repair, or recovery of an external action with an unknown outcome.
- Strict model/effort preflight. Previously observed OMP effort normalization remains unchanged.
- Crash at every internal database/archive write boundary, disk-full behavior, or complete backup restoration.
- Production observer authorization. This test proves non-consuming observation using a controller-held credential; it does not prove that credential is read-only.
- Browser workbench behavior, live Jira/Bitbucket writes, the pilot repository coding, independent code review, or the twelve complete ticket scenarios.

The proposed [combined runtime tests](../archive/complete-local-runtime-proposal-2026-09-27.md#next-combined-contract-experiment) still contain outstanding supervision and coding-workflow cases. The broader [offline suite](results-2026-09-26.md) retains its recorded failures/blockers; these new results must not be used to label the entire application green. Current vendor license facts remain in the [candidate assessment](../archive/agent-inbox-followup-2026-09-27.md#license-facts).

## Reproduction and evidence

On this macOS ARM64 workspace, with the recorded OMP and tmux executables:

```sh
python3 scripts/prepare-rust-mail-test.py
python3 scripts/test-rust-mail-offline.py --output .crewroom/test-results/rust-mail-new
python3 scripts/test-rust-mail-omp-tmux.py --output .crewroom/test-results/rust-mail-omp-tmux-new
```

Use fresh output paths. Preparation downloads a pinned release when absent, verifies its archive and extracted executable, and does not install globally. Test execution itself uses only the guarded local services.

Canonical raw evidence is in ignored `.crewroom/test-results/rust-mail-contract-1/` and `.crewroom/test-results/rust-mail-omp-tmux-verified/`. The combined fixture first passed in `rust-mail-omp-tmux-1/` and was rerun after strengthening the explicit resume-call-count assertion. The initial API discovery encountered a Python test-client keyword collision; that client error was fixed before acceptance runs and was not a Rust Mail failure.

All owned OMP/host/mailbox processes and the dedicated tmux server exited. Disposable OMP executable/addon copies were removed. Pinned Rust Mail downloads and test logs remain in ignored local experiment state for reproduction. No user tmux session or global agent configuration was changed.

Only experimental scripts and documentation changed. Application domain, TypeScript, and browser code were untouched; no new application build or browser acceptance is claimed.
