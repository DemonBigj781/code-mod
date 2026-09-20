# Release Runtime Regression Repair Plan

**Date:** Sunday, August 23, 2026

## Goal

Ship the next `code-mod` release with corrected client version reporting,
provider authentication and model discovery, synchronized TUI model choices,
OpenRouter free-model fallback, a compatible GitHub Actions runtime, and the
fixed GitHub run waiter.

## Confirmed Requirements

- Release binaries MUST report an internal Code/Codex client version that is
  not older than the pinned compatible upstream version.
- `gh_run_wait` MUST request only JSON fields supported by the installed GitHub
  CLI. It MUST use `url` and MUST NOT request the removed `htmlURL` field.
- The model and Accounts TUIs MUST expose Stable Horde alongside OpenAI and
  OpenRouter.
- OpenAI, OpenRouter, and Stable Horde MUST be peer providers selected
  automatically from the chosen model. OpenRouter MUST NOT be implemented as
  a configuration profile, and the global provider MUST NOT be hand-pinned.
- GitHub Copilot and Hugging Face are future peer providers; they are not part
  of this repair.
- OpenRouter use MUST require an OpenRouter API key.
- Stable Horde authentication MUST resolve in this order:
  1. Encrypted stored key.
  2. `AI_HORDE_API_KEY`.
  3. Anonymous key `0000000000`.
- Standard OpenAI account authentication MUST remain primary. A configured
  OpenAI API key MUST be available as a fallback when the account cannot use a
  selected model, is rate-limited, or has exhausted its model/API-call quota.
- `openrouter/free-max` MUST advance to the next compatible free candidate when
  a candidate returns HTTP 402 before producing model output.
- Selecting any model MUST derive its provider automatically, including after
  restart with stale persisted provider state. No provider-specific profile
  marker may be required.
- Agent, review, planning, validation, Auto Drive, and routing model selectors
  MUST derive their choices from the same current model inventory as the main
  model selector. They MUST NOT retain independent stale model lists.
- Provider-qualified display identities MUST use
  `{service-provider}/{catalog-owner}/{model}(:free)` while keeping the
  provider-native API model ID separate. Stable Horde anonymous entries use
  the `:free` suffix; authenticated Stable Horde entries do not.
- OpenRouter and Stable Horde image generation MUST be capability-driven and
  available when the selected provider model advertises image output.
- OpenRouter and Stable Horde reasoning controls MUST be enabled and sent only
  when the selected model advertises reasoning support.
- Stable Horde MUST use its v1 proxy API first and silently fall back to its v2
  direct API when v1 is unavailable or cannot serve the request. A v1 failure
  MUST write a non-user-facing fallback notice to the logs. If both transports
  fail, the UI MUST show one combined error while the logs retain both causes.
- The Auto Drive settings surface MUST visibly expose the sub-agent enablement
  control.
- The TUI MUST expose a dedicated Sub-agents settings section instead of
  requiring users to discover command configuration inside the Agents page.
- One parent agent MUST be able to fan the same command and prompt out to
  multiple enabled sub-agents concurrently and receive each result separately.
- Child sub-agent rollouts MUST remain excluded from `/resume` discovery while
  remaining eligible as inputs to the memories pipeline.
- Long conversations MUST preserve message and reasoning-note boundaries, and
  newly streamed assistant responses MUST remain fully visible rather than
  rendering only their first character.
- The release workflow MUST use the repository's established Node 24-compatible
  major versions for checkout and artifact actions.
- Every push to `main` MUST build only the cached Linux x86_64 `dev-fast` beta
  artifact. Stable preflight, platform builds, and publication MUST require an
  explicit manual stable dispatch for the exact commit after its beta succeeds.
- GitHub-hosted compilation MAY use the runner's available parallelism. The
  one-thread limit applies only to local and self-hosted compilation.

## Runtime Evidence

- Release `v0.6.97` reports `code 0.6.97`, while
  `code-rs/code-version/default_version.rs` pins `0.144.6`.
- `gh 2.96.0` rejects `htmlURL` and lists `url` as the supported run URL field.
- The live Stable Horde OpenAI-compatible service exposes `/v1/models`,
  `/v1/chat/completions`, and `/v1/responses` at
  `https://oai.aihorde.net/v1` and documents `0000000000` as its anonymous key.
- The OpenRouter free router currently excludes HTTP 402 from candidate
  fallback in `should_try_next_openrouter_model`.
- The release workflow still uses `actions/checkout@v4`,
  `actions/upload-artifact@v4`, and `actions/download-artifact@v4`, while other
  repository workflows already use the Node 24-compatible majors.
- The August 23, 2026 k-sampling test chat shows active-turn input being handled
  after an already-running response, producing response/input misalignment.
- The live August 23, 2026 configuration reproduced a stranded provider state:
  `model = "gpt-5.6-sol"` with `model_provider = "openrouter"`. New sessions
  inherited OpenRouter even though an OpenAI model was selected.
