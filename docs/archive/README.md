# Archived research

These reports record options and experiments that the accepted decisions have superseded. They are kept for their rationale and evidence, not as current guidance. Current direction is in [product.md](../product.md), [architecture.md](../architecture.md) and the [ADRs](../adr/); current state is in [status.md](../status.md).

| Report | Superseded by |
| --- | --- |
| [Agent communication](agent-communication-assessment.md), [agent protocols](agent-protocols-assessment.md), [IRC](irc-agent-communication-assessment.md), [OpenAgentEmail](openagentemail-assessment.md), [Agent Relay](agent-relay-assessment.md), [NTM Agent Mail](ntm-agent-mail-assessment.md) | [ADR 0004](../adr/0004-observe-independent-agent-processes.md) and [ADR 0005](../adr/0005-use-omp-tmux-and-rust-agent-mail.md): independent OMP processes with Rust Agent Mail |
| [Mailbox follow-up](agent-inbox-followup-2026-09-27.md), [complete local runtime proposal](complete-local-runtime-proposal-2026-09-27.md), [tmux supervision](tmux-supervision-assessment-2026-09-27.md) | ADR 0005 and the [selected runtime](../architecture.md#selected-runtime-technologies) |
| [Relaycast preparation](external-mailbox-preparation.md), [external round trip](omp-external-roundtrip-experiment.md) and its [review](external-roundtrip-review.md), [native OMP round trip](omp-roundtrip-experiment.md) | Rust Agent Mail results in [testing](../testing/rust-mail-results-2026-09-27.md) |
| [NTM fit](ntm-fit-assessment.md), [NTM workflow gates](ntm-workflow-gates.md) | The decision to keep the workflow independent of NTM ([product](../product.md#constraints)) |
| [Routing reuse](routing-reuse-assessment.md) | Coordinator-selected specialists from the user role catalog ([architecture](../architecture.md)) |

Evidence files cited by these reports are in [evidence/](evidence/). Research still in use stays in [docs/research](../research/).
