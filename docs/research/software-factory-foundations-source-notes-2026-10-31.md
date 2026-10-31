# Software factory foundations: independent source notes

Research date: 2026-10-31. Initial independent evidence gathering, with source clarifications and additions during reciprocal review. Consensus on the report and these notes is complete; see the [decision ledger](software-factory-foundations-consensus-2026-10-31.md). This is not an implementation assessment. The source researcher inspected no repository code, architecture, status, prior research, git history, or implementation issues. Sources are public. **Source findings** below describe what publications say; **synthesis** identifies proposed reasoning and gates, not validated universal requirements.

## Meanings and historical continuity

**Source findings.** The industrial analogy predates CI/CD and AI. McIlroy's 1968 NATO conference contribution, published in 1969, argued for families of interchangeable, reliable software components. He distinguished manufacturing techniques from merely copying software, which was already easy. This establishes an early reuse-oriented foundation, not the first-ever use of “software factory.” [Author's transcription](https://www.cs.dartmouth.edu/~doug/components.txt).

The 2005 OOPSLA Software Factories workshop proposal described domain-specific languages, reusable assets, models, and product-line practices organized around a particular system family. Separately, SEI's 2004 Adoption Factory report proposed phased adoption of software product lines to manage organizational and financial risk. These historical meanings concern systematic production of related products, rather than an unrestricted machine for arbitrary software tasks. [Workshop proposal](https://softwarefactories.com/workshops/OOPSLA-2005/Software%20Factory%20Workshop%20Proposal.pdf); [SEI report](https://www.sei.cmu.edu/library/software-product-line-adoption-roadmap/).

Modern DoD usage encompasses people, processes, and tools delivering continuing value to an end-user community. Its Fundamentals distinguishes the broader factory, common platform capabilities, and CI/CD pipelines. It explicitly says DevSecOps does not intrinsically require containers, cloud, or a particular architecture, while acknowledging requirements in specific reference designs. [DoD overview](https://www.cloud.mil/devsecops/); [Fundamentals v2.5, §3.1](https://dodcio.defense.gov/Portals/0/Documents/Library/DoD%20Enterprise%20DevSecOps%20Fundamentals%20v2.5.pdf). Access limitations for these sources are recorded below.

**Synthesis: useful distinctions.**

| Term | Distinct purpose | Boundary |
| --- | --- | --- |
| Software factory | A repeatable organizational system that turns selected needs into supported software and learns from use | Requires more than generating code or producing artifacts |
| CI/CD | Integration, verification, packaging, and release capabilities | Can support a factory without deciding product priorities |
| Internal platform | Shared capabilities presented as a product to internal users | Enables delivery teams; does not necessarily own their outcomes |
| Agent orchestrator | Coordinates model-driven actions, tools, state, or multiple agents | One possible mechanism within the delivery system |

DORA distinguishes continuous delivery—being able to release safely on demand—from continuous deployment of changes. A factory can retain deliberate release decisions. CNCF describes platforms around user needs and consistent access to shared capabilities. Anthropic distinguishes predefined workflows from agents that dynamically select their actions. These are compatible concepts with different scopes, not competing names for the same product. [DORA delivery](https://dora.dev/capabilities/continuous-delivery/); [CNCF platforms](https://tag-app-delivery.cncf.io/whitepapers/platforms/); [Anthropic agent patterns](https://www.anthropic.com/research/building-effective-agents).

## Foundations before scale

**Social and product foundations.** DORA associates high-trust information flow, shared responsibility, and inquiry after failure with stronger delivery outcomes; its guidance includes cross-functional collaboration and developers sharing production responsibility. This is research-informed guidance, not proof that a specific team structure causes success everywhere. [DORA culture](https://dora.dev/capabilities/generative-organizational-culture/).

**Synthesis:** establish an identifiable user, a bounded class of valuable changes, someone authorized to prioritize them, explicit acceptance criteria, and ownership after release. Provide a route for operators to report uncertainty and stop work. Roles can coexist in a small team; the evidence does not mandate separate departments. Funding maintenance, review, and incident response matters alongside funding new features.

**Process and technical foundations.** DORA emphasizes deployability, fast feedback, automated testing and deployment, pervasive security, and loosely coupled delivery. Google SRE recommends repeatable builds, automated release steps, and small changes. [DORA delivery practices](https://dora.dev/capabilities/continuous-delivery/); [SRE release engineering](https://sre.google/workbook/canarying-releases/).

**Synthesis:** make work inspectable from intent to source revision, checks, artifact, release, and observed result. Record who can change scope and authorize consequential actions. Define how failed verification, ambiguous requirements, or operational regressions return to the work queue. A person may perform early steps, provided the steps are explicit and their results survive handoff.

**Security foundations.** NIST SSDF 1.1 organizes practices into organizational preparation, protecting software, producing secure software, and vulnerability response. It calls for risk-based tailoring rather than treating every example as a universal checklist. Responsibilities, protected environments, verification criteria, and retained release evidence belong in the lifecycle. [Final SSDF, §§1.3–2](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-218.pdf).

SLSA adds incremental supply-chain integrity guarantees and provenance. It explicitly does not establish source-code quality or automatically cover transitive dependencies. A signed artifact may faithfully contain a vulnerable program. [Approved SLSA v1.2](https://slsa.dev/spec/v1.2/); [scope and exclusions](https://slsa.dev/spec/v1.2/about).

## Smallest end-to-end viable factory

**Synthesis, not a published standard:** start with one product, one change class, one delivery path, and named operating responsibility. The smallest credible demonstration closes this loop:

1. Select a real user need and record an observable acceptance condition.
2. Implement a small revision through an established development toolchain.
3. Run relevant checks and review; associate evidence with that revision.
4. Produce an identifiable artifact and release it under the chosen authority.
5. Observe whether the intended behavior works for its users.
6. Exercise recovery and turn a finding into a subsequent change.

The output can be a service, package, desktop application, or other supported deliverable. Production need not mean public internet exposure. A staging-only demonstration establishes pipeline feasibility, but leaves real adoption and operational feedback unproven. Automated deployment mechanics can coexist with human authorization. The proposal follows the broader DoD factory scope and SRE's operational treatment of releases; neither source names this exact minimum. [DoD definition](https://www.cloud.mil/devsecops/); [SRE](https://sre.google/workbook/canarying-releases/).

An AI agent is optional for this baseline. For an explicitly AI-enabled factory, introduce one bounded agent assignment within the loop and compare it against the existing way of completing equivalent work. A successful code patch alone does not demonstrate release capability, supportability, or user value.

## Staged expansion and observable exit gates

**Proposed sequence.** These gates are engineering hypotheses to tailor to risk and context. They are not a maturity certification. CNCF explicitly warns that higher maturity consumes resources and is not an end in itself; SEI's adoption work also supports staged investment. [CNCF maturity model](https://tag-app-delivery.cncf.io/whitepapers/platform-eng-maturity-model/); [SEI adoption roadmap](https://www.sei.cmu.edu/library/software-product-line-adoption-roadmap/).

| Stage | Bounded investment | Observable evidence before expanding |
| --- | --- | --- |
| 0. Define the service | User, change class, ownership, risks, acceptance, baseline | A representative change has an agreed outcome, accountable owner, release route, and recorded current effort and delays |
| 1. Close the loop | Existing tools, small change, verification, release, observation | A user-visible change is released; evidence identifies its revision and artifact; a recovery exercise and feedback-driven follow-up complete |
| 2. Make it dependable | Automate recurring friction; clarify failure states; protect credentials and artifacts | A fresh execution follows documented inputs; failed checks block promotion; interruption and ambiguous external outcomes are reconciled without silent loss or duplication |
| 3. Evaluate bounded AI | One agent or simple workflow, representative tasks, permissions, traces | Comparison covers accepted outcomes, human effort, cost, latency, and failures; stop/escalation and prohibited-action cases are exercised |
| 4. Expand autonomy selectively | Broader permissions or longer assignments where justified | New action classes meet explicit risk criteria; restart, cancellation, stale approvals, and changed requirements are tested; operators can intervene and inspect state |
| 5. Productize shared capabilities | Reuse across real consumers; specialized agents only where beneficial | Another consumer uses the capability with documented support; benefit survives integration, operating cost, and coordination overhead |

Security and recovery are present from the first meaningful release; later stages deepen them. Stages can overlap, and an existing reliable delivery platform can satisfy earlier gates. Repeated demonstrations should cover representative variation and failure cases. Neither a universal number of successful runs nor a universal success percentage follows from these sources. Set thresholds before evaluation according to consequence, workload, baseline uncertainty, and acceptable operating cost.

## What to defer, and what to reuse

**Source findings.** CNCF says platforms should provide the thinnest reasonable layer over capabilities and need not implement their backing services. Its maturity model begins with needs and fit before broad scaling. Anthropic recommends simpler solutions until complexity is justified, while warning that frameworks can obscure execution. [CNCF platforms](https://tag-app-delivery.cncf.io/whitepapers/platforms/); [maturity model](https://tag-app-delivery.cncf.io/whitepapers/platform-eng-maturity-model/); [agent patterns](https://www.anthropic.com/research/building-effective-agents).

**Synthesis:** ordinarily defer a universal workflow language, a general agent marketplace, extensive dashboards, broad integration catalogs, bespoke CI infrastructure, and a large hierarchy of agent roles until concrete users and recurring constraints justify them. Avoid making a portal, Kubernetes adoption, or an agent count the definition of completion. These are sequencing judgments, not claims that such investments are inherently unnecessary.

Prefer existing source hosting, CI, identity, secret storage, artifact registries, telemetry, deployment tooling, and workflow/runtime services when their contracts fit. Build domain-specific decisions, missing interfaces, or evidence handling only where they create demonstrable value. Evaluate reuse by operational responsibility, isolation, permission granularity, interruption behavior, exportability, integration effort, licensing, and lifecycle cost—not just feature lists. NIST's shared-responsibility guidance makes clear that using a provider does not eliminate the consumer's security responsibilities. [SSDF §1.3](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-218.pdf).

**Counterargument:** multiple products with established common needs, strict isolation constraints, or an existing platform team can justify shared infrastructure early. Deferring every abstraction can produce expensive divergence. Historical product-line approaches deliberately invest in common assets; their relevance increases when a stable product family actually exists. [2005 factory proposal](https://softwarefactories.com/workshops/OOPSLA-2005/Software%20Factory%20Workshop%20Proposal.pdf).

## Operations, autonomy, and durable recovery

Google SRE treats canarying as limited exposure followed by evaluation before wider rollout. Its example error-budget policy connects reliability evidence to prioritizing reliability work. The example's numerical limits are illustrative, not general service requirements. [Canarying](https://sre.google/workbook/canarying-releases/); [example policy](https://sre.google/workbook/error-budget-policy/). SEI connects continuous authorization to ongoing security monitoring, traceable evidence, and responsible risk decisions—not simply installing a pipeline. [SEI continuous ATO](https://www.sei.cmu.edu/blog/the-role-of-devsecops-in-continuous-authority-to-operate/).

**Synthesis:** choose user-relevant health signals and a release policy before broad exposure. Include support requests, regressions, incidents, and vulnerabilities in prioritization. For migrations or destructive operations, recovery may require forward repair or compensation rather than rollback. Record which recovery mechanisms were actually exercised.

OpenAI recommends maximizing a single agent first, splitting when instructions or tool selection become difficult, and providing human intervention for repeated failure or consequential actions. This is practitioner guidance rather than a controlled comparison of all architectures. [Practical guide](https://openai.com/business/guides-and-resources/a-practical-guide-to-building-ai-agents/).

**Synthesis:** autonomy is a set of separately granted capabilities: reading information, editing files, running code, publishing, merging, deploying, or deleting. Enforce authorization at tool and execution boundaries. Keep untrusted task content from granting permissions, bind approvals to the intended action and relevant revision, and make cancellation observable. A confirmation prompt is insufficient if the execution environment already exposes unrestricted credentials.

Anthropic's April 2026 Managed Agents account describes separating durable session events, the agent loop, and execution sandboxes. It keeps service credentials outside the sandbox and reports that model improvements can make earlier harness workarounds obsolete. This supports reviewing custom infrastructure for continuing necessity, not blindly reproducing that service architecture. [Managed Agents engineering account](https://www.anthropic.com/research/managed-agents).

Its earlier long-running-agent work used explicit progress artifacts, incremental implementation, and verification across context windows; it found compaction alone insufficient in its experiments. These techniques address continuity, but do not prove safe execution of external side effects. [Long-running harnesses](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents).

Amazon explains why retries after uncertain outcomes need idempotent API semantics, including caller-supplied request identifiers and atomic handling of identifiers with mutations. [Builders' Library](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/).

**Synthesis:** durable recovery needs both work-state continuity and side-effect reconciliation. Persist intent, execution identity, evidence, and confirmed outcomes separately. Test a crash after an external action succeeds but before acknowledgment is recorded. Resume by checking external truth; do not blindly replay a transcript. Where the provider cannot deduplicate or expose sufficient state, stop for reconciliation. An event log alone does not establish exactly-once effects.

## Agent evaluation and one versus many

Anthropic distinguishes tasks, repeated trials, traces, and actual environmental outcomes. It recommends combining deterministic, model, and human grading according to the problem and calibrating model graders. Its guidance distinguishes capability exploration from regression protection and evaluates the model together with its harness. [Agent evaluations](https://www.anthropic.com/research/demystifying-evals-for-ai-agents).

**Synthesis:** keep representative successful, failing, ambiguous, adversarial, and recovery tasks. Record model version, prompts, tools, environment, source revision, budgets, and grader version. Evaluate resulting behavior and unauthorized effects, not merely a confident completion statement. Protect acceptance checks from being weakened by the implementation under test. Include human review and correction effort, and preserve unexpected failures as regression cases. Sampling uncertainty and grader disagreement should be visible.

There is positive evidence for multiple agents in some workloads. Anthropic reported a 90.2% improvement over a single-agent baseline on an internal research evaluation, alongside substantially greater token use and difficulty with tightly coupled tasks. The comparison involved different model compositions and resource use; it does not isolate agent count or establish coding productivity gains. [Multi-agent research account](https://www.anthropic.com/research/multi-agent-research-system).

**Synthesis:** favor one agent for coupled changes sharing substantial context; investigate multiple agents for separable investigation or specialist work. Compare end-to-end quality, latency, cost, and reconciliation effort under declared resource budgets. Agent agreement is not independent proof: shared models, prompts, or assumptions can produce correlated mistakes. Role labels alone establish neither expertise nor review independence.

## Metrics, empirical disagreement, and uncertainties

DORA's current guide has five delivery measures: change lead time, deployment frequency, failed deployment recovery time, change fail rate, and deployment rework rate. It warns against disparate service comparisons, targets that invite gaming, and measurement infrastructure that crowds out improvement. [DORA metrics](https://dora.dev/guides/dora-metrics/).

DORA's 2025 report combines interviews and survey responses from nearly 5,000 technology professionals and characterizes AI as amplifying organizational strengths and weaknesses. **Interpretation:** these organizational associations help generate hypotheses; they are not randomized proof of a specific factory design. [Research summary](https://research.google/pubs/dora-2025-state-of-ai-assisted-software-development-report/).

Randomized studies do not yield one universal AI productivity effect. Cui and colleagues report a pooled 26.08% increase in completed tasks, standard error 10.3%, across experiments involving 4,867 developers. The intervention was access to a coding assistant; this does not measure autonomous factories. [Research paper landing page](https://www.microsoft.com/en-us/research/publication/the-effects-of-generative-ai-on-high-skilled-work-evidence-from-three-field-experiments-with-software-developers/).

METR's early-2025 experiment involved 16 experienced open-source developers and 246 tasks in familiar repositories; AI access increased task time by 19%. METR explicitly limits the result to that setting and period. Its February 2026 update says later data are unreliable for estimating current gains because of participant/task selection and measurement problems with concurrent agents. It believes selection likely understates benefit and speedups have probably increased; the magnitude remains uncertain. Treating the early slowdown as timeless, or the later results as a clean reversal, would both overstate the evidence. [Initial study](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/); [methodological update](https://metr.org/blog/2026-02-24-uplift-update/).

**Synthesis:** measure accepted user outcomes and operational quality alongside delivery flow, human attention, review/rework, elapsed time, agent cost, intervention, and recovery. Count unsuccessful and abandoned attempts. Distinguish throughput from active labor time and calendar delay. Lines generated, tokens consumed, agents launched, or PRs opened are activity signals, not sufficient value measures.

Material uncertainties remain: long-term maintenance cost, transfer across domains and model generations, rare harmful actions, and the causal benefit of multi-agent coding under matched budgets. This review is selective rather than a systematic literature review. Vendor engineering reports supply useful mechanisms but have selection and commercial biases; standards prescribe practices rather than proving return on investment. Regulated, embedded, disconnected, and safety-critical deployments can require earlier assurance investment. Local representative trials remain necessary before selecting an autonomy level or expansion rate.

## Source ledger and access limits

Publication status refers to the cited version observed during research; living pages can change. “Guidance” is not certification, and a preprint is not described here as peer-reviewed.

| Source | Publication status and evidentiary role |
| --- | --- |
| [McIlroy](https://www.cs.dartmouth.edu/~doug/components.txt) | 1968 address, 1969 proceedings; author's 1998 corrected transcription. Fetched directly in memory after browser fetch failed; historical argument, not outcome study. |
| [OOPSLA factory proposal](https://softwarefactories.com/workshops/OOPSLA-2005/Software%20Factory%20Workshop%20Proposal.pdf) | 2005 workshop proposal by proponents; primary definition and agenda, not empirical validation. |
| [SEI adoption roadmap](https://www.sei.cmu.edu/library/software-product-line-adoption-roadmap/) | Published technical report, September 2004; abstract inspected, not full report. |
| [DoD overview](https://www.cloud.mil/devsecops/) / [Fundamentals v2.5](https://dodcio.defense.gov/Portals/0/Documents/Library/DoD%20Enterprise%20DevSecOps%20Fundamentals%20v2.5.pdf) | Official overview and October 2024 guidance. Indexed primary-source passages inspected; direct fetches returned 403. No full-document review claimed. Date corroborated in [DoD's January 2025 guidebook references](https://www.cto.mil/wp-content/uploads/2025/08/SW-DT_E-in-DevSecOps-Guidebook-Jan-2025-1.pdf). |
| [SEI continuous ATO](https://www.sei.cmu.edu/blog/the-role-of-devsecops-in-continuous-authority-to-operate/) | October 2021 expert engineering article; government authorization context. |
| [DORA delivery](https://dora.dev/capabilities/continuous-delivery/) / [culture](https://dora.dev/capabilities/generative-organizational-culture/) / [metrics](https://dora.dev/guides/dora-metrics/) | Living research-informed guidance; current metrics page differs from older four-metric references. |
| [DORA 2025](https://research.google/pubs/dora-2025-state-of-ai-assisted-software-development-report/) | Published industry research report; first-party summary inspected. Full partner-hosted PDF exceeded fetch size; detailed methodology not audited. |
| [SSDF 1.1](https://csrc.nist.gov/pubs/sp/800/218/final) / [1.2](https://csrc.nist.gov/pubs/sp/800/218/r1/ipd) | 1.1: final, February 2022, substantive PDF inspected. 1.2: December 2025 initial public draft on official page, comment period closed; not treated as final. |
| [SLSA v1.2](https://slsa.dev/spec/v1.2/) | Explicitly Approved specification; security scope and limits inspected. |
| [CNCF platforms](https://tag-app-delivery.cncf.io/whitepapers/platforms/) / [maturity](https://tag-app-delivery.cncf.io/whitepapers/platform-eng-maturity-model/) | Published community white papers, living HTML; practitioner consensus, not experimental proof. |
| [SRE release](https://sre.google/workbook/canarying-releases/) / [policy](https://sre.google/workbook/error-budget-policy/) | Published 2018 workbook chapter and explicitly illustrative policy. |
| [Anthropic patterns](https://www.anthropic.com/research/building-effective-agents) | December 2024 engineering guidance; current page warns tooling has changed and points to newer work. |
| [Long-running harnesses](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents) / [evaluations](https://www.anthropic.com/research/demystifying-evals-for-ai-agents) | November 2025 / January 2026 vendor engineering accounts. |
| [Managed Agents](https://www.anthropic.com/research/managed-agents) / [multi-agent research](https://www.anthropic.com/research/multi-agent-research-system) | April 2026 / June 2025 implementation reports; not independent comparative trials. |
| [OpenAI practical guide](https://openai.com/business/guides-and-resources/a-practical-guide-to-building-ai-agents/) | Page displays January 15, 2026, confirmed directly by parent during consensus review. This is the web page's displayed date, not a claim about the guide's original publication. Deployment-derived recommendations. |
| [AWS idempotency](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/) | Published Builders' Library engineering article; no publication date established from inspected text. |
| [Cui et al.](https://www.microsoft.com/en-us/research/publication/the-effects-of-generative-ai-on-high-skilled-work-evidence-from-three-field-experiments-with-software-developers/) | June 2025 research listing explicitly links a preprint; abstract inspected, underlying analysis not independently reproduced. |
| [METR study](https://arxiv.org/abs/2507.09089) / [update](https://metr.org/blog/2026-02-24-uplift-update/) | July 2025 preprint and February 2026 methodological update; first-party study explanations inspected. |
| [StrongDM factory account](https://factory.strongdm.ai/) / [principles](https://factory.strongdm.ai/principles) / [techniques](https://factory.strongdm.ai/techniques) | Added during reciprocal review; February 6, 2026 first-party account and companion pages read directly by parent. Practitioner design choices, not controlled comparative evidence. |
| [Anthropic containment](https://www.anthropic.com/research/how-we-contain-claude) | Added during reciprocal review; May 25, 2026 vendor engineering account, read directly by parent. Reports approval fatigue and permitted-channel exfiltration; not a universal efficacy guarantee. |

No repository conclusions, tracker updates, commits, or publication actions are part of these notes.
