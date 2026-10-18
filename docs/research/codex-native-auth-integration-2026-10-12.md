# Native Codex authentication and recovery integration — FOR-7

Research date: 2026-10-12. Subsequent production implementation and acceptance are recorded in the [MH2 runtime contract](../testing/mh2-runtime-acceptance.md). Repository baseline: `8b9e9ea44a87ed9167a2bfc3eea47bb8fdfb6b2e`. This note extends the [MH2 compatibility spike](codex-mh2-compatibility-spike-2026-10-12.md); it does not establish production-adapter acceptance.

**Recommended implementation:** retain one observable app-server process and private persistent `CODEX_HOME` per assignment. Give those processes only an in-memory native access token through the experimental external-token protocol. A single server-side credential owner performs managed refresh. Do not give each child a managed `auth.json`, including through symlinks. This is an implementable, version-pinned experimental route; current documentation and the installed schema disagree about its support status, and real refresh acceptance remains outstanding.

## Evidence and reproducibility

The executable is `/home/crewroom/.local/share/mise/installs/codex/0.162.0/bin/codex`, reporting `codex-cli 0.162.0`; SHA-256 `50ed828f357c655a3c82054d346cab8434f24901f14ec19b8267571bdd008b38`. No matching installed Rust source tree was available. Claims below distinguish generated schemas, official documentation, isolated executable observations, and proposed policy.

The schemas were generated without inference:

```sh
codex app-server generate-json-schema --out /tmp/crewroom-codex-mh2-eq5kutam/stable
codex app-server generate-json-schema --out /tmp/crewroom-codex-mh2-eq5kutam/experimental --experimental
```

Relevant generated files under `experimental/` are `v2/LoginAccountParams.json`, `v2/GetAccountParams.json`, `v2/GetAccountResponse.json`, `ChatgptAuthTokensRefreshParams.json`, `ChatgptAuthTokensRefreshResponse.json`, `v2/ThreadInjectItemsParams.json`, `v2/TurnStartParams.json`, and `ClientRequest.json`. Regenerate against the pinned executable rather than relying on temporary paths surviving.

Two disposable fixture revisions ran the real executable with invented credentials, a local HTTP stub, and **zero inference turns**. A private filesystem/PID/network namespace exposed only system binaries, the pinned executable, the fixture/helper scripts and private output. The network namespace had loopback only. No real home, credential, repository or configuration was mounted; every child was stopped and reaped. `CODEX_REFRESH_TOKEN_URL_OVERRIDE` redirected the fake managed-refresh flow to the local stub. It is a test-only binary capability, not a production configuration recommendation.

| Observation | Result and boundary |
| --- | --- |
| Managed file, `account/read` with `refreshToken:true` | Issued the fake refresh request and persisted replacement tokens, despite the original fake JWT's future expiry. |
| Managed `auth.json` symlink | Preserved the symlink and changed its target. A child symlink can mutate the shared credential source. |
| Two homes with symlinks to one managed file | Both processes sent the **same original refresh token** concurrently, before either response returned. This layout has no adequate cross-process refresh exclusion. |
| External-token login with ephemeral storage | `account/login/start` returned success and created no `auth.json`. The child received no refresh token. |
| Synthetic 401 during external account lookup | Emitted `account/chatgptAuthTokens/refresh` with `reason:"unauthorized"` and the previous account ID. Replying on that server request ID caused a retry using the replacement fake access token. |

The account stub omitted required backend-routing information, so the final account reads returned `workspace routing discovery missing backend origin`. These observations prove refresh dispatch/persistence and external callback/token replacement, **not** a fully successful account lookup or real OAuth refresh. A preliminary rate-limit-only 401 did not produce the callback; do not assume every authentication-related endpoint has identical recovery behavior.

Local raw evidence is under `/tmp/crewroom-codex-mh2-eq5kutam/`:

| Artifact | SHA-256 |
| --- | --- |
| `auth-fixture-v2.py` | `eadfe7bfeff1e925944665011f003b5dbc1a99750e318149e265ac89a516fe48` |
| `auth-fixture-output-v2/auth-fixture-result.json` | `2e1ba67d19e80fb5fd9ca2f7ff9fab5221655560407a95f2aa72b499c07b2856` |
| `auth-fixture-v4.py` | `1b283f6330ea9a5e72063ebbdea55ce39f661965ecaa22a5b706ee531aeaca5e` |
| `auth-fixture-output-v4/auth-fixture-result.json` | `5ed45916db9929bd3eae40d53150f74b7c6c5342bada1e33857e82f54c5301a7` |

