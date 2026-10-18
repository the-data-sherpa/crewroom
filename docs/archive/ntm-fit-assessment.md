# NTM fit for the desired workflow

Assessed 2026-09-25 through documentation and source inspection. NTM was not installed or run, and no OMP configuration was changed.

## Recommendation

NTM is a credible candidate for the visible, independent-agent team the user wants. A bounded pilot is justified; replacing the working OMP setup is not yet justified. Adopt only if the desired sequence fits NTM's existing workflow configuration and a few repository commands. Building another controller around it would repeat the maintenance burden of the retired `omp-team` experiment.

## What it provides

The project documents tmux agent panes, messaging, separate worktrees, task-graph assignment through `br`/`bv`, Agent Mail integration, saved sessions, and executable pipelines. These match the desired coordination experience. Several capabilities require companion tools. [README](https://github.com/Dicklesworthstone/ntm/blob/69c7195e2cee00d33f6c27bb9d768568aa254f0f/README.md)

Current source launches OMP itself and passes model and thinking settings through. Preserving the existing Aperture configuration therefore appears feasible, but has not been tested against the endpoint. The default OMP launch includes `--auto-approve`; choose this setting deliberately before a pilot. [OMP launch tests](https://github.com/Dicklesworthstone/ntm/blob/69c7195e2cee00d33f6c27bb9d768568aa254f0f/internal/config/omp_test.go)

Each agent retains its own context. NTM documents per-pane OMP context readings, native session resume, and handoffs. Its compaction implementation invokes native `/compact` and verifies a reduction; this does not create shared model memory. [Compaction implementation](https://github.com/Dicklesworthstone/ntm/blob/69c7195e2cee00d33f6c27bb9d768568aa254f0f/internal/context/compact.go), [README](https://github.com/Dicklesworthstone/ntm/blob/69c7195e2cee00d33f6c27bb9d768568aa254f0f/README.md)

## Limits that matter

Agent-step completion detects activity ending, not correctness. Shell-command stages can fail on nonzero exits, so tests must be explicit stages. Independent review, documentation completion, PR creation, and CI evidence need an explicit workflow contract. Inspection did not establish a turnkey GitHub/Bitbucket PR-and-CI adapter. See [workflow gate research](ntm-workflow-gates.md) and [pipeline executor](https://github.com/Dicklesworthstone/ntm/blob/69c7195e2cee00d33f6c27bb9d768568aa254f0f/internal/pipeline/executor.go).

The inspected development commit is `69c7195e2cee00d33f6c27bb9d768568aa254f0f`. The latest published release checked was v1.35.1 (2026-09-14), whose OMP support uses a plugin preset. Current development source has substantially broader native OMP support. Its parser fixtures reference OMP 18.2.3; the user's installed version was last verified as 18.3.1. A release install cannot be assumed to match the current README. [Release](https://github.com/Dicklesworthstone/ntm/releases/tag/v1.35.1), [released plugin](https://github.com/Dicklesworthstone/ntm/blob/v1.35.1/examples/agents/omp.toml), [current parser tests](https://github.com/Dicklesworthstone/ntm/blob/69c7195e2cee00d33f6c27bb9d768568aa254f0f/internal/agent/omp_test.go)

## Proposed pilot, not executed

Use one repository and three visible roles: research/coordinator, implementer, independent reviewer. Queue two small changes. Configure research → implementation → command-based tests → review/rework → documentation and final checks → PR/CI → next item. Prove that a deliberate failing test stops progression, an interruption resumes without duplicate work, and the second task starts only after the first reaches the declared completion boundary. Confirm OMP/Aperture launch and messaging before running this sequence.