- The affected long rollout persisted complete assistant messages, proving the
  first-character cutoff is a TUI rendering failure rather than lost model
  output. The critical log repeatedly reports reasoning cells with cached
  multi-line heights but recomputed zero- or one-line heights at history indexes
  above 1,900, causing stale row offsets to survive across redraws.

## Implementation Map

- Internal version: update `code-rs/code-version` and release version
  selection. Verify with code-version unit tests and the hosted
  `code --version` smoke test.
- Run waiter: update
  `code-rs/core/src/tools/handlers/gh_run_wait.rs`. Verify the exact JSON
  field list in a unit test and exercise the live waiter.
- Stable Horde provider: update provider definitions, authentication, secrets,
  remote-model discovery, and TUI provider surfaces. Verify explicit-key and
  anonymous requests plus model-catalog loading.
- OpenRouter 402: update `code-rs/core/src/client.rs` and its tests. Verify a
  pre-output HTTP 402 advances to the next free candidate.
- Provider switching: repair inconsistent persisted OpenRouter state when an
  OpenAI preset is selected, remove profile-based provider switching, and
  cover every provider transition independently in TUI tests.
- Shared model inventory: update TUI model-selection builders and agent
  settings pages. Verify selector parity for every target.
- Sub-agent settings and execution: restore a dedicated settings section,
  retain multi-agent command selection, verify identical prompt fan-out, and
  separate resumable session sources from memory-eligible session sources.
- Long-history rendering: reconcile corrected reasoning-cell heights into the
  cache, invalidate stale prefix sums, and verify full assistant text and cell
  separation after large transcripts change reasoning visibility or state.
- Account methods: update the Accounts TUI and auth manager. Verify method
  visibility and fallback selection.
- Node actions: update `.github/workflows/release.yml`. Verify with
  `actionlint` and the enabled-artifact count check.
- Release: use `.github/workflows/release.yml` for hosted preflight, enabled
  builds, smoke tests, publication, and checksums.

## Delegated Work

The active-turn input-ordering implementation is being repaired by another AI.
This workstream MUST NOT edit the queue/steering files while that change is in
progress. Before release, the integrated branch MUST include a regression test
based on the August 23, 2026 k-sampling conversation and MUST verify that the
first response after a new active-turn input recognizes that input.

## Follow-up Routing UI Contract

A later Models and Agents submenu will expose models discovered from routing
providers. Each model will have an enabled state and an explicit fallback
position. Runtime fallback will use that user-defined order, skip disabled
models, advance only on failures before model output begins, and never move
from a free route to a paid route implicitly. The provider inventory created
by this repair MUST remain reusable by that submenu rather than introducing a
second model catalog. Every discovered routing model will also receive a
default dedicated agent entry keyed by its provider-qualified model identity.
Disabling the model disables that default agent from automatic routing while
preserving user customization and explicit invocation.

## Upstream Parity Gate

After the recovery implementation and its focused tests are coherent, compare
the resulting branch against all three maintained source lines: the current
`code-mod` history, Just-Every Code, and OpenAI Codex. Reconcile applicable
upstream fixes deliberately, preserving the provider safety, sub-agent session
visibility, and release constraints in this plan. Parity work is the final
integration phase rather than an implementation prerequisite so unstable
recovery code is not repeatedly rebased or rewritten.

## Completion Gates

- All affected unit and integration tests pass in hosted preflight.
- The release binary reports the intended non-stale internal version.
- Stable Horde models load with both an explicit key and anonymous fallback.
- Stable Horde uses v1 first, logs a silent fallback notice on v1 failure,
  succeeds through v2 without a user-facing error, and combines both causes
  into one user-facing error only when v1 and v2 both fail.
- OpenRouter free-max skips a pre-output HTTP 402 candidate.
- Provider selection is automatic for OpenAI, OpenRouter, and Stable Horde,
  works across every transition, and does not create or require profiles.
- Reasoning settings are applied to OpenRouter and Stable Horde models only
  when the selected model advertises reasoning support.
- All model-selection surfaces contain the same applicable current models.
- Accounts displays OpenAI account login, OpenAI API-key fallback, OpenRouter,
  and Stable Horde authentication methods.
- Settings displays a dedicated Sub-agents section; a configured command can
  launch multiple selected agents with the same prompt concurrently.
- Child sub-agent rollouts do not appear in `/resume` but can be selected and
  consolidated by memories.
- A long-history regression test preserves separate message/note cells and the
  complete assistant response after a stale reasoning-height mismatch.
- Applicable changes from code-mod, Just-Every Code, and OpenAI Codex have been
  re-audited after implementation, with intentional divergences documented.
- Release workflow annotations contain no Node 20 deprecation warning from the
  updated checkout or artifact actions.
- The exact commit's Linux x86_64 beta artifact succeeds and is approved before
  stable preflight or release publication can run.
