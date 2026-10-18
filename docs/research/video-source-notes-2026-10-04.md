# Video source notes: verification and agent workflows

Research date: 2026-10-04. Scope: the two YouTube links supplied by the operator. This records source observations and Crewroom implications; it changes no accepted requirements or implementation plan. The X post and pstack implementation are evaluated separately.

## Source access and confidence

Both videos are first-party interviews published by Matt Pocock. Full English automatic captions (`en-orig`, JSON3) and YouTube metadata were retrieved with the installed `yt-dlp`; both caption files were read through the end. No third-party summary substitutes for the interviews. Automatic captions can misrecognize names and technical terms; timestamps below are navigation points, not speaker-verified quotations. No audio-level transcription check or independent audit of the speakers' reported results was performed.

The web text extractor initially returned only YouTube boilerplate. The first caption request for the second video returned HTTP 429; a subsequent request for its original English track succeeded. There is no remaining transcript-access block.

| Source | YouTube metadata | Retrieved caption fingerprint |
| --- | --- | --- |
| [Lauren Tan / Poteto interview](https://www.youtube.com/watch?v=MN9dGgmLyso) | Released 2026-10-02; uploaded 2026-10-03; 1:05:36; no chapters returned | SHA-256 `76ebc0a0783bd1b10dac90757534457bda6f406aee30d61fc63ea533bd0f071a` |
| [Robert C. Martin / Uncle Bob interview](https://www.youtube.com/watch?v=zcLPGC-tvgk) | Released/uploaded 2026-08-19; 56:39; chapters returned | SHA-256 `0d3ca9719680e8b2ffd80bdb4901724fa2afe1af0e4453fbc085cccaeff09e0d` |

Raw captions and metadata remain temporary research inputs under `/tmp/crewroom-video-<video-id>.*`; full transcripts are not copied into the repository. Metadata dates distinguish publication from research time.

## Poteto: source observations

- [16:14](https://www.youtube.com/watch?v=MN9dGgmLyso&t=974s): executable application verification enables feedback and more autonomous work.
- [20:42](https://www.youtube.com/watch?v=MN9dGgmLyso&t=1242s): reusable CLIs handle deterministic mechanics; agents retain judgment.
- [30:10](https://www.youtube.com/watch?v=MN9dGgmLyso&t=1810s): conventions and lint rules constrain implementation; observed recurring failures motivate new constraints.
- [35:26](https://www.youtube.com/watch?v=MN9dGgmLyso&t=2126s): external context feeds coordinating agents. [47:32](https://www.youtube.com/watch?v=MN9dGgmLyso&t=2852s): buffering findings helps identify shared causes before fixes.
- [45:51](https://www.youtube.com/watch?v=MN9dGgmLyso&t=2751s): the reported 2,500 PRs include maintenance, not 2,500 features. [49:34](https://www.youtube.com/watch?v=MN9dGgmLyso&t=2974s): sampling accompanies environment improvements. [52:38](https://www.youtube.com/watch?v=MN9dGgmLyso&t=3158s): agents merge after verification; human review can follow landing.
- [56:28](https://www.youtube.com/watch?v=MN9dGgmLyso&t=3388s): applicability depends on verifiability. [1:00:35](https://www.youtube.com/watch?v=MN9dGgmLyso&t=3635s): mine intervention history into personally adapted skills.

These are reported practices, not audited throughput, defect-rate or economic evidence. Internal frameworks described here were not inspected.

## Uncle Bob: source observations

- [05:35](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=335s): Martin describes complexity/coverage scoring and mutation testing as agent feedback mechanisms. [12:55](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=775s): he prefers short instructions plus deterministic checks.
- [19:17](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=1157s): focused agents separate specification, coding, cleanup, hardening and UI QA; he acknowledges startup/context costs.
- [26:06](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=1566s): architecture inspection and enforceable dependency rules help; automating architectural judgment remains difficult.
- [33:56](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=2036s): he distinguishes engineering values from imposing human TDD rituals on agents.
- [36:16](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=2176s): he favors short implementation/feedback cycles over elaborate upfront plans. [42:38](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=2558s): his specifications are ephemeral and working artifacts become examples for adaptation.

Reported productivity improvements are personal experiments. Specific context-window explanations and complexity thresholds were not independently validated; neither should become a Crewroom default on this evidence alone.

## Crewroom assessment

The strongest application is to the quality of feedback available to specialists. Crewroom already requires actual checks, independent review and revision-specific evidence. These interviews support improving that evidence path, rather than treating more agents or larger PR counts as the objective. Useful evaluation measures would be accepted outcomes, escaped defects, repair cycles, operator intervention and elapsed/cost burden. This is our inference, not a measured conclusion from the videos. See the existing [product evidence contract](../product.md#evidence-required-for-coding-outcomes).

The approved [M8 redesign](../workbench-redesign.md) already includes model jobs/budget (S5), principles (S6), reviewed role learning (S7) and playbooks (S8). The interviews reinforce those decisions; they do not establish a missing parallel roadmap. Keep reusable principles concise, give playbook steps concrete outputs, and turn mechanically enforceable requirements into checked tools or domain invariants. Preserve the planned assignment snapshots and operator-reviewed changes.

Potential bounded experiments after the current slice order:

1. Evaluate whether a project verification recipe lets a specialist run the application and attach useful evidence without repeated operator setup. For the native workbench, the meaningful exercise is a real supported native flow, not browser-only evidence presented as equivalent.
2. Trial mutation analysis on one small, high-value deterministic module to discover tests that miss behavioral changes. Compare useful findings and runtime cost; do not impose universal 100% mutation scores or the speaker's complexity thresholds.
3. Mine a small set of retained steering and repair histories for repeated failures, then propose one reusable principle, tool check or role change through Crewroom's existing learning decision path. A recorded intervention is input for a proposal, not permission to rewrite user configuration.

Two approaches should not be imported into the current product contract: automatic merge/post-merge-only human sampling, and discarding accepted requirements/history. Crewroom explicitly keeps the human review/merge gate and durable requirement revisions. Likewise, automatic backlog selection and expanded coding concurrency require separate product decisions; the interviews supply no reason to override current admission and ownership rules. See [product direction](../product.md), [architecture](../architecture.md), [terminology](../../CONTEXT.md) and [current status](../status.md).

No code or active runtime state changed for this research; implementation test suites were not run.
