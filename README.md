# Crewroom

Crewroom is a local native workbench for directing coding agents from an accepted ticket to checked code and a Bitbucket pull request. You steer the team through a coordinator and keep the final review and merge decision.

The Rust/GPUI desktop app connects to a separate persistent Rust service. Coordinators and specialists run in separate observable processes. Crewroom retains their messages, work ownership, checks, reviews and recovery state.

## What it does

- Import Jira tickets or create local work, refine requirements and accept an implementation plan before coding.
- Assign OMP, Codex or Claude Code agents, with model choices captured for each assignment.
- Coordinate programming pairs and specialists in isolated worktrees, with one writer at a time.
- Observe agents, send steering, pause or cancel work, and recover interrupted sessions.
- Require revision-specific checks and independent review before reporting a pull request ready.
- Edit reusable roles, principles and playbooks, then compare proposed instruction revisions before adoption.

Jira status, workflow state and agent activity remain separate. A queued plan is not a running agent. Saved steering, confirmed receipt and application remain distinct too.

The accepted MVP and subsequent multi-harness and verification work through S9.3 are implemented. Validation is specific to the revisions, environments and cases in the [status record](docs/status.md) and [test evidence](docs/testing/mvp-progress.md). This is a single-operator developer application. It does not provision provider accounts or offer automatic merging, a hosted service or a portable Linux binary release.

## Install on Arch Linux

Use the [Arch installation guide](docs/linux.md) for system dependencies, Rust setup and an optional per-user installation. Arch x86_64 with Wayland is the documented Linux source-build target.

After installing those dependencies:

```sh
git clone --branch main https://github.com/the-data-sherpa/crewroom.git
cd crewroom
export CREWROOM_NATIVE_DATA_DIR="$HOME/.local/share/crewroom/data"
export CREWROOM_ROLE_PATH="$HOME/.config/crewroom/roles.json"
./scripts/run-native.sh --release
```

This builds a fresh native directory package, starts or reuses the service and opens the window. Add `--build-only` to build without starting anything. Keep the complete package and its adjacent receipt; subsequent launches can use its printed `Crewroom/bin/crewroom-launcher` path without rebuilding. Closing the window leaves the service running.

**`npm run build` builds the retained browser frontend, not the native app.**

| Command | Result |
| --- | --- |
| `./scripts/run-native.sh --release` | Build, package and launch the native app |
| `./scripts/run-native.sh --release --build-only` | Build and package only |
| `./scripts/run-native.sh` | Build and launch the debug app |
| Bundled `crewroom-launcher --data-dir … --roles …` | Launch or reopen an existing installation |
| `npm start` / `npm run dev` | Aliases for the release/debug native checkout launchers |
| `npm run build` | Type-check and build the retained browser frontend |

A fresh installation supports local tickets, briefs, roles and repository configuration. Missing integrations remain visibly unavailable. Installed native packages need no Node, Rust or Zig; Linux still needs compatible host libraries and GPU drivers.

## Build on macOS

The original packaged release target is Apple Silicon macOS. Install Apple Command Line Tools, Git, Rust through rustup and Python 3.12 or newer. The repository pins Rust 1.97.1; bootstrap downloads and verifies Zig 0.16.0. A full Xcode installation is not required.

From the checkout:

```sh
export CREWROOM_NATIVE_DATA_DIR="$HOME/Library/Application Support/Crewroom Native/data"
export CREWROOM_ROLE_PATH="$HOME/Library/Application Support/Crewroom Native/config/roles.json"
./scripts/run-native.sh --release
```

Reuse the printed `Crewroom.app/Contents/MacOS/crewroom-launcher` with explicit `--data-dir` and `--roles` paths for later starts. Finder opens the app but does not start or configure the service. See [native installation](docs/native-installation.md), [release scope](docs/release-native.md) and [start/stop operations](docs/operations.md#start-and-stop).

## Connect execution

Agent work requires a configured OMP, [Codex](docs/testing/mh2-runtime-acceptance.md) or [Claude Code](docs/testing/mh3-runtime-acceptance.md) harness, tmux, separately managed Rust Agent Mail, authenticated model access, and Docker with the required pinned local images. Jira intake and Bitbucket publication need their own account configuration. GitHub hosting for this repository does not add a GitHub PR adapter.

Configure the agent runner in **Settings → Runner**, which lists each prerequisite and saves a private runtime configuration; headless services can instead select private runtime JSON with `CREWROOM_RUNTIME_CONFIG`. Provider metadata lives in `<data>/jira.json` and `<data>/bitbucket.json`; credentials stay server-side. Follow the [configuration inventory](docs/operations.md#configuration-inventory) and [Linux provider guidance](docs/linux.md#execution-and-provider-access). A running daemon retains its original environment until a stopped restart; Settings → Runner reloads the runner configuration in place when no run owns live processes.

## Documentation and contributions

- [User guide](docs/user-guide.md): daily work, supervision and instruction editing.
- [Operations](docs/operations.md): configuration, shutdown, backup and recovery.
- [Contributing](CONTRIBUTING.md): setup, checks and pull requests.
- [Security](SECURITY.md): private reporting and safe diagnostic information.
- [Product](docs/product.md), [architecture](docs/architecture.md) and [domain terms](CONTEXT.md): accepted contracts.

Use [GitHub issues](https://github.com/the-data-sherpa/crewroom/issues) for bugs and proposals. Contributors do not need access to the maintainer's Linear workspace. Historical evidence can refer to private fixtures or unavailable local paths; those references are records of past checks, not prerequisites for contributing.

Public Git history begins with a reviewed source snapshot. Older revision IDs in design and test records refer to development history retained privately; they do not resolve in this repository and are not fresh checks of the public initial commit. See [source history and evidence](docs/publication-review.md#history-and-evidence) for the boundary and current verification routes.

| Path | Contents |
| --- | --- |
| `crates/crewroom-desktop` | GPUI workbench and terminal wall |
| `crates/crewroom-service` | Authenticated loopback service and persistence |
| `crates/crewroom-core` | Workflow state, transactions and migrations |
| `crates/crewroom-runtime` | Processes, model gateway, Mail, repositories and providers |
| `vendor/` | Pinned UI/terminal dependencies, licenses and patches |
| `server/`, `src/`, `shared/`, `tests/` | Retained TypeScript implementation and browser regression fixtures |
| `scripts/`, `docs/` | Build tools, disposable validation, contracts and evidence |

## License

Crewroom's own code is licensed under [MIT](LICENSE). Vendored and other third-party components retain their licenses; see [third-party notices](THIRD_PARTY_NOTICES.md).
