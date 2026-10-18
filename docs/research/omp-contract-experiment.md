# OMP contract experiment

Date: 2026-09-25. This is the first bounded experiment after design approval, not acceptance of the complete runner contract or the live Jira-to-PR milestone.

## Results

| Boundary | Observation | Evidence |
| --- | --- | --- |
| Retired workflow mechanisms | All 13 selected cases passed in a disposable copy. Real Git repositories, SQLite, and subprocesses; synthetic OMP execution and provider replies. | [Reuse report](evidence/omp-reuse-2026-09-25.json) |
| Installed OMP control transport | OMP 18.3.1 accepted correlated JSON RPC commands and returned session identity and activity. | [Control report](evidence/omp-rpc-controls-2026-09-25.json) |
| Steering an idle session | Acknowledgement was followed by `isStreaming: true` and zero queued messages. Treat idle steering as an execution trigger. | [Initial rejected hypothesis](evidence/omp-rpc-initial-2026-09-25.json), [repeated observation](evidence/omp-rpc-controls-2026-09-25.json) |
| Interrupt | Immediately after steering, `abort` acknowledged; observed activity stopped and the assistant result reported `aborted`. | [Control report](evidence/omp-rpc-controls-2026-09-25.json) |
| Session reopen | After clean process shutdown, a fresh process reopened the same session ID and name. Both processes exited normally after stdin closed. | [Control report](evidence/omp-rpc-controls-2026-09-25.json) |
| Transcript persistence | The runtime reported two messages after abort and one after reopen. The aborted result is not established as durable by session identity alone. | [Control report](evidence/omp-rpc-controls-2026-09-25.json) |
| Crewroom runtime | `/opt/homebrew/opt/node/bin/node` reports 26.9.0; its npm reports 11.19.1. This satisfies Crewroom's Node 24 minimum. The default shell still selects Node 22. | Direct executable version checks |

The retained cases cover inherited ownership locks after controller death, uncertain interrupted attempts, stale/corrupt evidence, failed check gates, review provenance, lost PR-create replies, merge-dependent scheduling, concurrent status/admission, and continuation after a durable result without another synthetic model invocation. They establish useful regression cases to retain during adaptation; they do not adopt the old fixed phase sequence, vendor restrictions, or repair policy.

The RPC probe used a temporary working directory, private OMP state/session directories, disabled tools/extensions/skills/rules, and a filtered environment without provider credential variables. It sent no explicit `prompt` command, but idle `steer` started a turn. The isolated runtime selected a default Bedrock model; this is not an approved Crewroom model assignment or evidence of a successful provider request. The probe immediately aborted the turn. It did not validate Aperture authentication, successful model output, tool execution, or steering during useful work.

No application runner adapter has been adopted. The retired installation remains inactive; archived source and tests were copied into a temporary fixture and removed after execution. No the pilot repository files, Jira issues, or PRs were changed.

## Implementation consequences

1. Crewroom persists steering first and dispatches it only when execution is authorized. It must not send an idle `steer` merely to store a message while paused.
2. A command acknowledgement establishes transport acceptance. Application to the plan requires a separate observation; `isStreaming` alone says nothing about completed work or quality.
3. Preserve interruption observations and receipts in Crewroom's durable store. Reopening the same OMP session does not prove all prior observations survived.
4. Use the executable contract as the authority. The available global npm package reports version 17.4.0 while the executable reports 18.3.1. Its RPC source was useful for discovery but is not version-matched proof.
5. Carry the 13 retained cases into the implementation decision between a bounded Python component and a TypeScript port. That decision remains open until useful live execution and recovery are tested.

## Reproduce

From the Crewroom repository:

```sh
python3 scripts/check-omp-reuse.py \
  /Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot \
  --report .crewroom/experiments/omp-reuse.json

python3 scripts/probe-omp-rpc.py \
  --binary /Users/crewroom/.bun/bin/omp \
  --report .crewroom/experiments/omp-rpc-controls.json
```

The reuse report records SHA-256 identities of every copied source/test file. The RPC probe does not select a production model; its scope is control behavior with tools disabled. Application builds should select the supported runtime explicitly, for example `PATH=/opt/homebrew/opt/node/bin:$PATH npm run build`. the pilot repository keeps its own runtime requirements.

## Remaining contract work

- Select an available OMP/Aperture model and verify successful output, actual progress, steering during a turn, and interruption during tool execution in an owned disposable repository.
- Exercise abrupt controller/worker failure and prove ownership reconciliation before replacement. Clean EOF/reopen is not crash recovery.
- Verify completed evidence survives recovery without duplicate work, and demonstrate applied steering separately from receipt.
- Complete Jira/Bitbucket onboarding for the operator-selected the pilot repository ticket and build the minimum persistent workbench. No live provider or browser acceptance case has passed yet.
