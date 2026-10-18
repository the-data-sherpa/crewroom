# FOR-9 model identity contract

Schema version 1 is additive. A role remains a reusable responsibility with a job and optional model/effort overrides. A selection is either an exact `offering:<harness>:<encoded-provider>:<encoded-selector>` key or an explicitly configured `equivalent:<id>` key. Percent encoding keeps identity components unambiguous. Encoded keys must fit the 1,000-byte public selection limit. Display names never select or group models.

An offering records its harness, provider, exact selector, label, advertised capabilities, configured access, accepted execution profile, discovery observation and inference observation as separate facts. Unknown capability arrays/booleans are null. A metadata response cannot establish account entitlement or successful inference. A requested Claude `default` alias stays `default`; its observed model has a source and observation time and is never substituted into the requested identity.

Equivalence mappings contain only offering keys. Each member belongs to at most one group. Preferences must name every member exactly once. Nested groups, cycles, missing references, duplicate keys and preference ties are rejected. No mappings are installed by default.

The frozen route retains the existing `requested`/`resolved` adapter shape and adds `selection.schemaVersion = 1`, intended selection, model/effort inheritance source, offering facts, catalog revision and policy revision. Adapter executable/profile, account binding, tools and observed session identity continue to use the existing runtime records. Refresh, settings changes and Resume never resolve an admitted route again.

| Input or event | Required behavior |
| --- | --- |
| Equal display labels | Separate offerings, even within one model family |
| Explicit equivalent offerings | First eligible member in the complete configured preference order |
| Unknown effort/input/tool capability | Block when required; never coerce or guess |
| Configured access, no inference observation | May admit through a verified profile; never display inference as proven |
| Discovery older than one hour, failed refresh or changed configuration/binary | Retain display/history; block new catalog admission until fresh evidence |
| Untagged historical `provider/model` | Original OMP route, even if provider is named `codex` |
| Historical explicit native adapter | Original native route; no migration by model-name inference |
| Catalog selection sent to legacy resolver | Reject rather than use configured legacy fallbacks |
| Provider failure on a schema-1 frozen selection | No runtime model or harness fallback |

The one-hour interval applies to new catalog admissions only. Historical assignments and their fallback receipts remain readable and unchanged. Existing unsaved legacy defaults continue using the original resolver until the operator deliberately selects catalog offerings. Executed evidence belongs in [MVP progress](mvp-progress.md); this contract alone does not establish live acceptance.

## Private configuration and public intent

Keep existing runtime configuration and native login ownership. `ompBinary`, `codex.binary` and `claudeCode.binary` are absolute configured paths. Discovery accepts OMP 18.4.12, Codex 0.162.0 and Claude Code 2.1.296; a version mismatch retains the previous snapshot as stale. Existing provider definitions, `codex.authFile` and `claudeCode.home` stay server-side. The packaged discovery implementation is Rust and does not invoke the Python acceptance fixtures or a Node SDK bridge.

The private `catalog` remains an explicit allowlist of verified execution profiles. Discovery can list offerings absent from that allowlist; those rows explain that no verified profile supports them. Match `adapter`, `provider` and `id` exactly. Untagged configured profiles remain OMP. Claude profiles additionally pin `resolvedModel`; alias drift requires an explicit configuration review and new admission. Metadata and configured capabilities intersect. Verified profile facts may supply tools/inputs that metadata omits, but unknown facts never become support automatically.

An illustrative native profile is:

```json
{
  "adapter": "codex",
  "provider": "codex",
  "id": "MODEL_FROM_CURRENT_DISCOVERY",
  "api": "codex-app-server",
  "inputs": ["text"],
  "efforts": ["low"],
  "tools": true
}
```

The placeholder is not an executable model claim. Configure only characterized model/profile combinations. The public job file uses keys, not credentials:

```json
{
  "revision": 1,
  "budget": "small",
  "jobs": {
    "code": "offering:codex:codex:MODEL_FROM_CURRENT_DISCOVERY",
    "judgment": "",
    "review": "",
    "fast": ""
  }
}
```

Empty jobs require a selection before a dependent role can launch. A role's optional `model` accepts the same offering or equivalence key. Its optional `effort` overrides the budget-derived effort; `default` inherits. A reusable role still describes responsibility, instructions and obligations independently of model choice.

Optional `policy.equivalence` is an array of `{key, displayName, members, preference}`. For example, two explicitly established equivalent routes can use key `equivalent:review-model`, members `["offering:codex:codex:EXACT_MODEL", "offering:omp:PROVIDER:EXACT_MODEL"]`, and those same members in the preferred order. Both must exist. This shape does not declare those placeholders equivalent. No grouping follows from names alone.

## Refresh, admission and recovery

