# Final native release validation

Use this plan only after root supplies a tested, pushed **combined** revision containing the UI, terminal, historical migrations and installation work. Do not perform an expensive release build against an earlier component branch. This plan does not authorize moving the retained pilot, changing the repository default, or retiring the browser before acceptance.

The accepted [M7 contract](mvp-plan.md#m7--mvp-release-and-repeatability) says the app starts and recovers through documented commands. [ADR 0009](adr/0009-rust-native-workbench-and-service.md) requires an independent persistent daemon. Neither requires Finder-only provisioning. The supported installation entry point is the bundled `crewroom-launcher`; Finder opens the client only. This distinction must remain explicit in the final release note.

## Current release scope

The 2026-10-04 [native release record](release-native.md) records final acceptance of the personal operator workflow. The operator deprioritized VoiceOver, and actual Japanese IME validation remains optional compatibility work beyond the accepted basic keyboard/layout contract. Both OS checks remain unverified; their failed tool-limited attempt is retained. Required keyboard, narrow-layout, draft, terminal, process, evidence and recovery gates remain unchanged. The gate order below is the reusable validation procedure, with these optional compatibility checks excluded from blocking cutover.

## Gate order

| Gate | Exact evidence and completion condition |
| --- | --- |
| Freeze | Full combined commit, exact remote equality, clean tracked source, dependency/tool versions and source manifest. Root reconciles all 87 cases/12 scenarios and native equivalents of the 31 browser journeys; unchanged source can reuse explicitly matched earlier receipts. |
| Fresh release checkout | New local clone with neither `target` nor `node_modules`, pinned bootstrap, one locked release build and release workspace test run. No terminal-lab path may supply a shipping dependency. Keep all failed logs. Existing macOS/CLT/Rust/OMP/tmux/Mail/Docker provisioned tools are disclosed; this is not a fresh OS installation. |
| Package | Four native executables, nonbuilding launcher, dependency notices, system-only dynamic library checks, deep strict ad-hoc signature verification. Verify **final hashes from the adjacent `.app.receipt.json`**, not pre-signature hashes inside Resources. Record on-disk bundle bytes independently of runtime memory. |
| Command installation | Launch from `/` using explicit absolute private data/role paths, including spaces; no Rust/Zig/Node on runtime PATH. Start fresh, save configuration/task, reopen the same service, stop exact ownership, restart and recover without source or state repair. Runtime/provider configuration remains private and explicit. |
| Whole-state recovery/repeat use | Run production bundled daemon with real OMP/Mail/Git/Docker and scripted models. Coding evidence → verified stopped inventory → complete original-path restore → retained evidence/steering → separate completed research. Require all process receipts stopped and command cleanup confirmed. |
| Failed service and ownership | The runner repeats baseline/heterogeneous/rotated legal four-agent profiles against the final bundled daemon, host and helper. It verifies actual model/effort identities, four external Mail hops per profile, daemon SIGKILL with the same eight host/OMP identities reattached, then paused shutdown and exact cleanup. Reports bind each executable hash to the final signed sidecar; providers remain scripted. Bind other actual native interrupted-controller/host/Mail/Docker and checked-Git scenarios to the combined source. Preserve unknown ownership and prove exact descendant containment. Graceful SIGTERM/restart is not evidence of SIGKILL recovery; do not relabel it. |
| Native app and footprint | Root coordinates the actual release app: open through the bundled command, close/reopen without stopping service, workflows/terminal/accessibility and input under sustained output. Measure GUI, daemon and explicit host/OMP PIDs separately at idle and under the same representative workload. Measure Mail and Docker/VM separately; report attribution limits and machine/cache conditions. |
| Rollback and cutover | Preserve the selected installation's stopped **pre-cutover whole-state snapshot plus matching old build** and verified inventory. Document restoring both at original paths and preserving failed/newer state. Same-build recovery is not a downgrade proof. Root records exact release revision, evidence, unsupported distribution/platform boundaries, and only then switches the default/retires runtime. |

## Execute the headless release gates

The runner refuses absent combined-source components, an existing destination, or a validation script that differs from the candidate. Plan mode is read-only. It performs no GUI launch, default switch or live provider operation.

```sh
export PATH=/opt/homebrew/opt/node/bin:$PATH
release_candidate=FULL_COMBINED_40_CHARACTER_SHA
release_root=/private/tmp/cr-native-final
mail_executable=/absolute/verified/mcp-agent-mail
scripts/validate-native-release.sh --plan "$release_candidate" "$release_root" "$mail_executable"
scripts/validate-native-release.sh --execute "$release_candidate" "$release_root" "$mail_executable"
```

The root path must be new and short enough for macOS tmux sockets. The script creates its own clone, private toolchain, release binaries/package and fixture state; it does not reuse a primary data directory. `headless-release.json` lists passed checks and still-pending gates. Native domain/process/provider coverage remains root's explicit acceptance matrix; this runner does not pretend its installation fixtures and three bounded profile checks replace those contracts. Retained domain/browser checks need exact source/dependency receipts or a fresh run on an isolated port.

The fixture Mail binary must be the characterized executable only; copy no private Mail configuration or live database. Credentials and the installed OMP binary/native addon are never copied from a real runtime configuration. Synthetic fixture configuration supplies all model responses.

## Launch and measure the actual release app

After coordinating use of the native GUI, use the emitted app path:

```sh
"$release_root/Crewroom.app/Contents/MacOS/crewroom-launcher" \
  --data-dir /absolute/private/release-validation-data \
  --roles /absolute/private/release-validation-roles/roles.json --port 4340
```

Without `--headless`, the launcher opens a new window connected to its selected service. Reopen reuses an authenticated existing service and explicitly reports that its existing role/port/runtime configuration remains in effect; it does not claim to reconfigure or replace it. To change the binary or configuration, stop the exact service first. A readiness timeout preserves state and reports its private log rather than killing an unknown process.

Record daemon PID from the private authenticated connection descriptor and GUI PID from the launched bundle's observed OS process. Select host/OMP PIDs from their exact inspected process receipts, not a name prefix. Measure only the known set:

```sh
python3 scripts/measure-native-footprint.py --seconds 10 \
  --process gui=GUI_PID --process daemon=DAEMON_PID \
  --bundle "$release_root/Crewroom.app" --output /absolute/new/idle-footprint.json
```

Repeat with each observed host/OMP/helper PID during the same recorded active workload; include the coordinator and three specialists for the four-agent workload where that scenario is used. Record actual activity, sustained output, visible responsiveness, reconnect and window-close behavior. An idle process is not a completed task. The collector reports RSS and cumulative CPU-time deltas, rejects changed process identities and never stops processes. RSS sums double-count shared pages and are not PSS or whole-machine memory. Docker Desktop's VM and external Mail have their own measurements and must not disappear into an unlabeled total. CLI launch timing is recorded by the launcher fixture; measure GUI readiness separately. No memory reduction, latency target or release-performance claim follows from binary size alone.

## Hold a real four-agent fixture for GUI and resource observation

After the combined source is frozen and packaged, coordinate the GUI slot and run:

```sh
python3 scripts/rust/verify-release-profiles.py --profile baseline --observe \
  --binary-directory /absolute/Crewroom.app/Contents/MacOS \
  --mail-binary /absolute/verified/mcp-agent-mail --output /private/tmp/cr-observe
```

The output root must be new. This is the production bundled daemon, four actual OMP processes with separate hosts, and actual private Rust Mail. Model replies are scripted. Normal profile runs omit `--observe` and retain their existing behavior.

Wait for private `observation-ready.json`. It identifies the descriptor, task/run, exact live host/OMP identities, tmux socket and labelled daemon/Mail/tmux/host/OMP PIDs with observed owner/start/command text. Harness PIDs are separate. Use its `launcherArguments` with the bundled launcher to reopen that same service and its `appArguments` for direct app launch. Observe the GUI PID independently. Select the recorded ticket/run and open its read-only agent wall; no facade substitutes for the real service.

After idle sampling, create the owned regular file `<root>/start-load`. `load-started.json` marks a 60-second target of harmless Unicode replies streamed through all four model gateways/OMP processes. `load-progress.json` reports rounds; `load-complete.json` records elapsed time, every input/completion ID, round counts and errors after all four agents settle. Sample active resources during this window and idle/reopen afterward. This workload does not execute Docker commands; any Docker VM measurement remains separately attributed. Synthetic token accounting does not establish real model cost.

Create `<root>/finish-observation` when GUI sampling is complete. Each control-file wait has a 180-second deadline; do not leave the fixture waiting for open-ended interaction. Node has a 540-second test deadline and the outer driver a 600-second bound. Errors are retained in `observation-error.json`; a timed-out or forcibly cleaned run fails. The fixture then runs its existing actual daemon SIGKILL, same-eight-process reattachment, Pause, shutdown and exact ownership cleanup. Wait for the final `report.json` and a successful process exit before accepting the exercise. Closing the GUI alone does not finish or stop the fixture.

## Rollback decision

Retain a matching old executable/source revision and the whole stopped installation, including repositories/Git common directories, configuration/roles, evidence/worktrees, sessions/journals/receipts/budgets and Mail database/archive. Verify content, symlink targets and modes before accepting the snapshot. Preserve newer state separately before restoration. Never lower SQLite `user_version` or run an older build against post-upgrade state.

The completed N7.1b fixtures establish same-build whole-state restore. A claimed old-build downgrade needs a separate disposable exercise with the matching pre-cutover state and executable; otherwise report rollback as a documented procedure with its inventory verified. Before enabling integrations after rollback, reconcile any remote activity since the snapshot using preserved receipts and provider readback. An old snapshot must not silently repeat newer external writes. The retained operator-merged pilot remains paused and outside this cutover.

Distribution signing/notarization, Finder provisioning and other platforms remain explicitly unsupported. The accepted launch command and local ad-hoc package are sufficient for this local release; they must not be described as a notarized installer.
