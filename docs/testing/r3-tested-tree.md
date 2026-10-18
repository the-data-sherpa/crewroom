# R3 tested tree manifest

Identifies the files covered by the final R3 gates (Node 26.9.0, 2026-09-29) recorded in [mvp-progress.md](mvp-progress.md#r3--event-driven-supervision-2026-09-28).

## Base

- Branch `codex/mvp-implementation`, HEAD `8106b19840e8deced3d6f49df35fad0245faa460`, tree `26174f0ec57c8e6bf0cca1df315ccb9bbe12a632`.
- Every tracked file not listed below is unchanged from HEAD. That includes `package-lock.json`, configuration files and `docs/testing/fixtures/`, which the guarded harnesses read.
- The listed files are the uncommitted modifications plus three untracked files: `scripts/live-connection.ts`, `tests/live-viewer.test.ts` and `tests/supervision-hosts.test.ts`.

## Excluded

These are not inputs to `npm test`, `npm run build`, `npm run test:e2e` or the guarded harnesses:
- The modified documentation: `HANDOFF.md`, `docs/mvp-plan.md`, `docs/status-2026-09-28.md`, `docs/testing/mvp-progress.md` and `docs/adr/0007-live-agent-wall-over-push-channel.md`.
- The untracked `Claude outputs/` screenshot.
- Git-ignored output and runtime state: `node_modules/`, `dist/`, `test-results/` and `.crewroom/`.
- Temporary runtime configurations under `/tmp`. They held credentials and fixture tokens, so they are not recorded here.

## Evidence that this is the tested state

The hashes were computed on 2026-09-29, after the gates ran, because none were captured at gate time. They match the tested state for these reasons:
- The two hashes recorded before the gates, `package.json` and `tests/live.test.ts`, still match.
- The latest modification time of any listed file is 2026-09-28 23:55:46 (`tests/repository-work.test.ts`).
- The final gates all ran after that time:

  | Gate | Evidence |
  | --- | --- |
  | `npm run build` | `dist/` written at 00:08:04 |
  | `npm run test:e2e` | `test-results/.last-run.json` written at 00:08:22 with `"status": "passed"` |
  | `npm test` | `/tmp/cr-r3-git-node26/npm-test-node26.log` at 00:09 shows 106 passed and 0 failed |
  | Guarded sweep | 17 logs in `/tmp/cr-r3-git-node26/`, all `"passed": true` with `unexpectedSurvivingProcesses: []` |

  The guarded sweep ran from 00:09 to 00:16 using `run.sh` in the same directory.

Limits:
- Modification times can be preserved by tools. They are supporting evidence, not proof.
- No Node 26 build or e2e console log was retained, and `.last-run.json` does not record the test count or the Node version. The 8/8 figure comes from the implementing session's report. The earlier Node 22 log (`/tmp/cr-r3-e2e-final.log`, 23:11) shows 8 passed, but it predates later source changes.
- The independent review's exact revision was not recorded. Its recheck covered the repairs, so it predates or equals this tree. That ordering is an inference.

## Verify

From the repository root:

```sh
git rev-parse HEAD   # expect 8106b19840e8deced3d6f49df35fad0245faa460
grep -E '^[0-9a-f]{64}  ' docs/testing/r3-tested-tree.md | shasum -a 256 -c
```

Any other difference from HEAD outside the listed and excluded paths also makes the recorded results stale. To check for one, run `git status --porcelain -uall`.

```text
c9c3a7caafcf1d211fea2289d1322872eac969b6d297efc5658b570c19678950  package.json
f172d92afd4fe948ff4ad12350f79e3759423a6b646264a367412673705b58cd  scripts/coding-driver.ts
e1e9548db7a50cbf400f486cb56e53f9621a07c7eea265568a835045f79a8d68  scripts/delivery-driver.ts
85fdc25d9f23bd69ff8890a03f673d044c792612bf5b06d5d6a909178fb3d50b  scripts/interruption-driver.ts
45414160cb8df02a44386922fbe14a25ee8c1389d540518e1e221408cec6495e  scripts/jira-driver.ts
fad580b0d43f0a79d6cb8453c0d407c8b3145e37ff70a7e567ee94b34ef8a0f6  scripts/live-connection.ts
51a4cb81465640c1687a210a2d6577c8ec1a88abb1eef69609ea9a1b7aa67c6c  scripts/live-model-driver.ts
220e0cbd74bab2eac3362623779aeefc00a6e19906693153ed4923f0c9604682  scripts/omp_offline_fixture.py
86a665f437196844638fa8fb1633b95484595e1feaa4342859f7e6ddc3e7b1e9  scripts/results-driver.ts
10a86d70fa3d10f34127f49a2927423249473a0b10d1e7a7ed75d82d7dffaeca  scripts/runtime-driver.ts
8748f98f853be73d0ac3786af5c68e07327ecb1632021ca5ade25859f45aa1ff  scripts/supervision-driver.ts
111dc5db36ac80e273475898177b95c7d12051aca9f92c6ff3da87e730e8c673  server/app.ts
df0a1a5c501c182ea9e3b88593d733db7f20b75aba39608aa1464c6cb3c2b432  server/bitbucket-client.ts
a98806353018f6c3666ca4a4a400a5f10b2fbd6f79e81b6e56a0b9dada7ad12b  server/checks.ts
231a67d097c6266ea3e8b92932ecad39addcb4e2656c7463cacd35dcb95a0626  server/coding.ts
0591c506bce7998473a5ee3163089159dfe60013d26180c1b8a9e7d02f9c0d7b  server/evidence.ts
589b58ed46686e770a2b2fddb0fdff6dd892649ecf9bd1c22c340ae5470aa91b  server/jira-client.ts
b4d6fd336d3aad9c73e8c3b6eafe53ba9b2ed110d8545104fefc00dba1f1429a  server/jira-delivery.ts
88be79de57ffe9c9a506a261b16b31a5e386f7d5ae969746a642d01ec73fd984  server/live.ts
819b45f72311df645c1a768aa54daa04074d0a0582c446bd74d805519c407ee6  server/publication-git.ts
a6c9edf2a66c2db7356d2a95e868de8c7ad2ecf43f3ff4e070e1aeeded511e7c  server/publication.ts
3ee3e9a03fab80473fae54c668190ce1e942c13357fa7cc0a67094baab1fe825  server/repository-work.ts
87b7a9ba5b4e8e97a0e210f1121ecfbb021239256ada37878c2942e02e094953  server/results.ts
b836b8017cf2241013824ffcfca1dff0fc62379a4c1a3c064f6ffb30573c1dce  server/runtime/host.ts
50e32670666759113a526b9fffd0232994f61179d8777f242f09b77d60b16945  server/runtime/processes.ts
b9f84a2c5e93b43d22000e60f028ff2d5b61a395e724b354718d1b5c5f5a42ba  server/runtime/supervisor.ts
90dc41ee3eaa40c73d8840c67e22d585d3633c4d66d2968e9eaf772e1c872bf4  server/terminals.ts
4ee1665b66319fe3b9d653a48357cd6fd2e7462c75e540eb88602d35685bfc0d  src/results.tsx
21688363dd5aabda2c3cb3555a7038e797c0dd50e8425e6656e22202aabfbb89  tests/check-environment.test.ts
3197c95995e00dbff3cfcc669f59834e8f1660fb8b6d3f065e04798576a22a80  tests/checks.integration.ts
1da00052870a55c5da278ff27bb979bb624e6099d76b15895f55ab743953d3a1  tests/coding.test.ts
4db600ca67330ca09df201840eb1d2a8acde32a1cd520b34e629e7acf5fa6be9  tests/delivery-notices.test.ts
df35af2dd459f7e9debff1762a0dd231233fbf00ad9954d230e34dfea68f1e8f  tests/e2e/check-environment.spec.ts
5234ef396ba7cbcc3bcbe23a9c5d94113556cc55c4d90a551b708a52e9ee3c67  tests/e2e/publication.spec.ts
fbe72b2a30d3704231709d03380275f5fa2349391112fa46744df17705078cea  tests/evidence.test.ts
31e62a447d27e512feb6e4755d82a9930e9d83c69497f444a2e1cc8057aee2af  tests/fixtures/publication.ts
05c1536a1ed600e655a264c0e91e0206db92746d58e5497ae90f9c252d884849  tests/jira-delivery.test.ts
b8293150b575ec12ee7518a1f80b73b54f8a51a72cd449643f71a8b6a747b7e3  tests/live-viewer.test.ts
fc071ed8b1e94827ea186e3274ad954fe80310f4c99a7e219bb7742e1887017d  tests/live.test.ts
4bca33c33f239327aea65aa8d466b9695126fdbc5ddb854c297cd91f89ec69ee  tests/processes.test.ts
4ad88d1ca8f49454c22c1097083b6656e14e300f4c737d5d991d9765d07cf63a  tests/publication.test.ts
5f394d8d189f9c58a0015293f0c2ed976566dea3d86da854e0803993c18e3095  tests/repository-work.test.ts
7da737535d92ea29f9f79596968ad7c48ecd6095ed37103bcdd0fc0ac41fdd0e  tests/results.test.ts
5527a399183afc86549f02e2b47764286f3158b502a79e8df32fae59bedb7c12  tests/supervision-hosts.test.ts
11eb67edae6d3a13c00ba29902cea25dabba6fe8a061df60d910205041981f6b  tests/supervision-load.test.ts
```

## Test-resource cleanup (2026-09-29)

One tmux server from an earlier run was still alive:
- PID 63115, started 2026-09-28 17:38:25.
- Command: `tmux -S …/T/crewroom-hosts-sEDwnb/private.sock -f /dev/null new-session -d -s crewroom … /bin/sleep 2`.
- Working directory: the repository.

It matches the dead-pane test in `tests/supervision-hosts.test.ts`, which creates a `crewroom-hosts-` directory and this session and command. The fixture had already removed the socket directory, so the server was orphaned. It had no clients and no child processes. Its only session was `crewroom`, with one dead pane (`/bin/sleep 2`, exit 0).

Cleanup steps:
1. Recreated the directory with mode 0700.
2. Sent SIGUSR1, which makes tmux recreate its socket.
3. Confirmed through the socket that it was server PID 63115 with the session above.
4. Stopped it with `tmux -S <socket> kill-server`.
5. Removed the directory.

No tmux processes remain.
