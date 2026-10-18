# UX Review — FOR-9

Reviewed 2026-10-13 against the FOR-9 working tree. Scope: four native working surfaces for selecting a model and inspecting its assignment. This is a source and interaction review, with a bounded Linux/Xwayland walkthrough. It is not an accessibility conformance audit or a user study.

Coverage: **4 of 4 scoped surfaces reviewed**. Source coverage is complete for the changed branches. Rendered coverage is partial: the walkthrough covers settings, role overrides and seeded history; launch states are source and GPUI evidence. Other workbench surfaces are outside this task.

| Surface and operator task | States reviewed | Result and evidence |
| --- | --- | --- |
| Models & budget: choose future assignment defaults | Empty catalog, refresh, success, stale/error, duplicate labels, blocked effort, unsaved draft, conflict, saved/reopened | **Pass.** `settings/models.rs:46` states that existing assignments retain their selections. `settings/model_picker.rs:341` separates Refresh from Save and shows each harness's result. `native_catalog_search_disabled_routes_drafts_and_restart` exercises search, disabled keyboard selection, saved jobs and 390px geometry. Rendered evidence: `models-idle.png`, `models-refreshed.png`, `narrow-final.png`. |
| Agent roles: inherit a job or override its model and effort | Inheritance, explicit override, unavailable choice, changed effort, dirty exit, saved/reopened | **Pass.** `settings/roles.rs:261` renders the shared preview. The catalog interaction test saves and reloads a role. Walkthrough: `role-inherited.png`, `role-disabled.png`, `dirty-guard.png`. |
| Assignment preview: understand what Start will admit | Resolved route, unavailable model, selection revision change | **Pass for source and service behavior.** `planning.rs:158` shows the effective role model; `planning.rs:229` captures its preview revision. `model_details.rs:61` exposes the service blocker. Catalog HTTP checks reject changed preview facts. **Skip:** a separate rendered live launch-dialog walkthrough; the live fixture admits through the authenticated service after native selection. |
| Active assignment and history: inspect the model that ran | New frozen selection, runtime observation, legacy OMP, Codex, Claude alias, unavailable current catalog | **Pass within the seeded-history scope.** `history.rs:181` uses frozen details; runtime observation remains separate at line 188. `native_catalog_refresh_and_reopen_keep_pre_catalog_history` checks all three old route shapes. Walkthrough: `history-frozen.png`. |

Source paths above are relative to `crates/crewroom-desktop/src/workbench`. Walkthrough files are retained under `/tmp/crewroom-mh4-native-k29idhjk`; they are local evidence, not bundled product assets. The final MH4 receipt records their hashes and the tested source.

## Resolved defects

- Keyboard Enter could confirm a disabled route. The delegate now vetoes disabled selection changes at `settings/model_picker.rs:125`. The interaction test attempts the blocked choice and asserts that saved intent remains selected.
- Wrapped explanations overlapped because the select list assumes equal row heights. `settings/model_picker.rs:70` measures the tallest row at the menu width. The test checks row bounds at 390px. `measured-narrow.png` and `narrow-final.png` show the final rendering; `wrapped-narrow.png` retains the earlier failure.
- The first role-picker placement disrupted the existing Tab order. The picker now follows Instructions. The focused existing keyboard-order check passes after the move.

## Applied checks

**Pass — clarity, feedback and defaults.** Model names include route details when labels match. Empty and failed discovery explain the next action. Refresh preserves drafts; Save uses revision checks. Inheritance is explicit. Saved legacy defaults remain identified until the operator changes them. Search does not require remembering a provider prefix.

**Pass — ethics gate.** Refresh does not start inference. Saving does not queue or start work. The budget explanation says it controls effort, not spending. Configured access is distinguished from successful inference. There is no urgency, scarcity, paid upgrade, hidden enrollment or asymmetric exit in these surfaces.

**Pass — navigation and ordinary keyboard use.** The settings title and selected navigation identify location. Existing unsaved-edit guards retain drafts. Search, selection, Escape and the role form's Tab order have GPUI coverage. Narrow rows keep reasons visible. These checks do not establish screen-reader behavior.

**Skip — acquisition and brand persuasion.** These are operator settings and evidence views, with no purchase or acquisition funnel. Anchoring, social proof, scarcity, paywall packaging and brand-message scoring do not apply.

**Skip — declared marketing intent.** No `BLUEPRINT.md` or `marketing/positioning.md` was found. Product intent comes from `docs/product.md`, the model contract and FOR-9.

**Skip — conditional platform checks.** No drag interaction, mobile tab bar, payment form or new authentication flow is introduced. OS assistive technology and input-method checks remain the previously accepted optional follow-ups. No usability study, conversion gain or broader platform acceptance is claimed.

Copy lint: strict mode scored 1.8 violations per 100 words for this report and 1.8 for the extracted visible microcopy. Table structure accounts for sentence/clause hits; the short status messages retain their clear subjects. Both scores are below the strict 2.0 threshold.


## FOR-10 intake review — 2026-10-13

Source `7fe0afa859c3cdbd2a15b379dbd15ef4be22bdc9`. Scope: native intake and plan review only. New state text separates queue intent, stopped questions, saved answers/delivery, accepted plan and actual agents. Plan editor uses named fields, readable packages and explicit save/rebind/accept decisions. Repaired stale pristine editors, terminal dialogue, unsent cancelled answers, duplicate editing surfaces and repeated accessibility identities. Dirty drafts are retained.

Full GPUI 53/53 plus final identity-delta workbench checks; 390px keyboard plan reject/edit/rebind/accept/queue and answer/restart tests. Actual Xwayland screenshots and action provenance: `/tmp/crewroom-mh5-validation/native-walkthrough.json`. Rendered 390px text/controls stayed within the viewport; long identifiers wrap. Answer saved/pending delivery and native acceptance/queue readbacks remain distinct from process execution. Review drafts/rejection partly staged via authenticated fixture API; manual Xwayland text injection was inconsistent. Screen reader, platform IME and new macOS/Wayland terminal behavior were not assessed.

No remaining identified release-blocking intake UX finding. This is not a whole-product UX audit or accessibility conformance certification.
