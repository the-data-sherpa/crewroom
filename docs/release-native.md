# Native migration release — 2026-10-04

The Rust/GPUI migration is accepted for the agreed personal operator workflow on Apple Silicon macOS. VoiceOver and advanced OS input-method compatibility remain optional and unverified. Both the operator workbench and persistent workflow service are native. The repository defaults now launch the native app; the TypeScript implementation remains an explicit compatibility oracle and matching-build rollback reference.

The repository cutover revision is `8b23fb7f13846a09ad9c0599a29c5a82f4246333` on `codex/rust-gpui`, bound by the [cutover receipt](testing/rust-native-default-cutover-2026-10-04.json). The signed app exercised by final package checks was built from `8f3e325870e40209f42c43ef0009e953f45398f5`. Its production source and build scripts remain unchanged at cutover. One later test-helper change permits the provider-injected final scenarios; the [cutover receipt](testing/rust-native-default-cutover-2026-10-04.json) records that difference and the exact source/package bindings. Documentation and npm launch aliases do not alter the packaged runtime.

## Start and use

Use [native installation](native-installation.md) to build an immutable bundle, then reuse its adjacent `crewroom-launcher` with explicit private data and role paths. The source-checkout conveniences are `npm run dev` for debug and `npm start` for release. The bundled launcher needs no Node, Rust, Zig or checkout. Finder opens the client only; it does not provision a service or installation.

Closing the app leaves the service and admitted work running. Reopening reuses the same authenticated service with its existing configuration. Use the [operations guide](operations.md) for an ownership-verified stop/restart, configuration, backup and recovery, and the [user guide](user-guide.md) for daily work.

The default switch changes repository startup and documentation. It does not migrate the retained pilot, replace an active daemon, change reusable roles, or open old state under a new runtime automatically. Explicit legacy startup is available through `npm run dev:legacy` and `npm run start:legacy`; keep legacy state separate.

## Accepted evidence

| Area | Executed evidence and boundaries |
| --- | --- |
| Workflow contracts | [Acceptance reconciliation](testing/rust-native-acceptance-reconciliation-2026-10-04.json): all 87 behavior cases, 31 native journey mappings and 12 original scenarios for the required workflow; optional OS compatibility remains separately unverified. These are coverage mappings, not 87 newly executed end-to-end cases. Three scenarios have final-candidate execution; nine compose earlier native proofs. |
| Native tests and controls | [Combined native checkpoint](testing/rust-native-keyboard-queue-2026-10-04.json): 180 default Rust tests, explicit native journeys, captured history, intake/restart, queue/Resume and merged-dependency checks, build, format and Clippy. |
| Fresh package and operations | [Final headless release](testing/rust-native-release-headless-2026-10-04.json): clean release build, signed binaries, minimal-PATH launcher/reuse/restart, 693-entry whole-state restoration followed by new use, three actual runtime profiles and killed-daemon reattachment. The unchanged optimized suite passed 180/180 after a retained initial 179/180 failure. |
| Complex work and repair | [Final original scenarios](testing/rust-native-final-scenarios-2026-10-04.md): multi-file code/UI/guide work, target drift, same-PR renewed checks/review, failed checks, blocking review and two explicit repair cycles. Provider/model responses are scripted; Git, Docker checks, host processes and Mail are real. |
| Packaged UI and terminal | [Final GUI](testing/rust-native-final-gui-2026-10-04.json): visible 1240/390 px controls, read-only terminals/transcript, actual selection/copy, hidden-field Quit guard, 18 output rounds/72 completed inputs, UI close/reopen and exact owned cleanup. |
| Optional OS compatibility — unverified | [OS attempt](testing/rust-native-os-input-attempt-2026-10-04.json): VoiceOver readout/navigation and actual Japanese–Romaji composition were not verified. The operator deprioritized VoiceOver on 2026-10-04. Japanese IME was additional compatibility validation beyond the accepted basic keyboard/layout contract. Both remain explicit follow-ups; neither is counted as passed or blocks this personal-workflow cutover. Original settings were restored. |
| Retained regression | [Fresh retained suites](testing/rust-native-cutover-browser-environment-2026-10-04.md): domain 332/332, build and full browser 31/31 for the exact proposed aliases. The first browser run lacked the isolated Mail prerequisite; that failed attempt remains recorded. |
| Startup cutover | [Default verification](testing/rust-native-default-cutover-2026-10-04.json): exact tested package bytes and source comparisons, literal npm alias dispatch and actual private installed-launcher start/reuse/cleanup. The npm recording stub checks dispatch only; it is not described as an end-to-end source build/launch. |
| Rollback | [Matching old-build rollback](testing/rust-native-rollback-2026-10-04.md): separate old build and same-path pre-cutover snapshot with unadmitted plans and pending steering. Full native runtime restoration is a separate complementary proof. No adoption of active legacy processes or older-code access to newer state is claimed. |

The original package's adjacent final-signed sidecar has SHA-256 `5f9b92bf3bda35bd549c1dc8902cd4e62bba48a347f12c9eb62f8051eedf835b`. Its individual executable hashes, source inputs, local signatures and tool identities are retained in the package receipts above. The internal Resources receipt records pre-bundle-signature hashes and must not replace that final sidecar.

## Resource observations and retained failures

The final visible four-agent exercise measured GUI median RSS of 153.42 MiB idle, 143.89 MiB during output and 181.91 MiB afterward, with CPU of 2.76/12.06/4.24 percent over roughly 15-second samples. The app bundle contains 71.76 MiB of file bytes. Rust service/hosts, external Mail, OMP and observer children are reported separately. RSS may count shared pages more than once; this is not PSS, total machine/GPU memory, a long-term leak test or a memory-reduction comparison. Docker VM resources were not attributed to this output-only workload.

The first GUI observation was invalidated by a fully covered window and a late finish latch. [Direct diagnosis](testing/rust-native-redraw-diagnosis-2026-10-04.md) established macOS display-clock suspension under occlusion; no production rendering patch was needed. The final run met hard deadlines but exceeded preferred start/finish timings. All failed attempts remain in the evidence history.

The passing retained browser run also logs four pre-existing `ghostty-web` range errors, present in an earlier accepted browser run. The optimized Rust suite's initial lock-contention failure is retained without a claimed production fix. These observations are not relabelled as impossible or silently removed.

## Supported scope

Validation covers macOS 26.6.2 on Apple Silicon with the recorded Rust 1.97.1, Zig 0.16.0, OMP 18.4.12, tmux 3.7c and Rust Agent Mail v0.3.36 environment. Component upgrades require their affected contracts to be rechecked. This is a local ad-hoc signed package, not a notarized installer or cross-platform release. General account provisioning and Finder-only daemon provisioning remain unsupported.

Private pilot details were removed during FOR-17 history sanitization. Original evidence is retained in a private backup. This placeholder is not executable validation evidence.