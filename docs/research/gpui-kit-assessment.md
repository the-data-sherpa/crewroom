# GPUI Kit assessment for Crewroom

Date: 2026-10-01. Research only; no migration, build, benchmark, or native application test was performed. This is a proposal, not an amendment to the accepted browser product direction.

## Recommendation

GPUI Kit is a credible candidate for a native Crewroom workbench. Evaluate one representative native screen against the existing production browser UI before committing to migration. Keep the workflow daemon, SQLite records, provider adapters, and process supervision intact during that evaluation. A complete Rust port is a separate decision with substantially more recovery and evidence contracts to revalidate.

The strongest case is a desktop workbench with good keyboard control, docking, long conversations, and responsive dense views. Smaller total memory use is plausible but unproved. A UI migration alone does not remove the agent runtime or command-containment footprint.

## What the toolkit supplies

These are verified descriptions in current official documentation/source, not locally executed acceptance results.

| Area | Finding and implication |
| --- | --- |
| Architecture | GPUI is Zed's GPU-accelerated Rust rendering/window framework, combining immediate and retained behavior. GPUI Kit adds unstyled behavior/state in `gpui-base`, styled controls in `gpui-component`, and a facade pinning a compatible GPUI version. React components would need to be rewritten; this is not a replacement renderer for the current JSX. The optional JavaScript extension runtime is a separate capability, not an automatic React compatibility layer. [GPUI](https://gpui.rs/), [Kit repository](https://github.com/longbridge/gpui-kit) |
| Workbench controls | The catalog includes docking, resizable panels, tables, virtual lists, forms, command menus, chat messages and a tail-following virtualized message list. These match several Crewroom surfaces. [Component catalog](https://gpui-kit.com/component/) |
| Code and prose | The Editor documents Tree-sitter language support, folding, search, read-only search, and decorations. TextView provides selectable Markdown/simple HTML, streaming content, code-block actions and custom Markdown plugins. These are useful building blocks for evidence and conversations. [Editor](https://gpui-kit.com/component/editor/), [TextView](https://gpui-kit.com/component/text-view/) |
| Terminal and diff | No ready-made terminal emulator or dedicated diff viewer was found in the documented catalog or the v0.7.0 component module exports. This bounded inspection does not establish that no third-party option exists. Budget an integration or custom view; an editor or Markdown code block does not establish terminal/ANSI fidelity or review-diff behavior. [Catalog](https://gpui-kit.com/component/), [v0.7.0 exports](https://github.com/longbridge/gpui-kit/blob/v0.7.0/crates/component/src/lib.rs) |
| Desktop platforms | Installation docs specify macOS 15+, Windows 10+ with an MSVC toolchain, and Linux dependencies verified on Ubuntu 24.04. Linux needs a graphical X11/Wayland session and working Vulkan driver. The documented locked graph requires Rust 1.92+. Cross-platform support is credible but does not certify Crewroom on those platforms. [Installation](https://gpui-kit.com/docs/installation/) |
| Accessibility | AccessKit semantics and keyboard/focus machinery are available. Official guidance explicitly separates headless semantic assertions from actual screen-reader announcements and platform adapter behavior. Test VoiceOver and keyboard navigation in the real application; do not assume native drawing alone makes the application accessible. [Accessibility](https://gpui-kit.com/docs/accessibility/) |
| UI testing | Headless interaction tests can drive real component input and inspect state, focus, layout and accessibility. Separate offscreen pixel rendering currently works on macOS/Metal; Linux/Windows run the portable interaction suite. Native-window behavior and complete OS IME composition still need separate tests. Existing Playwright journeys would need equivalent native coverage. [Testing](https://gpui-kit.com/docs/test/) |
| Packaging and updates | Kit supplies UI/application primitives, not an installer or updater. Crewroom would own app identity, resources, packaging, signing/notarization and release tests. The update guide leaves release selection, verification, installation and recovery to the application/distribution system. [Packaging](https://gpui-kit.com/docs/packaging/), [Auto update](https://gpui-kit.com/docs/auto-update/) |

The project claims commercial use in Longbridge Pro, which is a useful first-party maturity signal, not an independent reliability audit. The current release is v0.7.0, dated 2026-09-28, with explicit breaking changes to window/overlay and other APIs. Its manifest pins GPUI snapshots exactly because a prior floating requirement could select an incompatible snapshot. Pin the tested release and lockfile; budget upgrade work. The rendered site's getting-started dependency example still says `0.6`, another reason to use a consistent versioned source rather than mix snippets. [Repository](https://github.com/longbridge/gpui-kit), [release notes](https://gpui-kit.com/releases/), [v0.7.0 manifest](https://github.com/longbridge/gpui-kit/blob/v0.7.0/Cargo.toml), [getting started](https://gpui-kit.com/docs/getting-started/).

## Footprint and the current application

**Repository facts:** Crewroom currently runs React/TypeScript in a browser with a local Express server. It has no Electron dependency. Development runs `tsx watch` and Vite middleware; production serves built files. Comparing a release Rust binary with the development stack would confound the result. [Package manifest](../../package.json), [server setup](../../server/app.ts).

The independent coordinator/specialist processes, TypeScript agent hosts, OMP, tmux, Rust Agent Mail and Docker command containers remain if only the workbench changes. Independent observable agents are an accepted contract. Docker exists because a real escaped-descendant experiment defeated the simpler native ownership boundary. Replacing the UI does not solve that problem or justify removing containment. [Architecture](../architecture.md), [ADR 0006](../adr/0006-contain-repository-commands.md).

The current wall renders read-only ANSI terminal streams through `ghostty-web`; tmux and durable observations exist outside the browser. Native terminal display is therefore a first-order spike, not a cosmetic text-widget substitution. [Terminal component](../../src/terminal.tsx), [ADR 0007](../adr/0007-live-agent-wall-over-push-channel.md).

HTTP/WebSocket is a useful migration seam, but the existing server requires its own origin for JSON mutations and WebSocket upgrades. A native client needs a deliberate authenticated local-client contract, with credential isolation and cross-origin rejection revalidated; simply disabling those checks is not the migration design. The workflow daemon must retain sole authority, and UI closure must remain distinct from stopped execution. [Server authorization](../../server/app.ts), [architecture](../architecture.md).

**Performance evidence:** Kit's comparison page explicitly says its roughly 12 MB Hello World estimate is not a controlled cross-framework benchmark, and no shared workload/hardware supports an FPS ranking. Binary size is not resident memory; a 120 Hz rendering target is not an application throughput or battery guarantee. No Crewroom memory, startup, CPU, GPU or energy measurements were collected for this assessment. [Official comparison caveats](https://gpui-kit.com/docs/comparison/).

**Inference:** A native UI may reduce incremental renderer overhead and improve desktop interaction, but its impact on the complete running system depends on how much resource use belongs to the browser versus agents, hosts and Docker. A Rust backend/host port could reduce some runtime overhead, but would require revalidating durable ownership, cancellation, restart, store migration and provider reconciliation. The accepted M5 live proof and M6 progress make preserving that evidence valuable. [Current status](../status.md).

## Bounded evaluation proposed

1. Record a production-mode baseline on the operator's Mac: application-attributable and whole-process-tree memory, startup time, idle CPU, and responsiveness during the same four-agent terminal workload. Record browser attribution assumptions and Docker separately.
2. Prototype one GPUI Kit screen containing the ticket list, coordinating conversation, terminal wall and evidence state, using the existing daemon. Validate ANSI redraw/scrollback, text selection, copy, Unicode/IME, keyboard focus and VoiceOver. Implement the local-client boundary before exercising mutations.
3. Replay disconnect/reconnect and delayed observation, then exercise steering and pause/resume through the retained authority. Require truthful process/delivery/evidence states throughout and UI closure that leaves supervision well-defined.
4. Compare measurements and implementation cost. Adopt the native frontend only if it produces a meaningful benefit for the operator. Consider moving backend components to Rust afterward, one tested boundary at a time, if measured footprint or distribution needs justify it.

No framework selection, migration commitment, or runtime contract change is accepted by this note.

## Follow-up: native Ghostty/tmux side project

At the operator's request, a separate [terminal lab](/Users/crewroom/Projects/Git/crewroom-terminal-lab/README.md) was built on 2026-10-01. It uses GPUI Kit 0.7.0 with the third-party `gpui-libghostty` 0.3.1 adapter to embed actual Ghostty Metal surfaces, each attached to an isolated tmux session. Native visual/input checks and repeatable tmux/PTY checks establish a bounded macOS proof. A rebuild/signing launch defect was repaired and covered by a regression check. See [results and remaining limits](/Users/crewroom/Projects/Git/crewroom-terminal-lab/RESULTS.md). Crewroom's runtime was not migrated.
