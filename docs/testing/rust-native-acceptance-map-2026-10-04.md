# Native acceptance evidence map — 2026-10-04

The [machine-readable map](rust-native-acceptance-map-2026-10-04.json) accounts for every **87 behavior cases, 12 scenarios and 31 browser-equivalent journeys** at frozen main commit `e03459580765ee39af89803304b9fb2758de6221`. Each entry preserves its original identity and expected behavior, describes the assertions supported by native evidence, and names remaining gaps. The 42 referenced receipts and all native test source pointers carry SHA-256 hashes.

This is an evidence audit, not a new test run or an acceptance percentage. Of the 87 behavior entries, 78 map to existing bounded evidence and nine have explicit remaining assertions. Of the 31 native journeys, 16 map to bounded evidence and 15 have explicit remaining assertions. All 12 named scenarios compose existing component evidence but still lack an executed native certificate for their named model profile. A mapped entry does not certify a later release candidate or silently broaden an earlier receipt.

The runtime gaps are:

- **G01 / RTE-01:** all 950 pinned role/model bindings through the native boundary. Representative protocol checks do not cover the full catalog.
- **G02 / RTE-02:** the accepted baseline, heterogeneous and rotated profiles, with recorded per-process identity. Existing native actual-OMP runs cover representative models and fallback.
- **G03 / MSG-01:** coordinator → specialist A → specialist B → A → coordinator through actual external Mail. The native daemon receipt proves one peer handoff.
- **G04 / LIM-04:** reproducible seeded control/duplicate/crash/reply schedules and retained event traces. Fixed differential and crash fixtures are valuable, but do not prove randomized replay.

The native UI gaps are G05–G14 in the JSON: complete narrow keyboard and OS accessibility/IME checks; steering status and legacy-time labels; fallback model labels and observation outages; busy insertion/pausing; same-PR Resume; captured retired repository history; queue release/dependency transitions; the combined M1 restart sequence; the unowned cancelled-plan variant; and direct native automatic admission. Root owns these follow-ups; the terminal agent owns focused display/focus coverage. The five new positive GPUI journeys substantially cover the underlying functionality but use stopped attempt records and scripted provider replies. They do not claim live agent execution or live provider delivery.

Remaining release work is separate: integrate final sources and notices, run the prepared fresh-checkout release/install gate, measure idle/four-agent resources, execute matching-old-build/pre-cutover-snapshot rollback, and make the root-owned default/retirement decision. Same-build whole-state restore is already recorded; it is not a downgrade proof. Final notices must include the new Apache-2.0 `gpui-component` and `gpui-pre` vendors and their provenance.

Validation of this documentation checked exact membership/counts, unique IDs, original expected values, receipt hashes at the frozen commit, every referenced native function, browser source hashes, and gap-reference integrity. No application tests or operating-system processes were launched for this audit. Later UI/focus work must update or supersede this frozen map with its own evidence.
