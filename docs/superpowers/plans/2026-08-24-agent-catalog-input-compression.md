# Agent Catalog and Operator Input Compression Implementation Plan

> Approved design: `docs/superpowers/specs/2026-08-24-agent-catalog-input-compression-design.md`
>
> Execution constraints: edit only `code-rs`; use `apply_patch`; never run `rustfmt`; run every Rust build or test with `CARGO_BUILD_JOBS=1` and `-j1`; preserve the unrelated untracked `docs/superpowers/plans/2026-08-23-release-runtime-regression-repair.md`.

## Coverage Map

| Requirement | Implementation area | Verification |
| --- | --- | --- |
| One model-bound copy per operator submission | `code-rs/core/src/codex/streaming/agent/run.rs`, request test support | Request-capture regression test counts one user occurrence per submission identity |
| Every discovered model has an agent binding | `code-rs/core/src/agent_defaults.rs` | Catalog coverage and uniqueness tests |
| Independent model-agent toggles | Core config update path plus TUI agents settings model/input/render | Toggle unit tests and persistence assertions in both directions |
| Simple custom provider/model-slug agents | `code-rs/tui/src/chatwidget/agent_editor_flow.rs` and focused bottom-pane view | Flow/config construction tests including unknown slugs |
| OpenRouter Free and Paid sections | `code-rs/tui/src/bottom_pane/settings_pages/model/` | Classification, order, and rendering tests |
| Deterministic non-LLM compression | New core compression module and config plumbing | Golden tests for standard, aggressive, protected, short, and fallback cases |
| Original local history retained | Core submission/history split | Request-capture test compares stored original with compressed outbound copy |

## Task 1: Reproduce and Fix Duplicate Outbound Operator Input

**Files**

- Modify: `code-rs/core/src/codex/streaming/agent/run.rs`
- Modify as required: `code-rs/core/src/codex/streaming/turn/mod.rs`
- Modify as required: `code-rs/core/src/codex/session.rs`
- Test: `code-rs/core/tests/`

1. Add the smallest provider request-capture regression test that submits one uniquely identified `Op::UserInput` and asserts the first outbound model request contains that user text exactly once.
2. Run the focused test and confirm it fails because the payload contains two copies, not because setup is invalid.
3. Trace the captured duplicate to the exact assembly boundary. Preserve one original conversation-history record and one model-bound representation.
4. Fix by carrying submission identity through assembly or by ensuring the already-recorded initial item is not appended a second time. Do not deduplicate equal text across separate submissions.
5. Add a second assertion proving two distinct submissions with identical text are both retained once.
6. Run: `CARGO_BUILD_JOBS=1 cargo test -p code-core <duplicate_test_name> -j1` from `code-rs`; expect pass.

## Task 2: Build One Unified Model-Agent Catalog

**Files**

- Modify: `code-rs/core/src/agent_defaults.rs`
- Modify as required: `code-rs/core/src/config_types.rs`
- Test: module tests in `code-rs/core/src/agent_defaults.rs`

1. Add failing tests that enumerate the model-store/direct-provider catalog and require one unique model-agent binding for every provider/model slug.
2. Add tests preserving curated agent names, descriptions, arguments, and enabled defaults.
3. Add canonical provider/model identity to agent specifications/config only where needed to prevent slug collisions and support settings rows.
4. Generate disabled-by-default bindings for discovered models missing a curated binding. Generate executable arguments from provider/model identity without requiring a hard-coded entry per model.
5. Make `default_agent_configs()` return the complete catalog while preserving curated enabled defaults and generated disabled defaults.
6. Run: `CARGO_BUILD_JOBS=1 cargo test -p code-core agent_defaults -j1`; expect pass.

## Task 3: Make Agent Availability Mutable and Persisted

**Files**

- Modify: `code-rs/core/src/codex/session.rs`
- Modify: `code-rs/core/src/codex/agent_tool_call.rs`
- Modify as required: `code-rs/core/src/tools/repl/mod.rs`
- Modify: `code-rs/tui/src/chatwidget/settings_routing/builders.rs`
- Modify: `code-rs/tui/src/chatwidget/settings_overlay/agents/model.rs`
- Modify: `code-rs/tui/src/chatwidget/settings_overlay/agents/input.rs`
- Modify: `code-rs/tui/src/chatwidget/settings_overlay/agents/render.rs`
- Modify: `code-rs/tui/src/chatwidget/agent_editor_flow.rs`
- Test: focused core and TUI module tests

1. Add failing overview-row tests requiring all catalog entries, independent `enabled` state, and provider/model identity.
2. Add failing input tests for Space, Left, Right, and mouse toggles in both directions.
3. Add a lock-backed session snapshot/update path for agent configs and allowed tool values, updating all readers to use snapshots.
4. Persist toggles through the existing config writer and refresh the active session/tool schema immediately.
5. Render clear enabled/disabled state and update footer hints without conflating installation with enablement.
6. Run: `CARGO_BUILD_JOBS=1 cargo test -p code-tui --features test-helpers agents -j1`; expect pass.
7. Run the focused core agent tool test if touched: `CARGO_BUILD_JOBS=1 cargo test -p code-core agent_tool -j1`; expect pass.

