# Native terminal focus and load follow-up — 2026-10-04

This iteration follows `9afbd304608dc20d87717bfe12bdc1bc13ddc758`. The adjacent JSON binds the changed Rust/vendor sources, local gate logs, final binary hashes, scripted load receipt and cleanup. The result closes the observed native field-focus and reverse-tab defects. It does not certify the full N6/N7 acceptance matrix.

## Change and diagnosis

The native input frame previously used a different focus handle from its editor. GPUI therefore omitted the focused editor from the accessibility focus path. The vendored `gpui-component` 0.7.0 input frame now tracks the actual editor handle. GPUI's NSWindow also acted as first responder while its AccessKit adapter lived on the content view. The app installs AccessKit 0.26.3's public focus-forwarding bridge after creating the GPUI window. Agent wall activation occurs after its native adapter is installed.

Sharing the actual handle exposed a second defect: both input frame and inner editor registered the same handle in GPUI's tab order, so Shift-Tab returned the same field. The new application regression failed at “Reverse traversal must reach the preceding field” before the fix. The vendored `gpui-pre` 0.3.7 tab map now replaces a previous registration of the same handle, retaining the final inner position. Apache licenses, archive hashes, original source hashes and patch hashes are retained beside both vendors. Only one upstream source file per package changed.

The draft-confirmation title and explanation now appear as native accessibility labels. Opening the saved transcript focuses its real read-only field. The tmux observer now explicitly pans its viewport left after selecting the owned window; long output previously caused tmux to follow the cursor into the tail of a line. A regression also checks the 2,000-event/8 MiB retained-observation limits and duplicate reconnect pages.

## Executed evidence

- Full workspace tests: 149 passed. Final desktop tests after the wall activation seam: 13 passed. Full binary build, formatting and workspace Clippy passed. The first Clippy run caught the focus helper placed after a test module; that log remains, and the corrected rerun passed.
- The dependency's own test target cannot compile from its crates.io archive because its SVG tests reference IBM Plex Sans and Lilex files outside that archive. The failed compile log is retained. The new dependency-local tab-map test is not claimed as executed; the actual application regression exercises the same map and passes.
- Actual macOS app, final unsigned SHA-256 `f990d9707f482a62e96e11c97e7f6e897a1233ac4d01e7a63a85e19b9f151ab8`: entering a title, Tab to Description, then Shift-Tab back to Title reported the actual respective focused field in the native AX tree. Switching to Roles hid that input; Cmd-Q still opened the readable unsaved-draft guard. Discard closed only the UI.
- Actual wall: the transcript's native AX focus reported the text field after selection. Select-all highlighted saved text; attempted typing did not edit it. Four Ghostty panes displayed the leftmost `LOAD 145` role prefixes, Japanese text, emoji and accented text after reconnect. Earlier checks in this fixture covered narrow/wide resizing, transcript overlays and a named SIGSTOP/SIGCONT facade outage. This is a facade reconnect check, not a production daemon restart.
- The opt-in scripted-provider fixture emitted 29,000 long Unicode lines across four real independent runner/OMP process pairs over 127,196 ms (145 rounds), with no recorded command failures. Pane sizes remained 160×40 and scrollback stayed below its 2,000-line ceiling. The app's retained event bounds have a direct regression; a single RSS sample is not treated as a memory-leak proof.
- After final UI quit there were zero observer clients and the same eight owned processes were alive. The fixture was then stopped explicitly and exited zero; all eight owned processes were gone. The primary daemon, active data and retained installations were untouched.

## Remaining acceptance

Actual VoiceOver speech/navigation and an OS IME candidate/composition session remain unverified. Direct GPUI UTF-16 marked-text replacement, cancellation and draft retention tests passed in the prior checkpoint and still pass. Native synthetic `typeText` cannot substitute for an OS IME session. AX labels and focus are not speech proof.

No system settings were changed. CUA's Computer/Browser Use Confirmation Policy, section 3, says: “Non-sensitive system or application settings: If the initial prompt explicitly requests the change, proceed without reconfirming; otherwise confirm immediately before applying it.” Enabling VoiceOver or selecting/adding an OS input source therefore needs specific user authorization through that UI workflow. No automatic approval rejection occurred. The remaining bounded check is to enable VoiceOver, traverse status/evidence, form fields, draft guard and read-only transcript, then use an installed Japanese input source to compose, replace and cancel marked text before navigating away and back. Restore the prior settings afterward. The parent will combine this request with the final integrated acceptance work; no request is made at this checkpoint.

Progress assessment: terminal ownership, read-only transport, overlays, actual keyboard/AX focus, bounded load and cleanup have concrete evidence. The implementation remains on track; remaining manual speech/IME and combined production-service journeys must stay visible rather than being inferred from the scripted facade.
