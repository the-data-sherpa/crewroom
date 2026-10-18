# Original scenario steps on the native runtime

The original M7 exit requires all 12 synthetic scenarios, including their steps in `scenarios.json`, not only their short expected-result summaries. The original release manifest composes multiple jobs for fault variants. The prior native mapping was too broad for three sequences: staged code/documentation with unit and Chromium checks, target refresh followed by both fresh checks and review, and a failed-check/blocking-review repair chain.

Those three sequences pass with actual native services, OMP, Rust Mail, Git and Docker. This document retains the **development proofs**; the subsequent signed-candidate execution is recorded in [the final scenario receipt](rust-native-final-scenarios-2026-10-04.md). The [machine-readable receipt](rust-native-original-scenarios-2026-10-04.json) records all 12 scenario decisions, exact fixture hashes, binaries, checks, independent reviews, provider boundaries and retained failures.

| Scenario | Native execution added |
| --- | --- |
| E2E-02 | Four actual agents, staged edits to code/UI/guide under one writer, three criteria, guide plan/read/assessment, actual unit and Chromium checks, independent review, one synthetic PR. |
| E2E-03 | Three distinct candidate SHAs; an actual failing unit check and inspected output; a passing check followed by a blocking review; corrected implementation with fresh passing check and independent review. Exactly two implementer repair cycles; exhausted-cycle refusal composes with the existing native recovery gate. |
| E2E-09 | Advance the actual Git target, refresh the candidate, rerun unit and Chromium checks, independently review the new revision and update the same PR. Retain current CI failure, reject late success for the old revision, and start no agents automatically. |

The other nine scenarios compose existing native receipts with their original boundaries. They are not relabelled as newly executed monolithic jobs. Final package/GUI/resource and OS accessibility/input checks remain separate gates.

The first complex attempt exposed an inherited PostgreSQL volume in the accepted Chromium fixture image. Docker added an extra anonymous mount, so native exact-ownership checks refused its receipts. `prepare-browser-check-image.py` preserves every original filesystem layer and every other image setting while removing only `config.Volumes`. It records the original and derived identities. No production ownership check changes. A second attempt completed the initial coding proof but the fixture probed a host after publication correctly stopped it; identities now use the persisted actual OMP startup observation plus independent model-wire checks. Both failures remain recorded.

The final fixture-source development runs are `/private/tmp/cr-n7-complex4` and `/private/tmp/cr-n7-rework2`. All 48 host/OMP ownership receipts are stopped, all seven command containers removed, and all 18 external Mail receipts match the native message identity, body, transport ID and acknowledgement. The runs made 108 scripted model requests; these are not live-provider quality or delivery claims.

To repeat against a candidate, first build the existing provider example from that candidate checkout with `cargo build --release --locked -p crewroom-service --example provider-daemon`. It uses the production Service/runtime and injects only synthetic provider endpoints and credentials. The production daemon deliberately has no fixture endpoint override. Select the packaged adjacent host/helper binaries independently:

```sh
python3 scripts/rust/prepare-browser-check-image.py --output /private/tmp/new-native-browser-image
python3 scripts/rust/verify-native-scenarios.py \
  --scenario target-drift --output /private/tmp/new-native-complex \
  --browser-image sha256:295b78a1314778cbc83bdabccfa94aadde23f7b2cb13482e2728b814fb1a1470 \
  --binary-directory /absolute/Crewroom.app/Contents/MacOS \
  --provider-daemon /absolute/candidate/target/release/examples/provider-daemon \
  --binary-source FULL_CANDIDATE_SHA --mail-binary /absolute/mcp-agent-mail
python3 scripts/rust/verify-native-scenarios.py \
  --scenario rework --output /private/tmp/new-native-rework \
  --binary-directory /absolute/Crewroom.app/Contents/MacOS \
  --binary-source FULL_CANDIDATE_SHA --mail-binary /absolute/mcp-agent-mail
```

Use new disposable output directories. The harness verifies image ancestry/configuration, requires the explicit candidate SHA and compares all three actual packaged runtime hashes to the adjacent final signed `.app.receipt.json` (the internal receipt is pre-signature provenance), hashes binaries again after execution, records fixture source independently, and checks exact process/container cleanup. It leaves its logs and state for inspection. Model responses and Bitbucket replies remain scripted throughout.
