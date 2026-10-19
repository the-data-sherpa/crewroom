# Recorded harness transcripts (FOR-72)

Crewroom's adapter tests mostly use hand-written protocol fakes (for example
`crates/crewroom-runtime/tests/fixtures/claude-rpc.py`). A fake shows how Crewroom
handles a frame. It cannot show what a new Codex, Claude Code or OMP release
sends. Recorded transcripts close that gap.

## What a transcript is

`crates/crewroom-runtime/tests/fixtures/harness-transcripts/<harness>/<scenario>.jsonl`
holds one header line, then each protocol line the harness and Crewroom
exchanged, in order.

- **Header:** harness, `--version` output, executable identity (file name,
  SHA-256, size), scenario steps, the executable's non-session calls (for
  example `--version` and Codex's bundled model catalog, trimmed to the assigned
  model), and the adapter events the recording produced (`expect`).
- **Entries:** `start` (argv), `send` (Crewroom to harness), `recv` (harness to
  Crewroom), `file` (Claude Code session history that a later resume reads)
  and `exit`.
- **Placeholders:** `<<root>>` stands for the recording directory,
  `<<uuid:N>>` for generated identifiers, `<<provider>>` for the loopback
  provider and `<<redacted>>` for credential and account fields.

Scenarios for each harness:

| Scenario | Covers |
| --- | --- |
| `tool-completion` | launch, host tool call allowed and answered, completion |
| `steer-interrupt` | launch, streaming output, steering input, interrupt |
| `reattach` | tool turn, close, restart on the same session (`recoveryMode: session`), new turn |

## Recording

The recorder runs the **real installed executable** through Crewroom's
production adapter. It uses the `codex-fixture` example, which calls
`runner::open`, `submit`, `resolve_tool`, `interrupt` and `close`. A tee shim
stands in for the executable and logs every line in both directions. Model
output comes from a synthetic loopback provider (Anthropic Messages or OpenAI
Responses streaming). No operator login, account, repository or private prompt
is used:

- Claude Code gets a synthetic HOME and `ANTHROPIC_BASE_URL` with a
  non-credential API key.
- Codex gets a synthetic ChatGPT token file and its base URLs pointed at the
  provider.
- OMP gets a provider definition named `crewroom-fixture`. Do not name it
  `synthetic`: OMP has a built-in provider with that name.

Recordings run in `~/.cache/crewroom-transcript-recordings`. That directory is
outside any Git checkout, because harnesses report repository metadata such as
remote URLs, and it is not on tmpfs, because harness caches are large. The
directory is removed after a successful recording.

```sh
cargo build -p crewroom-runtime --bins --examples
python3 scripts/harness_transcripts.py record --harness claude-code --binary "$(command -v claude)" --scenario tool-completion
python3 scripts/harness_transcripts.py record --harness codex --binary "$(command -v codex)" --scenario reattach
python3 scripts/harness_transcripts.py record --harness omp --binary "$(command -v omp)" --scenario steer-interrupt
```

Without `--output`, the recorder replaces the committed transcript for that
harness and scenario. `--keep` retains the private recording directory for
inspection. Never commit that directory.

## Redaction gate

Before the recorder writes a transcript, it normalizes paths, host name, user
name and UUIDs, and replaces every credential or account field with
`<<redacted>>`. It then applies the same gate as
`python3 scripts/harness_transcripts.py check`. The gate fails on:

- home paths
- JSON web tokens
- API keys
- bearer credentials
- email addresses
- raw UUIDs
- an unredacted credential or account field
- a transcript that is not declared synthetic
- a task without the `Synthetic fixture` marker

The gate reports locations only and never repeats the matched content. `npm test`
runs the gate over every committed transcript through
`scripts/test_harness_transcripts.py`.

## Replay (simulated)

`cargo test -p crewroom-runtime --test harness_replay` puts
`tests/fixtures/harness-replay.py` in place of the executable. The peer emits
the recorded harness lines. Before each recorded Crewroom line, it reads the
adapter's line and compares it:

- A different frame kind (`type`, `method`, `subtype`) is a hard failure.
- Other differences are logged as client drift.
- Independent client lines may arrive in a different order.
- Placeholders bind to the values the live adapter chose, such as request IDs
  and session IDs, so recorded replies answer the live adapter. This also works
  across the restart in `reattach`.

The test drives the production adapter through the recorded steps. It then
requires that each session reached the end of its transcript and that the
adapter's events match the recording per type: opened (with recovery mode),
agent start and end, host tool calls, prompt results with status, policy
rejections and message ends. A changed harness frame (for example another model
in an assistant message) and a changed client frame kind each fail the replay
(`a_changed_harness_frame_fails_the_replay`).

Replays are **simulated checks**:

- The test prints `SIMULATED replay (not live evidence)` for each transcript.
- `CREWROOM_REPLAY_REPORT=<file>` appends rows labelled `"evidence":"simulated-replay"`.
- A replay never counts as live delivery or as a live harness run.

The recordings come from real binaries with a synthetic provider. They show
protocol behavior, not model behavior.

## Version changes

Re-record on the new version, then compare:

```sh
python3 scripts/harness_transcripts.py record --harness codex --binary /path/to/new/codex --scenario tool-completion --output /tmp/new.jsonl
python3 scripts/harness_transcripts.py diff crates/crewroom-runtime/tests/fixtures/harness-transcripts/codex/tool-completion.jsonl /tmp/new.jsonl
```

`diff` reduces each frame to its shape. Frame kinds, methods, states and
statuses keep their values; other values become their JSON type. It prints a
unified diff of the version, executable identity, non-session calls, adapter
event counts and the frame sequence.

Re-recording on a new installed version replaces the committed transcript. The
Git diff of that file is the reviewable change.
