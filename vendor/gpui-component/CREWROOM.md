# Crewroom accessibility dependency patch

This directory vendors the crates.io `gpui-component` 0.7.0 archive (SHA-256 `ac246c9f3d9771e56fbf90bd7d8656d0992d83940b580debf2efab4c44fe91df`). Its Apache-2.0 license is retained in [LICENSE-APACHE](LICENSE-APACHE). Registry bookkeeping and the nested Cargo.lock are omitted; Crewroom’s workspace lock controls resolution. `crewroom-patches.json` records original and changed source hashes.

The accessible input frame tracks the editor’s real focus handle. A separate frame handle leaves the focused editor outside the accessible tree.

The related native app seam uses AccessKit 0.26.3’s public macOS focus-forwarding API after GPUI creates its `GPUIWindow` NSWindow subclass. No system accessibility settings are changed. The app’s composition/focus regression and actual native AX checks cover typing, Tab, Shift-Tab and hidden-field draft guards.
