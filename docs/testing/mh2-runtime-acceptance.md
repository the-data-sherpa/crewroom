# MH2 runtime and Codex acceptance

FOR-7 adds native Codex execution behind the same Crewroom host, supervision, tool authority and external Mail workflow as OMP. Untagged historical assignments retain the OMP route and `ompPid`; Codex assignments retain `runnerKind`, `runnerPid`, thread identity, executable/profile fingerprint and capability facts. The assignment's model route is immutable across attempts.

## Native configuration

Use the existing Codex **0.162.0** ChatGPT file login. The private runtime JSON accepts this additional object:

```json
{
  "codex": {
    "binary": "/absolute/path/to/codex",
    "authFile": "/absolute/path/to/.codex/auth.json"
  }
}
```

The login must be a private, owned regular file. No separate Responses API key is required. Assignment processes receive ephemeral access tokens; only the trusted host's managed refresh helper can access the refresh login. Refreshes share a lock adjacent to the canonical login file across Crewroom runtime roots. The helper inherits that lease through exec, including after abrupt host death. Unrelated interactive Codex clients do not participate in Crewroom's lock; avoid rotating that login concurrently outside Crewroom.

Keep the existing root, socket, Mail, policy and tool/container configuration. For a native-only runtime, `ompBinary` and `models` may be omitted. Mixed runtimes retain the existing OMP provider definitions. Add a verified catalog entry such as:

```json
{
  "adapter": "codex",
  "provider": "codex",
  "id": "gpt-6-astra",
  "api": "codex-app-server",
  "inputs": ["text"],
  "efforts": ["low"],
  "tools": true
}
```

A role selects `codex/gpt-6-astra` with effort `low`. Production acceptance exercised this combination; the preceding synthetic compatibility spike also exercised `gpt-6.1-sol`. Other bundled model/effort combinations are checked against the pinned catalog but have no new live acceptance claim. Native model-picker integration belongs to MH4. This slice does not change the user's active runtime or role configuration.

## Authority, accounting and recovery

The restricted pinned Codex catalog disables native execution, patching, web, agents and other built-in effect paths. The app-server receives only Crewroom's addressed dynamic tools. Each assignment has separate host and runner OS processes, private configuration and durable thread history. Mail crosses the existing external service rather than an internal harness team bus.

Logical input and tool intents are persisted before dispatch/effects. Matching completed tool receipts must reconcile against retained Codex history on replacement. Unknown effects, missing history and unconfirmed input dispatch hold recovery; they never authorize automatic replay. All host tool errors currently hold native admission conservatively, including ordinary argument/read failures whose lack of an effect has not been classified. An operator must reconcile the original operation before re-admission; blind repeated Resume does not clear an unknown effect.

Native capabilities are explicit: cumulative token snapshots, unavailable request counts and cost, no gateway pre-request budget or hard output-limit enforcement, and harness-managed compaction. Snapshots survive restart and are deduplicated across attempts; recovered increments are attributed when first observed. Missing or in-flight usage remains partial/unavailable. Existing OMP gateway enforcement remains unchanged.

## Reproduce the disposable checks

Build `crewroom-process`, `crewroom-host` and the `codex-fixture` / `workflow-fixture` examples first. These commands use real native model access and may consume the existing account allowance; they perform no Jira or PR writes:

```sh
python3 scripts/verify_codex_adapter.py --output /tmp/mh2-adapter \
  --codex /absolute/path/to/codex --auth-file /absolute/path/to/auth.json
python3 scripts/verify_codex_workflow.py --output /tmp/mh2-workflow \
  --codex /absolute/path/to/codex --auth-file /absolute/path/to/auth.json \
  --mail /absolute/path/to/mcp-agent-mail
```

The full workflow driver requires Node >=24, tmux, Docker and Linux bubblewrap for its private disposable Mail server. It proves addressed live delivery, controller reattachment, stopped ownership and same-thread replacement without another message. Refresh tests use fake credentials plus a no-inference pinned-executable lease probe; deliberately expiring the user's real login is not part of acceptance.

The retained OMP fixtures use actual OMP 18.4.12 with synthetic provider replies. On Linux, install the pinned package in the ignored fixture directory, leaving global OMP unchanged:

```sh
npm install --prefix .crewroom/mh2-omp-fixture --no-audit --no-fund \
  --ignore-scripts @oh-my-pi/pi-coding-agent@18.4.12
cargo build --locked -p crewroom-runtime --bins --examples
python3 scripts/rust/verify-omp-rpc.py --output /tmp/mh2-omp-rpc
python3 scripts/rust/verify-omp-host.py --output /tmp/mh2-omp-host
python3 scripts/rust/verify-omp-supervisor.py --output /tmp/mh2-omp-supervisor
python3 scripts/rust/verify-omp-gateway.py --output /tmp/mh2-omp-gateway
```

Linux fixtures require Bun and bubblewrap. `CREWROOM_OMP_FIXTURE_MODULES` can select another private installation's `node_modules`. The fixture accepts OMP 18.4.12 (default) and 18.11.3 (FOR-52). To check 18.11.3, install it in a second ignored directory and point the suites at it. The supervisor suite needs a short `--output` path, because its tmux socket lives under it:

```sh
npm install --prefix .crewroom/omp-18.11.3-fixture --no-audit --no-fund \
  --ignore-scripts @oh-my-pi/pi-coding-agent@18.11.3
CREWROOM_OMP_FIXTURE_MODULES="$PWD/.crewroom/omp-18.11.3-fixture/node_modules" \
  python3 scripts/rust/verify-omp-supervisor.py --output "$XDG_RUNTIME_DIR/omp-supervisor"
```

`scripts/verify_omp_scripted_workflow.py --omp <installed omp> --mail <mcp-agent-mail> --output <new dir>` runs an installed OMP through `verify_catalog_workflow.py` with scripted loopback provider replies. It exercises the native picker, the production daemon and real Rust Mail. It does not exercise live inference.

The fixture guard exposes only system libraries, selected executables and disposable state, with private network/PID/mount namespaces. Allowlisted Unix relays connect allocated fixture ports to host loopback; no external IP route exists. The outer fixture wrapper retains the process ownership lease for inspection. The existing macOS sandbox remains available.

Executed, source-bound results and limitations are recorded in [MVP progress](mvp-progress.md) and the MH2 receipt linked there. Prior failed probes remain local evidence; their corrected results do not erase them.
