# Pause/resume

Key: `pause-resume`. Aliases: pause, resume. Covers UI-04, LIF-05 and the explicit Resume portions of LIF-06/E2E-05. Read [shared setup, identity, isolation, outcomes and cleanup](README.md) first. Platform: Linux source/debug, headless native GPUI with the production Rust daemon and separately supervised scripted RPC coordinator. Docker and live provider access are not required.

```sh
python3 scripts/verify-feature.py check pause-resume
python3 scripts/verify-feature.py run pause-resume --evidence-dir .crewroom/test-results/f12p1
```

## Actions and observable outcomes

1. The fixture creates private state and starts bounded scripted work. Retain its progress artifact and verify the current host/runner ownership descriptor (`owned-active-progress`). The runner is deliberately held so stopping is observable; this fixture fault is not a user action.
2. Select the fixture ticket and activate native **Pause** (`pause-run`). The workbench must first show **Pausing** while ownership is unstopped (`native-pausing-before-stopped`). It may show **Paused** only after every owned attempt is stopped (`paused-owned-stop`).
3. Close/reopen the isolated daemon and native client, select the same ticket and open **History**. Select the previous attempt (`history-attempt-<id>`), inspect it, then activate **Back to current run** (`back-to-current-run`). Compare run identity, attempt history and progress artifact (`reopen-preserves-history`). Restart must not silently admit execution.
4. Activate native **Resume** (`resume-run`). Observe running state and continuation (`native-explicit-resume`); compare current attempt/process identities and further progress with the stopped originals (`resumed-progress-no-old-writer`). A replacement attempt retains history and is reported as replacement, not same-session recovery.
5. Finish through owned cleanup and verify the daemon exits successfully with host/runner ownership stopped.

## Evidence and diagnosis

The helper requires all six assertion IDs, actual native Pause/Resume control traces, before/after durable runtime snapshots, retained progress and successful cleanup. Read native action results/logs, journey process identities and private `feature-cleanup.json` below the receipt's fixture root. PID existence alone is not ownership; paused must agree with checked stopped descriptors. This recipe excludes Cancel, specialist insertion, PR feedback and full-state restore.

Follow [receipt recovery and bounded rerun rules](README.md#evidence-cleanup-and-reruns). Preserve intermediate pausing failures; a later stopped process cannot retroactively prove the missing native observation. Every rerun needs a new evidence directory.
