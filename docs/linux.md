# Arch Linux native build

Crewroom has a Linux source-build path for the native GPUI workbench and separate Rust daemon. The initial target is **x86_64 Arch Linux with Wayland**, including Omarchy/Hyprland. This is a local directory package, not an AppImage, AUR package, or portable Linux release. The [Linux validation record](testing/linux-native-2026-10-04.json) separates executed checks from remaining validation. The older macOS release acceptance does not certify Linux.

## Dependencies

On an up-to-date Arch installation, install the build and runtime libraries:

```sh
sudo pacman -Syu --needed base-devel clang cmake pkgconf python git \
  libc++ fontconfig freetype2 libxcb libxkbcommon libxkbcommon-x11 \
  wayland libglvnd libxml2 openssl vulkan-icd-loader lsof
```

Use Rust >=1.97. If you already use rustup, keep it: the checkout selects Rust 1.97.1 from `rust-toolchain.toml`. On a system without Rust, install Arch's `rustup` package, then let Cargo select that pinned toolchain from inside the checkout:

```sh
sudo pacman -S --needed rustup
```

Arch's `rust` package is an alternative to rustup. Do not install both. Distro Cargo uses the installed compiler. The initial Linux validation used Arch Rust 1.98.1.

The build also needs Python >=3.12 and LLVM libc++ >=21. Bootstrap checks dependencies, then downloads the official Zig 0.16.0 archive into ignored `.crewroom/toolchains`. It verifies both the archive checksum and the extracted inventory. It does not change global Rust or Zig configuration.

The GPU needs working Vulkan and desktop OpenGL 4.3/EGL drivers, and the desktop needs a Wayland session. Keep the appropriate driver for your GPU. GPUI's renderer and the embedded Ghostty terminal use different graphics paths. Compiling the app does not prove terminal rendering. The launcher refuses X11-only sessions before it starts a service. `--headless` does not require a display. Bootstrap supports Linux ARM64 archives, but Crewroom has no executed ARM64 validation.