## Task 4: Add the Simple Custom Model-Agent Flow

**Files**

- Modify: `code-rs/tui/src/chatwidget/agent_editor_flow.rs`
- Add or modify a focused view under `code-rs/tui/src/bottom_pane/settings_pages/agents/`
- Modify as required: `code-rs/core/src/agent_defaults.rs`
- Test: focused view and config-construction tests

1. Add failing tests that enter provider plus model slug and produce an enabled Code CLI agent config with stable unique name and correct model arguments.
2. Implement a simple provider/model-slug form reachable from Add New Agent.
3. Keep the existing arbitrary executable editor reachable as an Advanced path.
4. Reject empty provider/slug and duplicate provider/slug identities with an actionable inline error.
5. Persist the custom binding and refresh active agent/tool state immediately.
6. Run: `CARGO_BUILD_JOBS=1 cargo test -p code-tui --features test-helpers agent_editor -j1`; expect pass.

## Task 5: Split OpenRouter Models into Free and Paid Sections

**Files**

- Modify: `code-rs/tui/src/bottom_pane/settings_pages/model/model_selection_state/data.rs`
- Modify as required: `code-rs/tui/src/bottom_pane/settings_pages/model/model_selection_state/presets.rs`
- Modify: `code-rs/tui/src/bottom_pane/settings_pages/model/model_selection_view/render.rs`
- Test: `code-rs/tui/src/bottom_pane/settings_pages/model/model_selection_view/tests.rs` and state tests

1. Add failing tests for case-insensitive OpenRouter provider detection and `:free` suffix classification.
2. Add failing ordering tests requiring OpenRouter Free before OpenRouter Paid while preserving model order inside each section.
3. Represent section metadata explicitly enough that rendering does not infer headings from adjacent rows.
4. Render Free and Paid headings only for OpenRouter; leave other provider group behavior unchanged.
5. Run: `CARGO_BUILD_JOBS=1 cargo test -p code-tui --features test-helpers model_selection -j1`; expect pass.

## Task 6: Add Deterministic Operator-Input Compression

**Files**

- Add: `code-rs/core/src/operator_input_compression.rs`
- Modify: `code-rs/core/src/lib.rs`
- Modify: `code-rs/core/src/config_types.rs`
- Modify: `code-rs/core/src/config/mod.rs` and config-loading tests as required
- Modify: `code-rs/core/src/codex/streaming/agent/run.rs`
- Modify the existing TUI interface/settings page and event/config persistence path discovered during implementation
- Test: core compression unit tests, config tests, request/history integration test, TUI settings tests

1. Add failing golden tests for standard compression: whitespace normalization, exact duplicate paragraph/clause removal, and conservative filler removal.
2. Add failing aggressive-mode tests for additional deterministic terse clause rewriting.
3. Add failing protection tests for fenced and inline code, quotes, paths, URLs, numbers, structured data, lists, mentions, IDs, and requirement keywords.
4. Add failing safety tests: short/protected-only input unchanged; empty/error result falls back to original.
5. Add config defaults: `input_compression.enabled = true`, `input_compression.aggressive = false`.
6. Apply compression only to cloned operator-origin text used for model requests. Keep the original input in TUI history, shell history, and conversation/rollout history.
7. Add settings toggles with immediate persistence. Aggressive is subordinate to standard enablement but retains its stored value while standard compression is off.
8. Run: `CARGO_BUILD_JOBS=1 cargo test -p code-core operator_input_compression -j1`; expect pass.
9. Run the request/history integration test and focused TUI settings test; expect pass.

## Task 7: Review and Final Verification

1. Search for all direct `Session.agents` and `Session.tools_config` readers and ensure they use the new snapshot/update API where mutation matters.
2. Search for duplicate agent/model definitions and reuse the unified catalog rather than adding parallel lists.
3. Review the diff against every row in the coverage map; remove dead code and fix all warnings.
4. Run `git diff --check`; expect no output.
5. Run the single required completion build from the repository root: `CARGO_BUILD_JOBS=1 ./build-fast.sh`; expect exit 0 with no warnings.
6. Inspect `git status --short --branch` and confirm the unrelated untracked 2026-08-23 plan remains untouched.

## Self-Review

- The plan covers every approved requirement and explicitly protects original history while transforming only the outbound copy.
- The duplicate fix is identity-aware and does not collapse intentional repeated prompts.
- The catalog is generated from shared model data rather than a manually maintained subset.
- Curated defaults remain enabled; generated models remain discoverable but disabled.
- OpenRouter grouping uses the approved `:free` convention because no pricing metadata is available.
- Each production phase starts with a failing focused test and ends with a one-thread verification command.
- The final validation is exactly the project-required `./build-fast.sh`, also restricted to one compiler thread.