The shared JSONL process helper was `scripts/probe_codex_runtime.py` at the baseline revision. Fixture scripts and raw evidence are temporary research artifacts, not checked-in regression tests.

The isolation command can be reproduced for either fixture revision with a fresh output directory:

```sh
probe_out=$(mktemp -d /tmp/crewroom-auth-fixture-XXXXXX)
bwrap --unshare-all --die-with-parent --new-session \
  --ro-bind /usr /usr --symlink usr/bin /bin \
  --symlink usr/lib /lib --symlink usr/lib /lib64 \
  --proc /proc --dev /dev --tmpfs /tmp --bind "$probe_out" /work \
  --ro-bind /tmp/crewroom-codex-mh2-eq5kutam/auth-fixture-v4.py /auth-fixture.py \
  --ro-bind /home/crewroom/Projects/crewroom/scripts/probe_codex_runtime.py /probe_codex_runtime.py \
  --ro-bind /home/crewroom/.local/share/mise/installs/codex/0.162.0/bin/codex /tools/codex \
  --clearenv --setenv PATH /usr/bin --setenv HOME /work --chdir /work \
  -- /usr/bin/python3 /auth-fixture.py
```

## Authentication contract and support discrepancy

Official authentication documentation distinguishes file, keyring, automatic selection and ephemeral credential storage. With file storage, `auth.json` belongs to `CODEX_HOME`; ephemeral storage is memory-only. Configuring separate homes therefore separates session/configuration storage, but copying managed tokens does not create independent refresh ownership. [Authentication](https://learn.chatgpt.com/docs/auth).

Official operational guidance says to preserve Codex's refreshed credentials, avoid concurrently sharing the same managed auth file, and avoid implementing a separate direct OAuth refresh flow. It identifies concurrent rotation and restoring stale credentials as refresh-failure causes. This agrees with the symlink/concurrency observations above. A read-only auth mount cannot make remote refresh harmless or persist rotation; it was suitable only for the earlier bounded live probe. [Managed account auth in CI/CD](https://learn.chatgpt.com/docs/auth/ci-cd-auth).

The public app-server documentation describes experimental host-owned `chatgptAuthTokens`, managed forced refresh through `account/read`, and a roughly ten-second external refresh callback deadline. Conversely, the pinned `LoginAccountParams.json` labels that same variant: **“[UNSTABLE] FOR OPENAI INTERNAL USE ONLY - DO NOT USE.”** The successful fixture establishes availability, not a stable support promise. Gate this adapter on the pinned executable/schema/profile and explicitly retain the experimental compatibility risk. [Official app-server authentication contract](https://learn.chatgpt.com/docs/app-server#auth-endpoints).

The installed schema gives this exact child sequence:

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"crewroom","version":"<version>"},"capabilities":{"experimentalApi":true}}}
{"method":"initialized"}
{"id":2,"method":"account/login/start","params":{"type":"chatgptAuthTokens","accessToken":"<in-memory access token>","chatgptAccountId":"<bound account ID>"}}
```

Use `cli_auth_credentials_store="ephemeral"` in the child's isolated configuration. `chatgptPlanType` is optional. On the server request `account/chatgptAuthTokens/refresh`, answer its RPC ID with `{accessToken,chatgptAccountId,chatgptPlanType?}`. The schema permits `previousAccountId:null`; a broker must resolve it from its registered child binding, never treat null as authority to choose another account. `account/read {refreshToken:true}` is ignored in external mode. These are wire-schema facts, not permission to persist token-bearing requests in ordinary protocol logs.

## Proposed credential ownership policy

1. **One owner per credential source.** Crewroom runs one auth-only managed Codex app-server for that source and serializes its refresh requests. Assignment app-servers remain concurrent. The owner uses the existing native login and lets Codex perform and persist rotation. It never runs assignment inference or shares assignment histories.
2. **File-backed integration first.** For the observed file store, a narrowly scoped trusted server component reads the current access token and account identity after managed refresh. The owner can use a separate clean `CODEX_HOME` with its sole managed `auth.json` link targeting the canonical native credential file; the fixture proves this writes back to that target. Do not use the native home for assignment histories. The app-server account response is metadata, not an access-token export API. Do not claim keyring support until secure access and refresh ownership are implemented and tested; copying keyring credentials into files is not an automatic fallback.
3. **Private child state.** Each assignment gets a stable private session directory, explicit clean configuration and restricted catalog/profile. Every replacement process receives a new in-memory login. Never mount the source auth file or pass a refresh token to an assignment child. Supply access tokens through private stdin JSON-RPC, not command arguments, environment variables, browser state or durable event records. Redact both login requests and callback replies before any protocol recording.
4. **Single-flight refresh.** Record an opaque credential generation and frozen account binding per child. If a callback refers to an older generation, return the current access token; otherwise coalesce callbacks into one owner `account/read {refreshToken:true}` operation. Read the persisted result before publishing the new generation. Bound callback handling; refresh proactively when needed so one slow operation does not become an unbounded callback queue. An auth failure pauses admission and produces a visible unavailable state, never an alternate provider/account fallback.
5. **Account changes require new admission.** Validate source account and child account before supplying tokens or resuming a thread. Reject mismatches, revoked credentials and unexpected storage changes. Restrict filesystem permissions and local IPC; the access token still grants bearer authority until expiry. Omitting refresh tokens limits renewal authority but does not mint a separately scoped assignment credential.

This policy is a design recommendation derived from the schema and fixture results. It still needs broker implementation and acceptance tests. In particular, a Crewroom lock does **not** coordinate an unrelated interactive Codex process refreshing the same login. Existing native-login reuse must document that boundary, detect source-generation changes and fail closed on conflicting/revoked rotation. Universal safe concurrency with arbitrary external credential writers has not been established. A dedicated credential owner or a future supported credential-broker contract would remove that ambiguity; sharing child auth files does not.

## Recovery after the original dynamic-tool RPC disappears

The prior spike's real executable/synthetic-provider crash matrix is decisive: a crash before effect, after effect, or after durable receipt but before replying can all leave Codex history with tool output `aborted`. A fresh process did not reissue the old server request. Completed replies were retained once they had reached the next provider request. These are observed facts, not exactly-once guarantees. [MH2 crash evidence](codex-mh2-compatibility-spike-2026-10-12.md#recovery-result-that-must-shape-implementation).

Installed recovery surfaces:

| Surface | What it can do | Limit |
| --- | --- | --- |
| `thread/resume` and history reads | Reopen/inspect the persistent assignment thread. | Do not make `aborted` authoritative about an external effect. |
| `thread/inject_items` | Append raw model-visible Responses items without starting generation. | No schema promise to replace an existing call output; append a recovery notice, not an invented reply on a dead RPC. |
| `turn/start.toolOutput` | Supply a standalone named tool output, optionally namespaced, with `input:[]`. | `TurnToolOutput` has **no callId** and starts/queues generation; it is not a reply to the lost invocation. |
| `turn/steer` | Add input to the expected active turn. | Not recovery of a previous process's lost RPC. |

The first two append/start behaviors are also described in the [official app-server lifecycle documentation](https://learn.chatgpt.com/docs/app-server). Recovery-item injection has not been executed in this research; its exact accepted content and restart durability require a disposable test before production use.

Recommended sequence: stop the prior owner, hold inference/tool admission, bind the same assignment/thread identity, inspect history, and reconcile Crewroom's durable logical-operation record or actual external effect. Uncertain effects remain held. For a known result absent from model history, append an explicit recovery notice containing the original thread/turn/call/operation IDs, outcome and receipt reference, then admit continuation. Preserve the original aborted history. A later new call ID cannot authorize an equivalent unresolved mutation.

Persist recovery-notice intent and a unique marker before delivery. If the process dies after delivery but before the host records its acknowledgement, inspect history for that marker before resending. Do not blindly append a second `function_call_output` for a call that already has an `aborted` output: valid replacement semantics have not been demonstrated. Host reconciliation and executor idempotency enforce safety; prompt text alone does not.

## Remaining acceptance gates

- Complete fake-backend account routing, then test successful broker refresh and persistence, concurrent child callbacks, callback timeout, wrong-account rejection, source changes, broker restart and absence of credentials in logs/state.
- Run a bounded native external-token roundtrip using the existing login, followed by a controlled managed refresh acceptance check. The earlier live managed-auth test does not prove external-token inference or real rotation.
- Verify recovered receipt delivery across injection acknowledgement crashes, known/unknown effects, and a model proposing the same logical operation under a new call ID. Verify two concurrently living assignment processes retain separate histories.
- Pin the experimental API and retest on upgrade. Preserve the native-route limits from the spike: post-fact usage snapshots do not provide gateway-enforced spend/token limits, model/request filtering or universal reroute prevention.

No real credential was read, copied, modified or refreshed for these additional research probes; no inference was requested. No production code, configuration or installed executable was changed by this research task.
