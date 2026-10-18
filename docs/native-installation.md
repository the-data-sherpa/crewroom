# Native installation and recovery

The accepted release validation environment is Apple Silicon macOS. The [Arch Linux / Wayland guide](linux.md) describes the additional source-build path and its separately bounded validation. macOS bundle/signing instructions below apply to macOS; Linux produces an unsigned `Crewroom/bin` directory package. The historical upgrade and headless operations evidence is in [the upgrade receipt](testing/rust-native-upgrades-2026-10-03.md) and [the operations receipt](testing/rust-native-upgrades-operations-2026-10-04.md). The [native release record](release-native.md) records final desktop acceptance, optional unverified OS compatibility, measured footprint and the repository default switch. The retained browser launcher remains available under explicit legacy commands.

## Build and package

Install the Apple Command Line Tools, Rust through rustup, Python 3.12 or newer, and Git. The checked-in `rust-toolchain.toml` selects Rust 1.97.1 without changing rustup's global default. Node is needed for the retained validation tools, not the packaged native runtime.

```sh
python3 scripts/bootstrap-native.py
scripts/run-native.sh --release --build-only
```

The bootstrap downloads the official Zig 0.16.0 Apple Silicon archive to a private checkout-local toolchain directory, verifies its pinned SHA-256 before extraction, checks the executable version and records a complete file hash inventory. Reuse verifies that inventory. `--directory /absolute/private/path` selects another toolchain directory. A changed installation fails verification; preserve it and choose a fresh directory instead of bypassing the check. The terminal adapter's pinned upstream source and patch/license records are in `vendor/gpui-libghostty/CREWROOM.md`. Cargo and Zig may fetch their locked build dependencies on first build. A full Xcode installation is not required; the Command Line Tools SDK is.

`--build-only` does not open a window or start a daemon. It prints a unique immutable `Crewroom.app` path. The bundle contains adjacent `crewroom`, `crewroomd`, `crewroom-host`, and `crewroom-process` executables. Packaging verifies native arm64 binaries and refuses non-system dynamic libraries that have not been bundled. It includes dependency license notices, source provenance and a local ad-hoc signing receipt. The adjacent `.app.receipt.json` sidecar records final signed executable hashes; the internal Resources receipt labels pre-bundle-signature hashes explicitly. Ad-hoc signing is not distribution notarization. Zig and Rust are build dependencies only.

To package an already-built binary directory without rebuilding:

```sh
python3 scripts/package-native.py --binary-directory target/release --output /absolute/new/path/Crewroom.app
```

Packaging refuses an existing destination. Keep old bundles until their running processes have stopped. A packaging receipt identifies its inputs; acceptance still requires exercising that exact combined bundle.

## Start an explicit installation

Without `--build-only`, `scripts/run-native.sh` builds and invokes the nonbuilding bundled launcher, which starts the daemon and opens the app with its private connection file. `CREWROOM_NATIVE_DATA_DIR` selects the data directory; its current default remains this checkout's `.crewroom/rust`. Use an explicit disposable directory for testing. Merely opening the app bundle does not currently provision a daemon or select a new installation.

After installation, launch without any build tool or repository checkout:

```sh
/path/Crewroom.app/Contents/MacOS/crewroom-launcher --data-dir /absolute/private/data --roles /absolute/private/roles.json
```

