# OMP model inputs for offline simulation

Inspected: 2026-09-26. Local executable: `omp/18.3.1`. No model was prompted, provider authenticated, catalog refreshed, or user configuration changed for this inventory.

## Inventory and evidence boundaries

The user configuration explicitly declares **50 selectors across four providers**. These make concrete inputs for routing and assignment simulations. They are not a benchmark, recommended production assignments, or a guarantee that each endpoint will accept requests. The source file was last modified on 2026-09-23 at 19:14:55 UTC. The [sanitized fixture](../testing/fixtures/omp-model-catalog-2026-09-26.json) captures provider, exact ID, API family, and declared limits/input/thinking metadata from the [local configuration](/Users/crewroom/.omp/agent/models.yml); it excludes credentials, endpoints, headers, and cost declarations.

| Configured provider | Models | Configured API family |
| --- | ---: | --- |
| `aperture-anthropic` | 6 | `anthropic-messages` |
| `aperture-openai` | 25 | `openai-responses` |
| `aperture-google` | 9 | `google-generative-ai` |
| `aperture-bedrock-compat` | 10 | `openai-responses` |

The final provider's name does not imply an AWS Bedrock transport: its configuration selects `openai-responses`. Provider and model must both be preserved. Several model IDs themselves contain `/`; parse a selector at the first slash only. The fixture contains both split fields and the full selector to test this explicitly.

Only `aperture-openai/openai.gpt-6-sol` has prior successful Crewroom message-experiment evidence. The [independent-process Relaycast report](../archive/omp-external-roundtrip-experiment.md) establishes that narrow result. Other entries are configured but unexercised by Crewroom. Offline simulation can verify selection, validation, process launch arguments, reported-model matching, and fallback policy; it cannot verify actual authentication, tool calling, output quality, latency, or price for those entries.

## Why the listing was not run

The installed `omp models --help` documents `ls`, `find`, and `refresh`, but no offline flag. Version-matched [models command source](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/cli/models-cli.ts#L348-L373) uses `online-if-uncached` for ordinary listing, so avoiding the explicit `refresh` action does not ensure no provider discovery. Its [runtime-provider path](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/cli/models-cli.ts#L330-L342) has the same policy. We therefore used an offline allowlist extraction of explicit YAML entries. Local cache tables were inspected read-only, but broad cached provider catalogs were excluded: cache presence alone does not establish a configured, usable model.

The CLI's [JSON projection](https://github.com/can1357/oh-my-pi/blob/6204b7508014bcdf12d95f0b3fd470905fd99344/packages/coding-agent/src/cli/models-cli.ts#L105-L118) demonstrates the provider/ID selector shape. This fixture preserves raw user-declared metadata rather than claiming to reproduce OMP's complete composed or filtered registry. Capability checks must retain that distinction. For example, `declaredToolSupport` is unknown for every entry because this local configuration does not declare it.

## Illustrative assignment inputs

Use these to exercise different configuration paths, not to rank models by their names. Role independence is determined by distinct assignments and implementation history, not by choosing different model brands.

| Simulated assignment | Exact selector | Coverage purpose |
| --- | --- | --- |
| Coordinator | `aperture-openai/openai.gpt-6-sol` | Previously exercised selector; responses API |
| Implementer | `aperture-anthropic/anthropic.claude-sonnet-5` | Different provider/API and adaptive-thinking metadata |
| Independent reviewer | `aperture-google/gemini-3.8-flash` | Third provider/API and Google thinking metadata |
| Delivery reporter | `aperture-bedrock-compat/us.anthropic.claude-haiku-4-5-20251001-v1:0` | Different provider on the responses API; reasoning disabled in configuration |
| Technical writer | `aperture-openai/openai.gpt-6-luna` | Different model under the same provider/API |

The remaining specialist roles should participate in the same assignment tests. Do not silently adopt this illustrative table as user role defaults. Rotate each applicable role through the fixture's provider/API families and preserve the exact selected role/model/configuration revision in the assignment snapshot.

## Cases enabled by this fixture

- Exact-selector matching, aliases remaining distinct, slash-bearing model IDs, missing/removed models, duplicate selectors, and stale inventories.
- Distinct model choices for each specialist, unchanged in-flight assignments after role edits, and explicit replacement/fallback choices recorded in history.
- API-family dispatch independent of provider name; a provider whose name contains `bedrock` must use its declared responses transport in this snapshot.
- Unsupported effort rejection: `aperture-openai/zai-org/GLM-5.2` declares only `high` and `max`; `aperture-google/gemini-3.1-pro-preview` declares only `low` and `high`.
- Text-only routing: `aperture-openai/qwen.qwen3-coder-next` declares text input and no reasoning; an image-dependent assignment must not accept it based on model-name inference.
- Context/output budget boundaries using declared metadata, with absent or suspect values treated as unknown rather than trusted runtime capacity.
- Configured versus actually exercised labels; missing price/tool-support evidence remains unknown and cannot establish a quality gate or monetary ceiling.

Validate the sanitized fixture without loading user configuration in tests. Deliberately changed/removed entries belong in derived test fixtures, not the captured inventory. An eventual live model contract check is a separate, explicitly identified layer; simulation results must never be presented as those models having completed real work.
