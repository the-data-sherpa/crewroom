# Crewroom native terminal dependency

This directory vendors the crates.io `gpui-libghostty` 0.3.1 archive, whose SHA-256 is `2b12d0d271cf0ca99a9073bdc148ce43782713c6d01be33af2a105b00c62ef5d`. The archive's nested Cargo.lock is omitted because Crewroom's workspace lock controls resolution. Registry bookkeeping files are omitted.

The dependency is MIT licensed. [LICENSE](LICENSE) preserves the upstream adapter license from [the upstream repository](https://github.com/behzade/gpui-libghostty/blob/main/LICENSE). [Ghostty's license](vendor/ghostty/LICENSE) and [source record](vendor/ghostty/VENDOR.md) remain alongside its source. No retired Crewroom or OMP installation is included.

`crewroom-patches.json` records before/after SHA-256 values for every modified upstream file:

- `build.rs` carries the retained Crewroom terminal lab's Apple Command Line Tools build patch. It uses the pinned Ghostty source and runtime Metal compilation without a full Xcode installation.
- `src/adapter.rs` and `src/terminal.rs` add a read-only observation option. The adapter rejects typing, paste, wheel forwarding and nonselection mouse input; Cmd+C on macOS, Ctrl+Shift+C on Linux and local selection remain available. `src/lib.rs` exposes the copy-key guard to the adapter macro. OSC clipboard reads and writes are denied in Crewroom's custom terminal configuration. The actual tmux client independently enforces `read-only,ignore-size`.

Verified development prerequisites are Rust 1.97.1, Zig 0.16.0 and the macOS Command Line Tools SDK. Set `ZIG` to a verified Zig executable when it is not on PATH. Zig builds the static library; it is not needed at app runtime. Ghostty's locked Zig dependencies may be fetched on the first build. Native acceptance currently covers Apple Silicon macOS only.

The upstream Linux implementation uses a Wayland subsurface and OpenGL/EGL. Its build links libc++ (LLVM 21 or newer) and libxml2. X11 terminal surfaces are not supported. Linux source-build validation is tracked separately from the accepted macOS release.

The observer uses a private, separately selected tmux session group. Closing a surface detaches its client and destroys only that observer session. Crewroom fixes runner window sizing independently of observers. A terminal is never an input channel, process-ownership proof, check result or completion receipt.
