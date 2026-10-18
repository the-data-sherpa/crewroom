# Public repository publication — FOR-17

## Current publication approach

On 2026-10-18 the operator selected a new independent repository with a reviewed source snapshot as its initial commit. The existing GitHub repository was renamed `the-data-sherpa/crewroom-private-archive` and archived privately. The new public repository is [the-data-sherpa/crewroom](https://github.com/the-data-sherpa/crewroom), with a distinct GitHub repository identity and no inherited development history, pull-request refs or fork relationship.

Preparation starts from private development revision `5dcbafd0210884ab01adb8cc00245c560c297bfe`. The application source, vendored files, licenses and package scripts are preserved. Publication-specific edits explain the history boundary and make public-content handling explicit in the tracked [AGENTS.md](../AGENTS.md#public-repository) and [contribution guide](../CONTRIBUTING.md).

**Published and verified 2026-10-18 UTC.** Initial public commit `d9e2607a857950a4bb593534a7c0dc1b0a906269` has the exact reviewed source tree `47e041c04b09c12642741fab036e61e569f98556`. The [publication receipt](testing/for-17-publication-2026-10-18.json) records the source-preservation checks, secret-scan triage, independent reviews, 22 passing native-script tests, synthetic notice-packaging checks and web build. An anonymous fresh clone contained exactly one commit and passed Git integrity checks. Anonymous repository/README access succeeds; probes for old private objects and the former PR return 404 in the new repository. The private archive is inaccessible anonymously. This receipt and completion notes follow the initial snapshot as ordinary documentation changes.

## History and evidence

Public Git history starts with the reviewed snapshot. Earlier commit IDs and source hashes in `docs/status.md`, `docs/testing/` and research documents describe private development revisions. They do not resolve in the new public history. They remain historical evidence for the exact source and environment originally checked; they are not relabeled as fresh validation of the new initial commit.

Original development history, the earlier sanitization commit map, Git metadata and raw verification artifacts remain private. Some older receipts were sanitized in the earlier review and are marked accordingly. Historical links to the private archive and old machine paths are provenance, not public installation prerequisites. Outside contributors can run current checks through [Contributing](../CONTRIBUTING.md#check-your-changes), [the feature map](testing/feature-map/README.md) and [the current status](status.md).

Use a fresh clone of the public repository for new work. The existing maintainer checkout was migrated after privately preserving and verifying its old refs and Git metadata. The working directory, untracked files and runtime data remain in place. Keep old branches, tags, stashes, bundles and Git metadata out of the new repository; importing or merging them would defeat this publication boundary.

## Earlier removal approach

The 2026-10-14 documentation and MIT preparation added the public README, Arch installation guide, contribution/security guidance and third-party notices. Native packaging preserves required notices. Original Crewroom code is MIT; the core crate also retains the existing BSD-3-Clause V8 attribution. The [original preparation receipt](testing/for-17-publication-2026-10-14.json) records that bounded work.

The original private history contained confidential project/provider information and local test capability values. A private backup preceded rewriting 236 commits. The [history-sanitization receipt](testing/for-17-history-sanitization-2026-10-14.json) records the all-blob review and fresh affected checks. GitHub nevertheless retained an old pull-request ref and original objects. The [2026-10-18 recheck](testing/for-17-publication-recheck-2026-10-18.json) confirmed that those objects were still accessible to an authenticated maintainer while the repository was private. It also records zero findings in the 34 newer commits, 146 new blobs and commit metadata.

That hosted-removal blocker applies to making the historical repository public. The new independent repository avoids carrying those objects into publication; it does not erase them from the private archive. The archive must stay private. GitHub Support removal is no longer a prerequisite for publishing the new repository. No Support request was sent by this agent. GitHub describes retained objects and cached views in its [sensitive-data removal guidance](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository).

## Review limits

Secret scanning and targeted manual review cannot prove the absence of every unidentified private detail. A public source repository is not a portable binary release, live-provider certification or a complete application security audit. Existing native platform, external-fixture and provider-delivery limits remain as recorded in [status](status.md). Publication does not change product behavior or extend those acceptance claims.
