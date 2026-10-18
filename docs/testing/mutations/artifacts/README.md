# FOR-13 artifact mutation assessment

This is one fixed assessment of `crates/crewroom-runtime/src/evidence/artifacts.rs`, not a mutation framework or a quality score. The five JSON patches freeze the inspected source preimage, exactly one replacement and resulting source hash. `config.json` binds their hashes and the one existing native fixture test.

Run the trial with Node 24 or newer on `PATH`, installed locked Node dependencies and Rust dependencies/build cache available:

```sh
python3 scripts/mutation-trial.py artifacts \
  --evidence-dir .crewroom/test-results/for13/mutation-new \
  --node "$(command -v node)"
```

The helper creates a detached private checkout at the current commit and copies the current evidence fixture into it, recording both identities. It uses that checkout's binaries. Identical locked dependency cache files may be copied with copy-on-write; all Crewroom cache artifacts are removed before the baseline build. The baseline builds `crewroom-process` and `repository-fixture`. Every mutant rebuilds and hashes its own `repository-fixture`, and the exact baseline source is restored between cases. The main artifact module is never patched.

One worker runs five fixed mutants with limits of 600 seconds for baseline setup/build, 180 seconds per subsequent build, 45 seconds per selected test and 1,800 seconds overall. A timeout is not a kill. Compilation failure is unviable. A kill requires the exact intended assertion failure, one executed test and a normal fixture exit. Skipped, cancelled, unrelated failure and unresolved child outcomes cannot establish detection. Raw stdout/stderr, commands, hashes, timing, fixture data and owned cleanup remain in the new owner-only evidence directory. Existing output is refused. Only the trial's disposable checkout is removed, after owned-process cleanup; source/test snapshots are retained first.

Inspect every survivor independently before changing tests. A recheck can run only the initial receipt's at most two surviving mutants after the fixture changes:

```sh
python3 scripts/mutation-trial.py artifacts \
  --evidence-dir .crewroom/test-results/for13/mutation-recheck \
  --node "$(command -v node)" \
  --recheck-from .crewroom/test-results/for13/mutation-new/receipt.json
```

The recheck retains the original trial's 30-minute deadline and builds a fresh unmodified baseline before the relevant mutants. An original receipt is private local evidence, not a command source; commands remain fixed in the helper. Recheck does not independently certify a survivor classification or the usefulness of an assertion change.

The selected fixture exercises local native artifact behavior. It does not select neighboring Docker/provider tests or claim live delivery. Fixture retention and explicit exit assertions activate only through `CREWROOM_MUTATION_RETENTION_DIR`; ordinary fixture teardown keeps its existing behavior. Two useful additional checks cover a false recorded byte count with otherwise valid content/hash and a legitimate exactly-4 MiB write/read roundtrip. The [2026-10-13 receipt](receipt-2026-10-13.json) distinguishes initial outcomes from their recheck and retains the independent survivor classification.

Helper guards run with `python3 -m unittest discover -s scripts -p test_mutation_trial.py -v`. They exercise fixed patch/preimage/path guards, exact restoration, selection and causal classification, non-overwrite, per-build binary rebinding and timeout/log retention. Disposable helper fixtures are not mutation results.
