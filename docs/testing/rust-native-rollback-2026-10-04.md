# Matching-build rollback — 2026-10-04

The [executed receipt](rust-native-rollback-2026-10-04.json) proves a disposable old-build rollback with the accepted M7 installation revision `965a563b6a42dcc5b4511927585644fe08f4d023`. The old source archive, lockfile dependencies and production build are preserved locally with a **7,793-entry** content/link/mode inventory. Fresh `npm ci` and build passed; both old-server starts used that unchanged build and Node 26.10.0.

The fixture configured a local repository and reusable role, accepted two briefs, queued two plans, and saved two pending steering messages through the old API. No runner or provider was configured. After confirmed shutdown, a **58-entry** whole-installation snapshot captured data, role configuration, repository Git metadata, an untracked file and a symlink. It was verified before native startup.

The production native daemon opened the same paths and preserved every selected task, role, repository, queue and steering fact. It then saved a new task, revised a brief and role, and added steering. After native shutdown, that **62-entry** newer state was preserved separately. The original snapshot was restored to its exact original absolute paths and verified before launching the matching old build. The old build recovered the original facts and successfully saved another local task. The newer native state and original backup remained unchanged. All three owned server processes exited with code zero; no additional process referenced the fixture afterward.

This closes the bounded **old-build plus pre-native snapshot** rollback gate. It does not claim that old code can open newer state, that `user_version` can be lowered, or that active legacy processes can be adopted. Both builds use schema 26. [Historical upgrade evidence](rust-native-upgrades-2026-10-03.json) covers earlier schemas; [native operations evidence](rust-native-upgrades-operations-2026-10-04.json) separately proves full native sessions/Mail/worktree/evidence recovery and repeat use.

Reproduce with the three already-built native service executables:

```sh
python3 scripts/test-native-rollback.py --output /private/tmp/cr-native-rollback
```

The output must be new. The script preserves its old build, source archive, original snapshot, newer state, inventories and logs. It runs only disposable local APIs with explicit private paths and unconfigured integrations. The current native binary hashes exactly match the prior runtime acceptance receipt; final combined release packaging and resource measurements remain root-owned gates. The actual primary data, retained pilot, the pilot repository and retired installation were not used.

Progress and self-evaluation: the requested rollback exercise is complete and pushed with the implementation. The proof is deliberately limited to safe unadmitted legacy state; combined with the already executed native whole-runtime restore, it covers the two distinct recovery obligations without claiming unsupported active legacy adoption.