Dependency references: [GPUI/Zed's Linux build guidance](https://zed.dev/docs/development/linux), [Zig's official release manifest](https://ziglang.org/download/index.json), and the [vendored terminal requirements](../vendor/gpui-libghostty/README.md).

## Build and launch

Clone the repository, then choose explicit private paths for this installation:

```sh
git clone --branch main https://github.com/the-data-sherpa/crewroom.git
cd crewroom
```

Build and start the release app:

```sh
export CREWROOM_NATIVE_DATA_DIR="$HOME/.local/share/crewroom/data"
export CREWROOM_ROLE_PATH="$HOME/.config/crewroom/roles.json"
./scripts/run-native.sh --release
```

To build and package without starting a window or service, use:

```sh
./scripts/run-native.sh --release --build-only
```

Omit `--release` for a debug build. `npm run build` only builds the retained browser frontend. It does not build or install the native app. The wrapper respects Cargo's configured target directory, including `CARGO_TARGET_DIR`. It prints a fresh package path containing `Crewroom/bin/crewroom-launcher` and the four native executables. The adjacent `Crewroom.receipt.json` records source hashes, binary hashes and resolved shared-library dependencies.

You must own existing data and role directories, with no group/other permissions. The launcher creates new directories privately and preserves existing configuration. It does not import macOS state or rewrite private files. Without the environment variables above, checkout defaults remain `.crewroom/rust` and `~/.config/crewroom/roles.json`.

Subsequent starts need only the built package and its runtime libraries:

```sh
/absolute/path/Crewroom/bin/crewroom-launcher \
  --data-dir "$HOME/.local/share/crewroom/data" \
  --roles "$HOME/.config/crewroom/roles.json"
```

The Linux launcher remains attached to the GUI process. Closing the window leaves the independent service running. The launcher retains GUI stdout/stderr under the private data directory. Add `--headless` to provision or reuse only the service. Add `--port` to choose its initial loopback port. Reopening authenticates and reuses the existing daemon, retaining its port, roles and runtime configuration. Follow [operations](operations.md#start-and-stop) to stop or restart that exact installation.

The directory package is dynamically linked. A copied package still needs the recorded libraries and compatible GPU drivers on its destination host. The build installs no desktop entry, system service or autostart configuration.

## Service did not become ready

Inspect `service.log` in the selected data directory. If it reports `Address already in use`, identify the listener with `ss -ltnp 'sport = :4318'`. Another Crewroom installation can occupy the default port even when the selected data directory has no running service. Rebuilding does not resolve a port collision.

For a new installation without live hosts to reattach, choose an available loopback port and launch the already-built package:

```sh
/absolute/path/Crewroom/bin/crewroom-launcher \
  --data-dir /absolute/private/data \
  --roles /absolute/private/config/roles.json --port 4319
```

Retain that port for subsequent stopped restarts. Reopening an authenticated running service reuses its existing port. Leave other installations running unless you intend to stop them through the [verified shutdown procedure](operations.md#start-and-stop). Preserve the log when startup fails for another reason.

## Install the package for your user

Building does not register an application with Arch or copy it onto `PATH`. You can run the printed bundle directly. To retain it outside Cargo's build directory, copy the **whole package and its receipt** to a new directory under your home directory.

Set `CREWROOM_PACKAGE` to the absolute `bundle` path printed by the build. Replace the example placeholder:

```sh
CREWROOM_PACKAGE="/absolute/printed/path/Crewroom"
mkdir -p "$HOME/.local/opt/crewroom"
CREWROOM_INSTALL="$(mktemp -d "$HOME/.local/opt/crewroom/build.XXXXXX")"
cp -a -- "$CREWROOM_PACKAGE" "$CREWROOM_INSTALL/Crewroom"
cp -- "$CREWROOM_PACKAGE.receipt.json" "$CREWROOM_INSTALL/Crewroom.receipt.json"
printf 'Installed package: %s\n' "$CREWROOM_INSTALL/Crewroom"

"$CREWROOM_INSTALL/Crewroom/bin/crewroom-launcher" \
  --data-dir "$HOME/.local/share/crewroom/data" \
  --roles "$HOME/.config/crewroom/roles.json"
```

Retain that printed installation path for future starts. The package contains the GUI, daemon, runtime helpers, launcher and license notices. Do not copy or symlink the launcher by itself: it locates the other executables beside itself. The launcher creates missing data and role directories privately. Existing directories must have mode `0700`, and an existing role file must have mode `0600` or stricter.

To check the service for this installation:

```sh
"$CREWROOM_INSTALL/Crewroom/bin/crewroomd" \
  --data-dir "$HOME/.local/share/crewroom/data" --status
```

Before upgrading, Pause work and verify that the owned agents have stopped. Follow [start and stop](operations.md#start-and-stop) to verify the exact daemon's identity, send SIGTERM and wait for cleanup. Take a [whole-state backup](operations.md#backup-and-same-path-restore) before starting a newer build. Reopening with a newer launcher while the old daemon runs reuses that old daemon.

Build or copy the upgrade into a fresh installation directory. Keep the previous package, receipt and matching stopped backup together. Use the same data/role paths when starting the new version. Rollback requires the matching older package and its complete backup at the original paths. An older binary must not open newer state.

To remove the app, first stop its work and service, then delete only the copied installation directory. Data, roles, runtime configuration and separately managed Mail storage remain at their selected paths. These steps create no pacman package, desktop entry or autostart service.

## Execution and provider access

You can use local tickets, briefs, role editing and repository configuration without connecting providers. Agent execution needs a configured OMP, [Codex](testing/mh2-runtime-acceptance.md) or [Claude Code](testing/mh3-runtime-acceptance.md) runner. It also needs tmux, Rust Agent Mail, model access and Docker with the required local pinned images. Use the absolute Docker socket for this Linux engine in `commandEngine.socket`. Do not copy a macOS Docker Desktop socket path. Each agent remains a separate supervised process with external messaging.

The native service accepts `CREWROOM_JIRA_TOKEN` and `CREWROOM_BITBUCKET_TOKEN` in its private launch environment. Both are **bearer access tokens**. The Jira setting is for the existing OAuth endpoint `api.atlassian.com/ex/jira/<cloudId>`. An email/API-token Basic credential is not interchangeable. Jira metadata still requires the configured cloud/site/account/filter fields in `<data>/jira.json`. Bitbucket keeps its existing `<data>/bitbucket.json` metadata contract. See the [configuration inventory](operations.md#configuration-inventory).

These tokens remain in the service, outside workspace responses and agent environments. Linux does not invoke macOS Keychain commands. Missing credentials remain unavailable. Crewroom does not refresh environment tokens automatically. Rotate them through the private service environment, then stop and restart the service. Reopening a live daemon does not reload its environment. Never put credentials in a command-line argument, committed file or UI draft.

`scripts/prepare-rust-mail-test.py` now selects and verifies the pinned Linux test archive. Downloading that fixture does not start a Mail service or validate a new OMP version. The retained OMP fixtures now have a guarded Linux path with a private pinned runner installation. Their synthetic-provider results and the live native Codex/Claude workflow results are separate from Linux unit tests and historical macOS package acceptance.
