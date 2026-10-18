# MH3 Claude Code runtime contract

FOR-8 adds native Claude Code to the existing Rust runner interface, host tools, process ownership and external Mail workflow. It retains OMP and Codex routes and the single coding slot. The model picker remains FOR-9/MH4. This contract does not activate or change the user's installed Crewroom runtime.

## Configuration and identity

The verified Linux executable is Claude Code **2.1.296**, SHA-256 `24972e3bc859fab2b46ed4c1e51f7d6130f06d3bd550811a114640de3370d0de`. The host verifies the exact version and binds the executable hash plus launch policy into the assignment's profile. A different version fails admission; a changed executable/profile fails retained-session binding. No Node/Python SDK bridge is packaged: Rust speaks the pinned CLI's stream-json control protocol directly. Primary sources, inspected SDK/native source and probe details are in the [protocol report](../research/claude-mh3-protocol-2026-10-13.md).

Add this to the private runtime JSON, retaining the existing root, loopback socket, Mail, policy and containment configuration. Replace the example `binary` with the absolute path to your installed executable and `home` with the home directory containing your existing Claude login:

```json
{
  "claudeCode": {
    "binary": "/home/crewroom/.local/share/claude/versions/2.1.296",
    "home": "/home/crewroom"
  },
  "catalog": [{
    "adapter": "claude-code",
    "provider": "claude-code",
    "api": "claude-code-stream",
    "id": "sonnet",
    "resolvedModel": "claude-sonnet-5-5",
    "inputs": ["text"],
    "efforts": ["low"],
    "tools": true
  }]
}
```

A role selects `claude-code/sonnet` with effort `low`. Live acceptance covers this combination; other catalog entries are not an inference or access guarantee. Requested alias, observed model, effort, runner version, profile, session and host/runner PIDs remain separate facts. Initialize checks the offered alias/effort; startup inventory, every assistant message and pre-tool callback verify the frozen execution policy before host effects. Alias drift on resume requires explicit re-admission, never fallback.

`home` is the existing native login HOME. Its private, owned `.claude.json` supplies account/organization identity; only a digest enters Crewroom's private binding. Leave `CLAUDE_CONFIG_DIR` unset: pointing it at `.claude` changes the native account-metadata location. Crewroom does not read/copy refresh tokens or introduce an API-key requirement. Claude owns its normal credentials and refresh lock. Safe mode, empty settings sources, disabled native tools/Chrome/slash commands, strict SDK-only MCP and the frozen system prompt isolate ambient hooks, plugins, settings and project customizations. Each assignment gets a private cwd and native history directory; credential storage remains native and server-side.

The actual-executable fake-auth [receipt](mh3-claude-auth-2026-10-13.json) covers missing access, expired invalid-grant failure, two concurrent refresh clients, refresh-owner death before remote rotation, and launcher death. It uses fake credentials and loopback-only networking. Concurrent refresh is serialized by Claude; killing only its launcher leaves the native runner and refresh lease alive. Crewroom therefore stops its checked owned process group before replacement. Recovery after losing a real remote token-rotation response remains unverified; deliberate expiry of the operator's login is outside these tests.

## Tools, inputs and recovery

Only the existing addressed Crewroom host catalog is exposed through one SDK MCP server. Native shell, file, web and subagent tools are absent. A matching assistant intent, frozen model, per-call `PreToolUse` callback, session/cwd/effort, schema, current turn group, tool identity and MCP correlation are required before durable call intent and host dispatch. The existing host still validates attempt, assignment and workflow authority. Actual-executable synthetic-provider tests verify that `PreToolUse` runs even for an auto-allowed SDK tool; denial produces zero backend effects. Injected native Bash produces no filesystem effect. Result delivery uses the original logical call, never a new effect identity.

Queued input acknowledgment means accepted dispatch. It does not mean execution or completion. Claude can coalesce initial inputs or absorb steering into a running turn; the adapter tracks the known input group and settles each original host input from the terminal receipt. The queue is bounded to 16 outstanding inputs, below the native 64-ID terminal list cap. Default interruption preserves queued steering. An aborted text/tool turn remains distinct from successful completion; provider errors cannot become success merely because native `subtype` is `success`.

The shared transport bounds frames at 8 MiB, outstanding controls, buffered frames and writes. A partial physical output frame has ten seconds to finish; a silent idle session has no frame deadline. Malformed/stale/session-mismatched frames revoke admission. Buffered output or results after that rejection cannot mark an input complete. Original dispatch intent remains durable when dispatch is uncertain.

The assignment journal binds account digest, assignment, route/profile and tool catalog. Input and tool intents are saved before dispatch/effect; original result receipts are saved before delivery. A new attempt may start only after checked stopped ownership. It resumes the original session after the native history agrees with original inputs and effect receipts. Missing, incomplete, corrupted or changed history, unknown effects, undelivered receipts and conflicting owners hold recovery without replaying the operation.

**Recovery intentionally supports an unambiguous append-only native history chain.** It validates parent links and resume checkpoints. Compacted/relinked histories, unrelated sibling branches and stored `content-replacement` rows require reconciliation; they are not flattened into false receipt coverage. Native compaction can run, but automatic replacement after such a history transformation is not supported by this profile. Preserve the original journal/history and reconcile the hold; repeated Resume does not clear it. MH6 additionally verifies the normal Claude 2.1.296 streamed-tool batch: each result names its own tool-use block in the current assistant message, and both receipts survive resume. Other sibling branches remain held. No automatic replay or history deletion is offered as a repair.

## Accounting and enforcement

Claude's cumulative `modelUsage` tokens and list-price estimate are durable and deduplicated across turns and attempts. Recovered increments are attributed when first observed. Repeated snapshots do not add another charge; wrong-session/source reports are ignored by projections, and decreasing native counters fail the adapter contract. Missing counters, native request counts and provider/subscription billing remain unavailable. An absent reasoning counter is not measured zero.

Cost is a **list-price estimate**, not subscription charges or provider billing. Native request counts, pre-request budget enforcement and hard output/spend guarantees are unavailable. Compaction is harness-managed. Existing Crewroom active-time limits, authorization, pause/cancel and owned-process cleanup still apply. OMP gateway enforcement is unchanged.

## Reproduction and evidence

Build the runtime/service binaries and examples first. These opt-in commands use real existing model access, private assignments and real external Rust Mail; they perform no unrelated Jira or PR writes:

```sh
cargo build --locked -p crewroom-runtime -p crewroom-service --bins --examples
python3 scripts/verify_claude_adapter.py --output /tmp/mh3-adapter \
  --claude /absolute/path/to/claude-2.1.296 --native-home /absolute/native/home
python3 scripts/verify_claude_workflow.py --output /tmp/mh3-workflow \
  --claude /absolute/path/to/claude-2.1.296 --native-home /absolute/native/home \
  --mail /absolute/path/to/mcp-agent-mail
cargo test --locked -p crewroom-runtime --test claude_adapter -- --test-threads=1
```

The live workflow fixture requires Node >=24, tmux and Linux bubblewrap for its private Mail service. Shared native fixture implementations retain the Codex entry points and run the same process/recovery/Mail assertions. The [MH2 contract](mh2-runtime-acceptance.md#reproduce-the-disposable-checks) documents the retained actual-OMP synthetic-provider fixtures and Codex commands. Python/Node are test tools, not added packaged runtime dependencies.

Source-bound execution results, exact commands, failed diagnostics and platform limits are retained in [MVP progress](mvp-progress.md) and the MH3 runtime receipt linked there. The Linux CLI evidence does not certify a new macOS package, every model/effort combination or live Jira-to-PR publication.
