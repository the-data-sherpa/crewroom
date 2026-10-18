# Contributing to Crewroom

Use [GitHub issues](https://github.com/the-data-sherpa/crewroom/issues) for bug reports and proposals, and pull requests for changes. You do not need access to the maintainer's private Linear tracker. Maintainers mirror accepted work into Linear and keep repository status and evidence current.

For bugs, include the Crewroom commit, operating system, command or user action, expected result and actual result. Supply a small reproduction with synthetic data. Use [private reporting](SECURITY.md) for vulnerabilities.

## Set up a checkout

Clone the repository and create a topic branch in your own checkout. Follow the [Arch guide](docs/linux.md) or [macOS instructions](README.md#build-on-macos). Native builds use the pinned Rust toolchain, Python and verified Zig bootstrap. Node >=24 and npm are needed for retained TypeScript checks. Install their dependencies with `npm ci`.

The maintainer's machine uses a single development checkout on `main`; that local convention does not require external contributors to edit `main` or use the maintainer's absolute paths.

Before changing behavior, read [product direction](docs/product.md), [architecture](docs/architecture.md), [domain terms](CONTEXT.md) and [current status](docs/status.md). Keep Jira status, workflow state and agent activity separate. Preserve independent processes, server-side credentials, loopback/origin restrictions and revision-bound evidence.

## Check your changes

Choose checks that cover the behavior you changed. Report the exact commit, commands and results in the pull request, including skipped or unavailable checks.

For native changes:

```sh
ZIG="$(python3 scripts/bootstrap-native.py)"
export ZIG
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
./scripts/run-native.sh --release --build-only
```

For native build, package or launcher changes:

```sh
python3 -m unittest discover -s scripts -p test_native_scripts.py
python3 scripts/test-native-package.py --output /tmp/crewroom-package-check-unique
```

Choose a new output directory for each package check. It uses small compiled fixtures and does not launch Crewroom.

For domain changes, run `npm test`. For TypeScript/build changes, run `npm run build`. For user flows, run `npm run test:e2e` and the affected native journeys. The retained browser suite needs more than a Node installation: prepare its installed OMP/tmux, Docker images and pinned Rust Agent Mail fixture using the [runtime fixture guide](docs/testing/mh2-runtime-acceptance.md#reproduce-the-disposable-checks).

```sh
npm ci
npm test
npm run build
npx playwright install chromium
python3 scripts/prepare-rust-mail-test.py
npm run test:e2e
```

For reconnect, pause/resume or readiness after revision changes, start with the [feature map](docs/testing/feature-map/README.md). Its recipes select isolated state and retain cleanup evidence. Some native tests require external fixtures and are ignored by the default suite; a passing default suite does not mean those cases ran.

Use disposable state for tests. Never point fixtures at your normal data, repositories or provider accounts. Use `npm run test:run -- <command>` to record test ownership; `npm run test:processes` previews survivors and `npm run test:cleanup` stops verified test processes. See the [cleanup guide](docs/operations.md#lingering-test-processes).

## Submit a change

Keep the change focused and explain the problem and resulting behavior. Link the public issue when one exists. Update affected user documentation and describe limitations with the checks that cover them. Maintainers record accepted implementation state in `docs/status.md` and revision-bound results in `docs/testing/mvp-progress.md`.

Do not commit credentials, private Jira or repository content, connection descriptors, runtime stores, raw model sessions or screenshots containing private work. Check both the diff and files added by test tools. `.crewroom` and `.env` files are ignored, but an ignore rule does not remove anything already committed.

The public repository starts from a reviewed source snapshot. Use a fresh clone of its `main` for contributions. Do not merge or push branches or tags from the former private development history; that would reintroduce history excluded from publication. Historical revision IDs in retained evidence are explained in the [publication record](docs/publication-review.md#history-and-evidence).

Preserve third-party notices and document changes to vendored code in its existing patch records. Contributions to Crewroom's own code use the [MIT license](LICENSE); existing third-party terms still apply.