Add `--headless` to start/reopen only the service, or `--port` to select its initial loopback port. The private data/role directories must be owned by the current user with no group/other access. Reopen reuses the authenticated existing service and clearly retains its existing role, port and runtime configuration; changing them requires a stopped restart. The launcher never kills a service on readiness failure. Keep the role file outside project instructions. Runtime configuration remains server-side: `CREWROOM_RUNTIME_CONFIG` points to private JSON; absent configuration reports execution unavailable. Execution requires real OMP, tmux, Rust Agent Mail, model access and a running Docker engine with configured pinned images. The native tmux socket must be inside the configured runtime root and fit the macOS path limit. Keep the Mail service loopback-only and bearer authenticated. Read [the operations inventory](operations.md#configuration-inventory) for the runtime/provider contract. No fixture credential establishes live Jira or Bitbucket access.

The daemon binds to loopback and writes an operator-only `native-connection.json`. Native API access requires its bearer token; mutations also require the exact own origin. Stop the exact daemon with SIGTERM and wait for clean exit before stopping its separately managed Mail service. Never infer stopped ownership from a quiet terminal.

## Upgrade and backup

Native startup upgrades actual schema versions 1 through 25 in ordered transactions to schema 26. It refuses newer databases and nonempty unversioned stores. Back up before opening old state. Each completed version commits separately, matching the retained migrator; failure rolls back the failing version. Do not lower `user_version` to simulate or undo an upgrade.

Use the whole-state [same-path backup procedure](operations.md#backup-and-same-path-restore). Include data/evidence/worktrees, repository Git common directories, runtime hosts/journals/sessions/receipts/budgets, reusable roles, private configuration, and Mail database plus archive. Stop every owned process and both services before capture. Preserve remaining SQLite WAL/SHM files, permissions and symlink targets. A database-only copy is incomplete. A code rollback also needs the matching whole-state snapshot.

Tmux can leave its socket file after its last session exits. A Unix socket is not durable recovery state. Before excluding or removing the exact configured private socket, verify every owned host/OMP receipt is stopped and that `tmux -S /exact/private/socket list-sessions` reports no server running. Any live or uncertain resource blocks capture. Never kill a shared tmux server to simplify a backup.

Preserve the old state separately, restore every component to the original absolute path, verify the entire inventory, then restart Mail and the matching daemon. Inspect recovery holds, requirements, steering, queue, messages, evidence and worktree identity before explicitly resuming work. Do not start a second controller against a copied database.

## Conservative legacy holds

| Retained state | Native behavior and remediation |
| --- | --- |
| Missing or incomplete legacy Git/process ownership | Preserve journals, receipts and worktrees. Refuse access or replay that requires stopped ownership. Inspect the exact external resource and use an existing explicit recovery action only when its required proof is available. Missing proof does not mean stopped. |
| Conflict refresh points at another candidate, or lacks `sourceCandidateId` | Refuse conflict access and preserve both candidate and workspace. Inspect the retained candidate, completion and refresh receipts together. No automatic database repair is provided; keep the run held until the association is recoverably established. |
| Noncanonical or redirected conflict/publication path | Refuse access through the substituted path. Preserve edits and original Git metadata. Restore the original identity from the verified same-path snapshot; matching HEAD alone does not establish ownership. |
| Checked promotion has incomplete command, claim, completion or seal proof | Retain the conservative hold. Do not synthesize success from a matching revision or an absent PID. Restore matching proof/state or retain the run for an audited recovery decision. |

These holds are intentional migration outcomes, not completed workflow claims. Valid schema data can remain readable while a particular external operation needs operator recovery. The fixtures verify that refusal preserves recoverable content.

## Repeatable disposable acceptance

After building the service/runtime binaries, the following exercises production `crewroomd` with real OMP, Mail, Git and Docker, using scripted model responses:

```sh
PATH=/opt/homebrew/opt/node/bin:$PATH python3 scripts/test-native-installation.py \
  --output /private/tmp/a-new-native-fixture \
  --binary-directory /path/Crewroom.app/Contents/MacOS \
  --mail-binary /absolute/path/to/mcp-agent-mail
```

It configures fresh roles and a repository, produces coding check/review evidence, pauses all work, copies and restores the entire installation at the same paths, verifies retained evidence and pending steering, then completes a separate research run. It records exact process ownership and container cleanup. A failed boundary remains failed even if emergency cleanup succeeds. Fixtures are private, disposable, and make no live provider-delivery claim.

See [the final release validation plan](native-release-validation.md) for the gated fresh checkout runner, source-bound package checks, native resource measurements and rollback/cutover checklist.
