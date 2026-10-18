# N7.1b native operations and installation preparation

This iteration completes headless upgrade/operations preparation after `431c19a1d6f84e959caaea36f5853fa941dbe9d1`. [The machine-readable receipt](rust-native-upgrades-operations-2026-10-04.json) binds source, fixture, binary and log hashes. Central N6/N7 acceptance and the default switch remain root-owned.

## Executed evidence

- The production native daemon with this branch's migrated store passed fresh configuration, actual OMP/Mail/Git/Docker coding, stopped whole-state capture, original-path restore and a separate completed research run. The inventory verified **693 entries** by contents, link targets and modes. It retained requirements, pending operator steering, three received messages, four sessions, candidate/check/review/artifacts and the owned worktree. All 16 host/OMP process receipts reported stopped; the Docker command receipt recorded removal; the operator checkout stayed unchanged. The 26 model requests were scripted, with expected model identities and no provider errors.
- Four actual schema-26 legacy audits passed: candidate/refresh split, missing source candidate, noncanonical conflict path and unknown legacy ownership. All refused conflict access while retaining the candidate, isolated contents and clean original checkout. See [the remediation contract](../native-installation.md#conservative-legacy-holds); no database repair or fabricated stopped proof is supplied.
- The official Zig 0.16.0 archive passed its pinned SHA-256 and version checks. Reuse checked the full installed file inventory. Rust is pinned to 1.97.1. The bootstrap uses private local storage and leaves the global Rust default unchanged.
- Packaging used the terminal agent's frozen binaries without repeating a full GPUI build. The signed bundle contains all four executables and dependency notices, rejects unbundled dynamic libraries, and passed deep strict signature verification. Its **1,620 source inputs match terminal commit `83a8389debdc40ae44063fb49c14a0e234cbd44d` exactly**. The receipt was captured just before that commit, so it retains the earlier base plus working-tree diff; an immutable Git archive comparison establishes the final binding. This is not the combined migration build.
- The signed packaged daemon then independently passed the same fresh configuration/restore/repeat-use exercise: **697 inventory entries**, 26 scripted requests, two Mail starts, three retained messages, four sessions, a separate completed research run, 16 stopped host/OMP receipts, stopped private tmux server, no process survivors and removed contained command. The signed four-executable total is **200,833,360 bytes**; this is a debug mechanics measurement, not the final release footprint.
- TypeScript/Vite build passed. The prepared retained browser suite passed **31/31** on isolated port 4339. Shell syntax, Python compilation and `git diff --check` passed. No Rust production source changed after the N7.1a tested commit, so its 142-test core/runtime/service gate was not repeated here.

## Failures preserved and corrections

The first fixture used an out-of-root tmux socket and native admission refused it. After aligning the socket with the native contract, backup rejected the stale socket left after tmux exited. The fixture now checks every exact process receipt and requires `no server running` before removing only that private socket from durable state.

The initial packaged repeat reached research but hit the existing optimistic read conflict, `Research evidence changed during verification.` The fixture now retries only that exact read-only 409, at most eight attempts; it never replays mutations. The subsequent complete run passed.

The first retained browser run passed 29/31; the two remaining fixtures lacked the checkout-local Mail executable. Only the known fixture binary was copied and hash verified. A second invocation correctly refused the main migration run's occupied port. The prepared isolated run passed 31/31. The retained browser log still records existing `ghostty-web` range errors; passing assertions do not establish a silent browser console.

The terminal agent's clean 332/332 domain run is also retained by log hash. Of its 350 input files, 349 match this worktree. The single exception is the native launcher shell, which `npm test` does not invoke. This is a shared retained-domain observation, not a claim of a fresh local 332 run or a byte-identical 350-file manifest. The prior N7.1a 331/332 plus successful focused retry remains recorded unchanged.

## Remaining integration work and self-evaluation

The assigned historical migrations, legacy audits, whole-state recovery and installation preparation now have concrete proof. Scope remains on track: conservative holds preserve recoverable state, package provenance is explicit, and no fixture is presented as live Jira/Bitbucket delivery.

Root still needs to integrate the upgrade, terminal and native UI commits, run the final combined release package from a fresh checkout, exercise its native flows/repeat use, reconcile the 87-case/12-scenario map, measure release footprint and then decide the native default/retirement cutover. Merely opening the app bundle does not yet provision a daemon; the explicit launcher controls installation paths. This iteration does not accept N7 as a whole or switch the repository default.
