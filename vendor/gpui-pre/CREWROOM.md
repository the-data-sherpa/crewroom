# Crewroom accessibility dependency patch

This directory vendors the crates.io `gpui-pre` 0.3.7 archive (SHA-256 `0e87a42bb37c7cb4e76dd1ac0ce88851e46e976e0373a47ab3e0757abffee54d`). Its Apache-2.0 license is retained in [LICENSE-APACHE](LICENSE-APACHE). Registry bookkeeping and the nested Cargo.lock are omitted; Crewroom’s workspace lock controls resolution. `crewroom-patches.json` records original and changed source hashes.

Repeated registration of one focus handle replaces its prior tab-order entry. The final, innermost position wins. Without deduplication, Shift-Tab returns the same input forever when both its accessible frame and inner editor register focus.

The related native app seam uses AccessKit 0.26.3’s public macOS focus-forwarding API after GPUI creates its `GPUIWindow` NSWindow subclass. No system accessibility settings are changed. The app’s composition/focus regression and actual native AX checks cover typing, Tab, Shift-Tab and hidden-field draft guards.

Tab and Shift-Tab now request a one-frame reveal of the newly focused control inside its ancestor scroll areas. The request uses actual focus bounds, clamps each ancestor offset, and schedules one repaint. It does not scroll unrelated sibling containers or override later manual scrolling. The native acceptance tests cover a 390 px history/continuation journey and nested scroll containers with reverse traversal and retained sibling offsets. This patch is limited to `src/window.rs` and `src/elements/div.rs`; the manifest records their original and patched hashes.

GPUI’s existing `requested_autoscroll` path is consumed by virtualized `List` and `UniformList`, not ordinary scrollable `Div` elements. The Tab reveal request is separate so a generic control reveal cannot consume an editor/list autoscroll request or carry it into an unrelated sibling container.
