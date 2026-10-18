# Find a feature to verify

Start here when assigned an operator feature name. Run commands from the repository root. The [manifest](manifest.json) is a lookup and freshness map into existing acceptance cases; it does not define a second acceptance catalog.

| Feature name (accepted aliases) | Recipe | Existing case subset |
| --- | --- | --- |
| Steering across reconnect (steering, reconnect) | [steering-reconnect](steering-reconnect.md) | UI-02; steering/reconnect portion of E2E-05 |
| Pause/resume (pause, resume) | [pause-resume](pause-resume.md) | UI-04, LIF-05; Resume portion of LIF-06 and E2E-05 |
| Readiness invalidation after a revision change (readiness, revision drift) | [readiness-invalidation](readiness-invalidation.md) | EVD-04; revision-drift portion of E2E-09 |

```sh
python3 scripts/verify-feature.py list
python3 scripts/verify-feature.py check steering-reconnect
```

Read the selected recipe before running it. Its assertions cover the named subset, not every step of its linked case. [suite.json](../suite.json), [scenarios.json](../scenarios.json), the [native reconciliation](../rust-native-acceptance-reconciliation-2026-10-04.json) and [operation inventory](../../native-workbench-operation-map.md) remain authoritative. Historical specification statuses in the original suite are not current execution results.

## Supported surface and setup

The initial recipe surface is **Linux source/debug, headless GPUI test interaction, production Rust daemon**. GPUI renders the real workbench controls and dispatches native input through the test platform. This is native interaction evidence; it does not certify OS mouse/keyboard input, GPU display, terminal/IME/accessibility behavior, packaged release builds or another platform.

Install repository dependencies using the [Linux build guide](../../linux.md) before verification: Python ≥3.12, Node ≥24 with the lockfile-installed `node_modules`, Rust and locked dependencies, the pinned Zig/native libraries, Git and tmux. `check`/`run` report missing prerequisites and do not install dependencies, refresh authentication or pull images. No live model, Jira, Bitbucket or Rust Mail login is required: RPC/model and Mail replies are explicitly scripted local fixtures. The daemon, UI, Git and owned OS processes are real.

On this development machine, select its already installed supported Node:

```sh
export PATH="$HOME/.local/share/mise/installs/node/26/bin:$PATH"
node --version
python3 scripts/verify-feature.py build --evidence-dir .crewroom/test-results/for12/build-01
```

`build` is an explicit setup action. It invokes the existing pinned native bootstrap and locked Cargo build, then records current native source inputs and binary hashes in `.crewroom/feature-verification/build.json`. Bootstrap can download its pinned Zig archive if absent. A source hash plus an old executable does not count as a rebuilt application. After relevant native source changes, repeat this build step; unrelated documentation changes do not require a native rebuild.

Readiness invalidation additionally requires the local Docker engine at `/var/run/docker.sock` and a pinned Linux image with `/bin/sh`, `cat` and `test`. The recorded machine already has this Node 24 image; selecting it neither pulls nor changes it:

```sh
export CREWROOM_CHECK_IMAGE=sha256:3638d9a6fe4030bd716be989438248074489337ba3275657f93595428be4fc03
docker image inspect "$CREWROOM_CHECK_IMAGE" --format '{{.Id}}'
```

If unavailable, record that result before choosing an explicitly provisioned local image. Steering and pause/resume do not need Docker. Use a new evidence directory for each attempt:

```sh
python3 scripts/verify-feature.py run steering-reconnect --evidence-dir .crewroom/test-results/f12s1
```

Keep run directory names short: the absolute path plus `fixtures/cr-wf-XXXXXX/runtime/tmux.sock` must be at most **107 encoded bytes** on Linux. The short examples above fit this checkout. For a longer checkout prefix, choose a fresh `/tmp/<short-name>` directory and archive its private evidence after cleanup if long-term retention is needed. `run` reports an overlong path as unavailable before launching the daemon; choose a shorter **new** directory. Build evidence directories do not carry this socket limit.

Run recipes sequentially. Preflight must find the documented source entry point, exactly one Node recipe and exactly one native driver, verify source/binary binding and confirm selected prerequisites before admitting the journey. Each UI action additionally checks authenticated health, selected fixture task/run and fresh native observations. Silence or a zero-test exit is not readiness.

## Evidence, cleanup and reruns

Each invocation records recipe/manifest/helper hashes, source commit and dirty patch hash, native source and executable fingerprints, OS/architecture/toolchain, exact arguments and timing, selection/assertion counts, journey result and cleanup. Native action artifacts retain input plans, rendered labels, durable observations and test output. The isolated Git identities (target, candidate head/base/tree) differ from the application's source/build identity.

Fixtures live below the new private evidence directory and create their own configuration, SQLite database, repositories, loopback endpoints and tmux socket. They use synthetic tickets and credentials. Their private connection and process descriptors are retained for cleanup; do not publish raw fixture directories or descriptor tokens. Commit only sanitized summaries plus hashes/paths to private artifacts.

Normal execution closes the owned daemon and verifies its exit plus host/runner identities, UI exits, containers and tmux resources. Evidence remains in place. The recovery operation accepts only the registered receipt for that verification:

```sh
python3 scripts/verify-feature.py cleanup --receipt .crewroom/test-results/f12s1/receipt.json
```

Cleanup is idempotent and restricted to the named run. It never targets the active installation, global tmux sessions, arbitrary PIDs or another receipt's resources. Unresolved ownership or recovery cleanup leaves the original journey incomplete; cleaning up cannot turn failed evidence into a pass.

Receipt outcomes are `passed`, `failed`, `unavailable`, `incomplete` and `stale`. A pass requires every documented native/durable assertion, one selected/executed journey and verified owned cleanup. A guard check may successfully detect `unavailable` or `stale`; its target journey still did not pass.

Preserve failed attempts. Classify each failure as recipe/helper defect, unavailable environment, product regression or uncertain ownership before rerunning. Fix only the bounded cause and rerun affected recipes in new directories. After two unsuccessful reruns of the same classified failure, stop that lane and record the next bounded action. A changed recipe, selector or relevant shared helper makes its prior run historical until executed again. Never replace a missing native action with service-only or browser evidence.

## Freshness drill and independent execution

For FOR-12 acceptance, a fresh specialist receives the feature names and this repository context, discovers the recipes here, and runs all three without private setup coaching. Preserve its initial assignment and findings. A question that requires a private answer identifies a documentation defect: fix the recipe before another fresh attempt.

The stale-entry-point drill uses a disposable copy of `manifest.json`: change one `nativeSelector` to a nonexistent test, run preflight and retain the `stale` result with no journey started, correct that copy to the versioned selector, then rerun the affected native recipe. Pass the copy before the subcommand: `python3 scripts/verify-feature.py --manifest <copy.json> check steering-reconnect`, then the same prefix with `run steering-reconnect --evidence-dir <new-directory>`. Record both manifest identities and results. Static correction alone does not complete the drill.

Implementation/acceptance state lives in [status](../../status.md) and [revision-bound progress](../mvp-progress.md). Until a final receipt is linked there, these recipes are under implementation and unverified. The [FOR-12 plan](../for-12-implementation-plan.md) sets the bounded validation and follow-up order; FOR-13 and FOR-14 remain separate work.
