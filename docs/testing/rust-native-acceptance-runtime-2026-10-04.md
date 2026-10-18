# Native acceptance runtime closure — 2026-10-04

The [source-bound receipt](rust-native-acceptance-runtime-2026-10-04.json) closes runtime gaps **G01–G04** from the [frozen acceptance map](rust-native-acceptance-map-2026-10-04.json). No production runtime code changed.

- **RTE-01:** all 950 role/model bindings match the retained policy, preserving provider, full model ID and API family. Tool capability remains unknown; a binding does not imply execution eligibility.
- **RTE-02:** all 57 named profile selections match at the native policy boundary. Baseline, heterogeneous and rotated profiles each execute a legal coordinator plus three specialists through the production native daemon and actual OMP. All five pool models and three API families are represented. Every observed model/API/effort and all 30 scripted provider requests match the assignment. The test selects a team from each profile; it does not launch all 19 roles or replay all 12 named scenarios.
- **MSG-01:** each profile executes coordinator → specialist A → specialist B → A → coordinator through actual Rust Mail. All 12 transport records match native message IDs, causal replies, originating tool events, recipient completion events and external Mail sender/recipient acknowledgements. An actual daemon SIGKILL reattaches the same eight host/runner processes per profile. All 24 owned processes subsequently stop; private tmux servers and daemon descriptors are gone.
- **LIM-04:** seeds `7`, `42` and `20261002` each execute 100 schedule steps twice against real native SQLite. The normalized traces match. Duplicate keys, delayed acknowledgements, durable reopen and terminal cancellation satisfy the retained seeded contract. This is not randomized OS-process crash injection; physical crash-window evidence remains separately recorded.

The frozen gate passes **143 Rust tests**, strict harness type checking, formatting, clippy, the catalog/profile comparator and all three actual-process profile runs. The receipt records every source hash, command, log hash and external proof. No giant domain/browser suite was rerun for these test-only changes. All providers are guarded local fixtures; there is no live delivery or model-quality claim.

The first fixture attempts exposed a non-UUID request key, JSON `undefined` normalization, incorrect message field names and an idle specialist without input. Those harness errors were corrected before the final frozen gate; failures and hashes remain in the receipt. The fourth specialist now receives one explicit harmless fixture input to observe its model; the four-hop chain uses Mail throughout.

Reproduce after building `crewroomd`, `crewroom-host`, `crewroom-process` and the `policy-oracle` example:

```sh
node --import tsx scripts/rust/verify-release-models.ts
cargo test -p crewroom-core --test release_schedules --locked -- --nocapture
python3 scripts/rust/verify-release-profiles.py --profile baseline --output /private/tmp/cr-profile-baseline
python3 scripts/rust/verify-release-profiles.py --profile heterogeneous --output /private/tmp/cr-profile-mixed
python3 scripts/rust/verify-release-profiles.py --profile rotated --output /private/tmp/cr-profile-rotated
```

Each output directory must be new and short enough for private tmux sockets. The fixture uses a checkout-local Mail executable and copies the installed OMP executable/native addon into a guarded private directory. It does not load real provider configuration.

Progress: the four assigned runtime gaps now have evidence. This remains on track for completion; parent-owned UI assertions, integrated release installation, resource measurements, matching-old-build/pre-cutover-snapshot rollback and the default/retirement decision remain outside this iteration.
