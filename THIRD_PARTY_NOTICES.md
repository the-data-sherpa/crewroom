# Third-party notices

The top-level [MIT license](LICENSE) covers Crewroom's own code. It does not replace the licenses or copyright notices of third-party code, fonts, libraries or tools.

## Source included in this repository

| Component | License and provenance |
| --- | --- |
| `vendor/gpui-pre` | Apache-2.0; [license](vendor/gpui-pre/LICENSE-APACHE) and [source/patch record](vendor/gpui-pre/CREWROOM.md) |
| `vendor/gpui-component` | Apache-2.0; [license](vendor/gpui-component/LICENSE-APACHE) and [source/patch record](vendor/gpui-component/CREWROOM.md) |
| `vendor/gpui-libghostty` | MIT; [license](vendor/gpui-libghostty/LICENSE) and [source/patch record](vendor/gpui-libghostty/CREWROOM.md) |
| Embedded Ghostty source | MIT; [license](vendor/gpui-libghostty/vendor/ghostty/LICENSE) and [source record](vendor/gpui-libghostty/vendor/ghostty/VENDOR.md); nested dependencies retain their own notices |
| Nerd Fonts sources/assets within Ghostty | Mixed MIT, SIL Open Font License and upstream font terms; [retained license](vendor/gpui-libghostty/vendor/ghostty/vendor/nerd-fonts/LICENSE) |
| Date parsing adapted from V8 | BSD-3-Clause terms; [source attribution](crates/crewroom-core/src/source_time.rs) and [V8 license](crates/crewroom-core/licenses/V8-LICENSE) |

Keep the original notices with these files. Vendored patch records identify the changes made for Crewroom.

## Dependencies and native packages

[Cargo.lock](Cargo.lock) and [package-lock.json](package-lock.json) record resolved dependencies. Those packages retain their upstream licenses. OMP, Codex, Claude Code, Rust Agent Mail, Docker and model/provider services are separately installed or configured; Crewroom's license does not grant rights to those products or services.

The native packager includes Crewroom's MIT license, this notice, the V8 attribution/license, and retained vendored license/notice files. Linux stores them under `Crewroom/share/crewroom/licenses`; macOS uses `Crewroom.app/Contents/Resources/licenses`. Links in this document refer to the source repository; the copied licenses preserve their package-relative paths.

This inventory describes included source notices. It is not a complete license report for every transitive dependency of a binary. Before distributing compiled releases, review the resolved dependency licenses and include any additional required notices. The documented Linux package is a local build with host-provided shared libraries.
