# Readiness invalidation after a revision change

Key: `readiness-invalidation`. Aliases: readiness, revision drift. Covers EVD-04 and the revision-drift portion of E2E-09. Read [shared setup, identity, isolation, outcomes and cleanup](README.md) first. Platform: Linux source/debug, headless native GPUI, production Rust daemon, real disposable Git/Docker checks and scripted independent review/RPC/Mail. The review validates the workflow contract; it is not a live model assessment or hosted-provider certification.

Select the pinned local Docker image described in shared setup before preflight:

```sh
export CREWROOM_CHECK_IMAGE=sha256:3638d9a6fe4030bd716be989438248074489337ba3275657f93595428be4fc03
python3 scripts/verify-feature.py check readiness-invalidation
python3 scripts/verify-feature.py run readiness-invalidation --evidence-dir .crewroom/test-results/f12r1
```

## Actions and observable outcomes

1. The existing contributed-pair fixture prepares a disposable coding candidate through authenticated workflow tools, runs its configured real container check and records an independent scripted reviewer verdict for that exact revision. Assert `ready=true` with matching candidate, check and review (`ready-candidate-real-check-review`). No pre-seeded ready flag substitutes for this starting state.
2. Select the fixture ticket and native **Results**. Observe current ready/local-validation presentation and its revision (`native-ready-results`). Stop the fixture's admitted agents and verify stopped ownership before refresh (`stopped-before-refresh`).
3. The setup helper adds one nonconflicting commit to the fixture's own target repository. It does not change this development checkout. Select native **Refresh target** (`target-refresh`) and wait for the actual repository refresh to complete (`native-target-refresh`).
4. Inspect native **Results** and the durable coding assessment. The candidate/source base, head and tree must reflect the revision change; readiness must be false with an explicit evidence blocker (`revision-invalidates-readiness`).
5. Compare the retained old candidate, checks and review with their original revision identities (`historical-evidence-retained`). They remain historical and cannot validate the changed source. No agent restarts automatically; finish with checked owned cleanup.

## Evidence and diagnosis

The helper requires all six assertion IDs, a positive ready starting state, the native Refresh target trace, Git identities, container check/review records, negative assessment and verified cleanup. Read the journey, native Results labels/action output, private coding snapshots and `feature-cleanup.json` under the receipt's fixture root. A run that was never ready cannot pass this recipe. Missing Docker/image is unavailable, not a skipped successful check.

This recipe ends at invalidation; restoring readiness, conflict resolution, reimplementation, PR updates and hosted CI are separate cases. Follow [receipt recovery and bounded rerun rules](README.md#evidence-cleanup-and-reruns), retaining all failed attempts and using a fresh evidence directory for corrections.