The private runtime directory contains `model-catalog/catalog.json` and owned discovery process receipts. Snapshots use schema 1, a monotonic catalog revision and refresh generation, plus per-harness outcome, last attempt/success, sanitized error, identity and retained offerings. Files use private permissions and atomic replacement. Catalog writes reject snapshots larger than 16 MiB before replacing the previous readable file, including interrupted-refresh recovery. A failed recovery write exposes a persistence error and blocks admission while allowing ownership cleanup and another refresh. Preserve them with the runtime backup.

Refresh sends metadata requests only. It isolates ambient configuration, hooks, plugins and MCP servers; it authorizes no inference turn or tools. Each harness has a 30-second exchange deadline, an 8 MiB output limit, 10,000-row limit and 100-page limit. Repeated cursors, malformed data and duplicate identities fail that harness. Refresh retries coalesce into the running generation. Successful harnesses persist independently; failed harnesses retain their old offerings without refreshing their eligibility. Service restart marks unfinished refresh as interrupted. Persistence failure blocks new catalog admissions.

Freshness lasts one hour. Failed refresh, backwards clock observations, configuration/account changes and executable changes invalidate new admissions. Preview uses an executable metadata stamp for responsive reads; admission and frozen-route recovery verify SHA-256. Access means configured, not paid entitlement or successful inference. Old catalog rows and missing saved selections stay visible so drafts remain recoverable.

The same resolver supplies picker eligibility, role previews and admission. Every catalog preview carries a digest over intended selection, role, policy, requirements, catalog revision and the resolved route. Launch and insertion carry that digest as `modelPreview`. A changed preview, including a change back to a legacy selector, fails admission. A pending team proposal with an invalid preview is durably rejected with a reason and its transition marker cleared, allowing a fresh proposal. Applied proposals replay their frozen admissions without checking current catalog freshness. Refresh the preview before trying again. Saving settings or requesting refresh never queues or starts work.

Before publishing readiness after restart, the service recovers owned discovery processes. Uncertain ownership blocks startup instead of declaring cleanup complete. `scripts/verify_catalog_restart.py` exercises the production daemon's interrupted-refresh cleanup with synthetic metadata and no inference.

Admission captures `selection.schemaVersion`, intent/source, requested effort/source, effective and advertised capabilities, offering/observation facts, executable hash/profile, configuration revision, catalog revision, policy revision and role revision. Existing host records retain account, tool catalog and session identities. Recovery uses the captured route. Current catalog freshness and edited job settings do not substitute a new route into an old assignment. Executable/account/profile changes require re-admission; alias/session changes retain the existing adapter's conservative recovery holds. Catalog failures cannot release process ownership or resolve outstanding effects/messages.

## Service and native UI

All operations use the existing authenticated loopback boundary. Mutations also require the daemon's own origin.

| Operation | Result |
| --- | --- |
| `GET /api/model-catalog` | Credential-free snapshot, per-harness freshness, offerings, explicit groups and eligibility by effort |
| `POST /api/model-catalog/refresh` with `{}` | Background refresh reservation/current generation; completion arrives through observations |
| `POST /api/model-preview` with `{role: FULL_ROLE}` | Shared resolver result or an actionable eligibility error; no assignment |
| Workspace / role preview | Effective role model, preview revisions and current job settings revision |
| Existing job/role save | Revision-checked public intent only |
| Attempt history | Captured model and separate observed runner facts; no current-policy resolution |

Models & budget and role overrides use searchable, disambiguated choices. Disabled choices explain blockers and cannot replace the selection by keyboard confirmation. Missing saved keys remain visible. Refresh preserves drafts. Narrow menus wrap labels and explanations within measured rows; the list retains equal row heights to match its virtualization contract. History preserves legacy OMP/native routes and distinguishes requested Claude aliases from observed identities when recorded.

## Validation boundaries

Focused tests cover discovery/process bounds, durable refresh, policy, stale previews, account/profile/capability drift, service authentication/concurrency, retained history and native interaction. The rendered Linux/Xwayland walkthrough is separate from GPUI tests and from live inference. Seeded records are migration/display evidence only.

Opt-in live acceptance uses `scripts/verify_catalog_workflow.py` with a private configuration file, a new disposable output root and the existing Rust Mail binary. It invokes the production daemon and the ignored `native_catalog_external_live_selection` GPUI check. The fixture then exercises addressed tools, immutable selections, controller restart, stopped-process recovery and completion. Actual-binary simulated-provider OMP checks remain a separate evidence class. Gate 9 requires successful real model execution through all three adapters; metadata, a picker test or an ignored case cannot substitute for that result.

Existing native accounting and platform limits remain: reasoning budget is not a spending cap, native provider request counts/billing are unavailable, and Claude cost is a list-price estimate. No installer, active role file, global harness or unrelated Jira/PR is changed by this implementation.
