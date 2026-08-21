# Main Release Consolidation Implementation Plan

> **Execution mode:** Work inline in the sole repository checkout, directly on
> `main`. Do not create linked worktrees, feature branches, or subagents. Steps
> use checkbox (`- [ ]`) syntax for release tracking.

**Goal:** Recover, complete, submit, and verify every valid intended change from
the former mixed worktree, including direct OpenAI-compatible `/v1` model
selection and the AI Horde auxiliary client.

**Architecture:** Use the backed-up patch and untracked archive as read-only
evidence. Reconstruct coherent behavior sequentially against current `main`,
commit directly on `main`, push directly to `origin/main`, and use one-worker
GitHub Actions for all Rust compilation and testing.

**Tech Stack:** Rust 2024, Tokio, reqwest, serde, ratatui, TOML editing,
`code-secrets`, GitHub Actions, POSIX `sh`.

## Authority And Constraints

- This file is the only implementation tracker for the consolidation.
- Supporting requirements live in `docs/superpowers/specs/`.
- The source baseline is
  `d8a80334438402abd237b083429a67b9dc06aa1c` plus the documentation commits
  made directly on local `main`.
- The immutable recovery source is
  `/var/home/jack/.code-backups/code-worktrees-20260820T174218Z`.
- `feat-mcp-reload/tracked.patch`, `status.txt`, `untracked.list0`, and
  `untracked.tar.gz` are evidence; never apply the complete patch to `main`.
- Do not compile or run Rust tests locally. Compilation begins only after a
  commit is submitted and pushed to `origin/main`.
- Every hosted Rust command must retain `CARGO_BUILD_JOBS=1` and `-j1` where
  the command accepts a job count.
- Preserve bidirectional coverage. Test each direction independently rather
  than inferring reverse behavior.
- Do not use broad runtime correction code to hide invalid source data or
  incomplete implementations.

## Coverage Map

| Requirement | Source | Evidence | Status |
|---|---|---|---|
| Preserve all mixed edits before cleanup | Operator correction | Backup patch, archive, checksums, and status under the immutable backup path | Validated |
| Use one checkout directly on `main` | Operator correction | `git worktree list` reports one checkout; local branch is `main` | Validated |
| Remove the worktree skill | Operator correction | No `using-git-worktrees` skill file or reference remains under `.code` or `.codex` | Validated |
| Retain approved execution resource requests | Resource request specification | Commit `495cc00b2`; hosted resource and full-build runs from the prior consolidation | Validated |
| Retain live MCP reload | MCP reload specification | Commit `d8a803344`; hosted MCP and full-build runs from the prior consolidation | Validated |
| Recover valid dirty correctness fixes | Backed patch and operator approval | File-level disposition and regression tasks below | In progress |
| Direct OpenAI-compatible `/v1` selection | Direct provider specification | Negative search confirms no implementation on `main` | Required |
| AI Horde auxiliary client | AI Horde specification | Negative search confirms no implementation on `main` | Required |
| Produce a release binary | Operator goal | `Build Code` workflow builds and smoke-tests after pushes to `main` | Required |

## Backed Edit Disposition

### Already Integrated

These paths primarily contain the resource-request, MCP-reload, agent-settings,
or one-worker build changes already reconstructed and verified on `main`. Their
backed hunks are not copied again:

- `code-rs/app-server/src/message_processor/v2.rs`
- `code-rs/core/src/cgroup.rs`
- `code-rs/core/src/codex/exec_tool.rs`
- `code-rs/core/src/codex/streaming/submission/configure_session/build_session.rs`
- `code-rs/core/src/conversation_manager.rs`
- `code-rs/core/src/error.rs`
- `code-rs/core/src/exec.rs`
- `code-rs/core/src/lib.rs`
- `code-rs/core/src/openai_tools/registry.rs`
- `code-rs/core/src/openai_tools/tests.rs`
- `code-rs/core/src/spawn.rs`
- `code-rs/core/src/tools/handlers/exec_command.rs`
- `code-rs/core/src/tools/handlers/mod.rs`
- `code-rs/core/src/tools/router.rs`
- `code-rs/core/src/unified_exec/mod.rs`
- `code-rs/exec/src/event_processor_with_human_output.rs`
- `code-rs/mcp-server/src/code_tool_runner.rs`
- `code-rs/protocol/src/lib.rs`
- `code-rs/protocol/src/protocol.rs`
- `code-rs/tui/src/app/events/run/review_model_selection.rs`
- `code-rs/tui/src/app_event.rs`
- `code-rs/tui/src/bottom_pane/settings_pages/mcp/state.rs`
- `code-rs/tui/src/bottom_pane/settings_pages/mcp/tests.rs`
- `code-rs/tui/src/chatwidget/code_event_pipeline/approval_events.rs`
- `code-rs/tui/src/chatwidget/history_pipeline/runtime_flow/approvals.rs`
- `code-rs/tui/src/chatwidget/impl_chunks/validation_and_mcp_commands.rs`
- `code-rs/tui/src/chatwidget/notifications.rs`
- `code-rs/tui/src/user_approval_widget.rs`
- `docs/exec.md`
- `code-rs/core/src/resource_grants.rs`
- `code-rs/core/src/tools/handlers/request_resources.rs`
- `code-rs/core/tests/mcp_reload.rs`
- `code-rs/protocol/src/request_resources.rs`
- `docs/superpowers/specs/2026-08-16-mcp-reload-design.md`
- `docs/superpowers/specs/2026-08-18-execution-resource-requests-design.md`

### Valid Dirty Fixes To Reconstruct

These paths contain behavior absent from current `main`. Reconstruct only the
named behavior, not every backed hunk:

- App-server config-home correctness and reload fanout coverage:
  `code-rs/app-server/src/message_processor.rs`.
- Execution compatibility and shutdown correctness:
  `code-rs/core/src/agent_tool/exec/cloud.rs`,
  `code-rs/core/src/agent_tool/exec/runner/model_exec/spawn_exec.rs`,
  `code-rs/core/src/codex/session.rs`,
  `code-rs/core/src/exec_command/exec_command_session.rs`,
  `code-rs/core/src/exec_command/session_manager.rs`,
  `code-rs/core/src/landlock.rs`, and `code-rs/core/src/seatbelt.rs`.
- Config, schema, shell, watcher, context, and client-version correctness:
  `code-rs/core/config.schema.codex.json`, `code-rs/core/src/config.rs`,
  `code-rs/core/src/config/schema.rs`, `code-rs/core/src/default_client.rs`,
  `code-rs/core/src/file_watcher.rs`, and `code-rs/core/src/shell.rs`.
- MCP lifecycle and presentation edges:
  `code-rs/core/src/codex/streaming/submission/mod.rs`,
  `code-rs/core/src/mcp_connection_manager.rs`,
  `code-rs/core/src/protocol.rs`,
  `code-rs/tui/src/bottom_pane/settings_pages/mcp/presentation/servers_list.rs`,
  `code-rs/tui/src/bottom_pane/settings_pages/mcp/presentation/summary.rs`, and
  `code-rs/tui/src/bottom_pane/settings_pages/mcp/presentation/tools_list.rs`.
- Review coordination and terminal-event ordering:
  `code-rs/core/tests/review_coord_integration.rs`,
  `code-rs/tui/src/chatwidget/code_event_pipeline.rs`,
  `code-rs/tui/src/chatwidget/code_event_pipeline/exec_events.rs`,
  `code-rs/tui/src/chatwidget/code_event_pipeline/task_events.rs`,
  `code-rs/tui/src/chatwidget/interrupts.rs`,
  `code-rs/tui/src/chatwidget/session_flow/fork.rs`,
  `code-rs/tui/src/chatwidget/session_flow/startup.rs`,
  `code-rs/tui/src/chatwidget/shared_defs/preamble.rs`,
  `code-rs/tui/src/chatwidget/tests/review.rs`, and
  `code-rs/tui/src/chatwidget/web_search_sessions.rs`.
- Transcript geometry and keyboard/mouse scrolling:
  `code-rs/tui/src/bottom_pane/chat_composer/history.rs`,
  `code-rs/tui/src/chatwidget/history_render/render_state.rs`,
  `code-rs/tui/src/chatwidget/history_virtualization_impl.rs`,
  `code-rs/tui/src/chatwidget/impl_chunks/agents_overlay_and_terminal_mode.rs`,
  `code-rs/tui/src/chatwidget/impl_chunks/perf_spinner_interrupts_redraw.rs`,
  `code-rs/tui/src/chatwidget/input_pipeline/key_event/history_shortcuts.rs`,
  `code-rs/tui/src/chatwidget/input_pipeline/key_event/key_handler.rs`,
  `code-rs/tui/src/chatwidget/input_pipeline/mouse/scrollbar.rs`,
  `code-rs/tui/src/chatwidget/internals/state.rs`,
  `code-rs/tui/src/chatwidget/layout_scroll.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/agents_terminal_overlay.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller/render_pass.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller/render_pass/cell_paint.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller/render_pass/post_paint.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller/render_pass/window_selection.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller/scroll_layout.rs`,
  `code-rs/tui/src/chatwidget/session_flow/config.rs`,
  `code-rs/tui/src/chatwidget/smoke_helpers.rs`, and `code-rs/tui/src/lib.rs`.
- Settings persistence regression coverage:
  `code-rs/tui/src/bottom_pane/settings_pages/shell/tests.rs` and
  `code-rs/tui/src/chatwidget/settings_handlers/keys.rs`.
- Resource/MCP wording and compatibility hunks requiring comparison rather
  than blind replay: `code-rs/core/src/openai_tools/builtin_tools.rs`.

### Required And Incomplete

- `docs/superpowers/specs/2026-08-18-direct-openai-v1-model-selection-design.md`
  defines behavior with no implementation on `main`.
- `docs/superpowers/specs/2026-08-18-ai-horde-auxiliary-client-design.md`
  defines behavior with no implementation on `main`.

### Superseded Or Duplicate

- `docs/superpowers/plans/2026-08-16-mcp-reload.md`,
  `docs/superpowers/plans/2026-08-18-ai-horde-auxiliary-client.md`,
  `docs/superpowers/plans/2026-08-18-direct-openai-v1-model-selection.md`, and
  `docs/superpowers/plans/2026-08-18-execution-resource-requests.md` are
  superseded by this ledger. They are not restored.
- The unmerged `ci/automatic-build-artifacts` branch is superseded by
  `48519e269`, `d059754aa`, and the current `.github/workflows/preview-build.yml`.
- `feat/gpt-5.6-agents`, `integrate/mcp-reload`, and
  `integrate/resource-requests` point to commits contained by `origin/main` and
  are branch-cleanup candidates after final verification.
- `feat/mcp-reload` and `integrate/release-consolidation` contain no unique
  committed implementation required by the release; their former uncommitted
  evidence is preserved in the immutable backup.
- Exact backed lines already present on `main` are duplicate evidence even when
  their containing file also appears in the valid-fix list.

## Phase 1: Core And Lifecycle Hardening

- [ ] **Task 1.1: Restore app-server config-home correctness and reload fanout coverage**

  **Files:** `code-rs/app-server/src/message_processor.rs`.

  **Implementation:** Build effective config through `ConfigBuilder` with
  `self.base_config.code_home`, CLI overrides, request overrides, and default
  loader overrides. Add a two-conversation regression that calls
  `mcp_server_refresh_v2`, verifies the response envelope, and then observes an
  MCP snapshot on both live conversations.

  **Hosted verification:** Add the test to the MCP focused workflow command or
  extend its path coverage to include `message_processor.rs`; push `main` and
  require `MCP Reload Regression Tests` and `Build Code` to pass.

- [ ] **Task 1.2: Resolve pending resource requests during session shutdown**

  **Files:** `code-rs/core/src/codex/session.rs`.

  **Implementation:** Drain pending resource approvals using the same denial
  response semantics as other pending approvals. Add
  `pending_resource_requests_are_resolved_as_denied` so shutdown cannot leave a
  requester waiting indefinitely.

  **Hosted verification:** Push `main`; require `Resource Request Regression
  Tests` and `Build Code` to pass.

- [ ] **Task 1.3: Preserve initial exec-session output without a subscriber race**

  **Files:** `code-rs/core/src/exec_command/exec_command_session.rs` and
  `code-rs/core/src/exec_command/session_manager.rs`.

  **Implementation:** Create the initial broadcast receiver before the spawned
  session can publish output, pass it into `ExecCommandSession::new`, and use it
  for the first response. Add a deterministic test that emits output
  immediately after spawn and proves the first call receives it exactly once.

  **Hosted verification:** Push `main`; require `Resource Request Regression
  Tests` and `Build Code` to pass.

- [ ] **Task 1.4: Keep legacy sandbox call sites on default execution limits**

  **Files:** `code-rs/core/src/agent_tool/exec/cloud.rs`,
  `code-rs/core/src/agent_tool/exec/runner/model_exec/spawn_exec.rs`,
  `code-rs/core/src/landlock.rs`, and `code-rs/core/src/seatbelt.rs`.

  **Implementation:** Preserve the public legacy wrapper signatures and route
  them through limit-aware internal functions with
  `ExecCgroupLimits::default()`. Confirm both Linux sandbox and seatbelt paths
  preserve prior behavior when no grant is supplied.

  **Hosted verification:** Push `main`; require `Resource Request Regression
  Tests` and `Build Code` to pass.

- [ ] **Task 1.5: Recover independent small correctness regressions**

  **Files:** `code-rs/core/src/file_watcher.rs`, `code-rs/core/src/shell.rs`,
  `code-rs/core/src/config.rs`, `code-rs/core/src/config/schema.rs`,
  `code-rs/core/src/default_client.rs`,
  `code-rs/core/tests/review_coord_integration.rs`,
  `code-rs/tui/src/bottom_pane/settings_pages/shell/tests.rs`, and
  `code-rs/tui/src/chatwidget/settings_handlers/keys.rs`.

  **Implementation:** Recover poisoned watcher-lock state rather than dropping
  events; recognize Windows separators in `shell_basename`; cover GPT-5.5 1M
  context expansion and wire-compatible client version; serialize tests that
  mutate process-global review coordination state; preserve shell edit focus;
  and keep pending icon-mode changes when returning to settings overview.

  **Hosted verification:** Push `main`; require `Settings and Agent Regression
  Tests`, `MCP Reload Regression Tests` when watcher code changes, and `Build
  Code` to pass.

## Phase 2: MCP And Resource Edge Completion

- [ ] **Task 2.1: Audit and recover manager-owned disabled-server edge tests**

  **Files:** `code-rs/core/src/mcp_connection_manager.rs`,
  `code-rs/core/src/codex/streaming/submission/mod.rs`, and
  `code-rs/core/tests/mcp_reload.rs`.

  **Implementation:** Compare backed tests against current coverage. Add only
  absent cases for startup-disabled enablement, stable case-insensitive server
  ordering, and disabled-tool snapshots. Do not replace the verified manager
  implementation or reintroduce client-supplied server config.

  **Hosted verification:** Push `main`; require `MCP Reload Regression Tests`
  and `Build Code` to pass.

- [ ] **Task 2.2: Reconcile settings text with live MCP actions**

  **Files:**
  `code-rs/tui/src/bottom_pane/settings_pages/mcp/presentation/servers_list.rs`,
  `code-rs/tui/src/bottom_pane/settings_pages/mcp/presentation/summary.rs`, and
  `code-rs/tui/src/bottom_pane/settings_pages/mcp/presentation/tools_list.rs`.

  **Implementation:** Make labels state that `R` reloads live server status and
  `S` queues `/mcp status`; retain existing key behavior and add rendering
  assertions.

  **Hosted verification:** Push `main`; require `MCP Reload Regression Tests`,
  `Settings and Agent Regression Tests`, and `Build Code` to pass.

- [ ] **Task 2.3: Reconcile resource tool schema wording**

  **Files:** `code-rs/core/src/openai_tools/builtin_tools.rs` and
  `code-rs/core/src/openai_tools/tests.rs`.

  **Implementation:** Compare the backed descriptions to current schemas. Keep
  current semantics unless the text is inaccurate; if changed, assert exact
  memory and process-limit descriptions without changing accepted inputs.

  **Hosted verification:** Push `main`; require `Resource Request Regression
  Tests` and `Build Code` to pass.

## Phase 3: Terminal Event Ordering

- [ ] **Task 3.1: Queue all order-sensitive terminal events consistently**

  **Files:** `code-rs/tui/src/chatwidget/interrupts.rs`,
  `code-rs/tui/src/chatwidget/code_event_pipeline.rs`,
  `code-rs/tui/src/chatwidget/code_event_pipeline/exec_events.rs`, and
  `code-rs/tui/src/chatwidget/code_event_pipeline/task_events.rs`.

  **Implementation:** Extend queued interrupts to approval and terminal event
  variants, retain provider order metadata, add `submission_id`, and provide
  `discard_submission`. A task-complete event must discard queued terminal
  events for the same submission without affecting later submissions.

  **Tests:** Restore focused cases named
  `task_complete_discards_queued_exec_end_for_same_submission`,
  `task_complete_rejects_late_search_events_for_same_submission`, and
  `task_complete_keeps_final_answer_after_search_card`.

  **Hosted verification:** Push `main`; require `Settings and Agent Regression
  Tests`, `MCP Reload Regression Tests`, `Resource Request Regression Tests`,
  and `Build Code` to pass because the shared event pipeline spans all three.

- [ ] **Task 3.2: Bound completed-submission history and initialize every session path**

  **Files:** `code-rs/tui/src/chatwidget/shared_defs/preamble.rs`,
  `code-rs/tui/src/chatwidget/session_flow/fork.rs`,
  `code-rs/tui/src/chatwidget/session_flow/startup.rs`,
  `code-rs/tui/src/chatwidget/tests/review.rs`, and
  `code-rs/tui/src/chatwidget/web_search_sessions.rs`.

  **Implementation:** Maintain a bounded `VecDeque<String>` of completed
  submission IDs, initialize it for startup and forked sessions, and reject
  delayed provider events for completed submissions. Remove the old behavior
  that allows a late search/end event to mutate finalized transcript history.

  **Hosted verification:** Use the same pushed workflows as Task 3.1.

## Phase 4: Transcript Geometry And Navigation

- [ ] **Task 4.1: Promote transcript geometry from `u16` to `u32`**

  **Files:** `code-rs/tui/src/chatwidget/history_render/render_state.rs`,
  `code-rs/tui/src/chatwidget/history_virtualization_impl.rs`,
  `code-rs/tui/src/chatwidget/internals/state.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller.rs`,
  and the files beneath
  `code-rs/tui/src/chatwidget/overlay_rendering/widget_render/history_scroller/render_pass/`.

  **Implementation:** Store prefix sums, total heights, spacing ranges, and
  scroll positions as `u32`. Convert to terminal `u16` coordinates only at the
  final bounded paint operation. Add
  `history_row_offsets_preserve_more_than_u16_rows` with enough rows to exceed
  65,535.

- [ ] **Task 4.2: Keep keyboard, mouse, overlays, and scrollbar on one scroll model**

  **Files:** `code-rs/tui/src/chatwidget/layout_scroll.rs`,
  `code-rs/tui/src/chatwidget/input_pipeline/key_event/history_shortcuts.rs`,
  `code-rs/tui/src/chatwidget/input_pipeline/mouse/scrollbar.rs`,
  `code-rs/tui/src/chatwidget/impl_chunks/agents_overlay_and_terminal_mode.rs`,
  `code-rs/tui/src/chatwidget/impl_chunks/perf_spinner_interrupts_redraw.rs`,
  `code-rs/tui/src/chatwidget/overlay_rendering/agents_terminal_overlay.rs`,
  `code-rs/tui/src/chatwidget/session_flow/config.rs`,
  `code-rs/tui/src/chatwidget/smoke_helpers.rs`, and `code-rs/tui/src/lib.rs`.

  **Implementation:** Use `u32` scroll offsets throughout, clamp only against
  the current total height, and ensure scrollbar conversion and overlay restore
  use the same maximum-scroll calculation.

- [ ] **Task 4.3: Separate transcript arrows from sent-message history**

  **Files:** `code-rs/tui/src/bottom_pane/chat_composer/history.rs`,
  `code-rs/tui/src/chatwidget/input_pipeline/key_event/key_handler.rs`, and
  `code-rs/tui/src/chatwidget/tests/review.rs`.

  **Implementation:** With an empty composer, Up and Down scroll the transcript.
  At the start of a non-empty composer, Up still begins sent-message history
  navigation and Down restores the draft.

  **Tests:** Restore
  `empty_composer_up_scrolls_transcript_instead_of_recalling_history`,
  `empty_composer_down_scrolls_transcript_toward_latest`, and
  `up_at_start_of_non_empty_composer_still_recalls_input_history`.

  **Hosted verification for Phase 4:** Push `main`; require `Settings and Agent
  Regression Tests`, `MCP Reload Regression Tests`, `Resource Request
  Regression Tests`, and `Build Code` to pass.

## Phase 5: Direct OpenAI-Compatible `/v1` Model Selection

- [x] **Task 5.1: Add normalized direct-provider persistence**

  **Files:** `code-rs/core/src/model_provider_info.rs`,
  `code-rs/core/src/config.rs`, the existing config write service under
  `code-rs/core/src/config/`, and `code-rs/core/config.schema.codex.json`.

  **Implementation:** Add deterministic collision-safe provider IDs, normalize
  user input to one `/v1` root, and persist a normal
  `model_providers.<provider_id>` entry without creating or activating a
  profile. Persist only the encrypted-secret reference in `env_key`.

  **Tests:** Round-trip unauthenticated and authenticated provider entries,
  duplicate display names with different URLs, trailing `/v1` variants, and
  proof that `[profiles]` is unchanged.

  **Evidence:** Exact SHA `6e41e6b4ed24d4d02e982eb828bf920d94b7417d`;
  Provider Integration Tests `32428319179` and Build Code `32428319115`
  succeeded.

- [x] **Task 5.2: Scope remote model catalogs and caches by provider**

  **Files:** `code-rs/core/src/remote_models/mod.rs` and
  `code-rs/core/src/remote_models/cache.rs`.

  **Implementation:** Key refresh state and cache paths by provider ID. Retain
  backward-compatible loading for the built-in OpenAI cache. Return explicit
  fresh, stale, loading, authentication-error, and connection-error state while
  retaining stale models after a failed refresh.

  **Tests:** Two providers exposing the same model ID, provider-isolated cache
  writes, stale retention after authentication failure, and one-provider
  refresh that leaves the other provider unchanged.

  **Evidence:** Exact SHA `8bba7a3f6882573809ea36667afde95fc1429244`;
  Provider Integration Tests `32432177692` and Build Code `32432177691`
  succeeded.

- [x] **Task 5.3: Add the inline endpoint form**

  **Files:** Files under
  `code-rs/tui/src/bottom_pane/settings_pages/model/model_selection_view/`, plus
  a focused `endpoint_form.rs` if current module boundaries require it.

  **Implementation:** Add `+ Add OpenAI-compatible /v1 endpoint`; collect
  display name, base URL, optional key, and Chat Completions versus Responses;
  default to Chat Completions; validate and refresh inline; save a provided key
  through `code_secrets::SecretsManager`; cancel cleanly on Escape.

  **Tests:** Inline validation, no-key local endpoint, encrypted-key reference,
  authentication error without cache deletion, and successful return to the
  model list.

  **Evidence:** Exact SHA `f119dbc9516921252af403e4b0e8ad847496e116`;
  Provider Integration Tests `32440081529` and Build Code `32440081439`
  succeeded.

- [ ] **Task 5.4: Group provider models and apply direct session selection**

  **Files:** Files under
  `code-rs/tui/src/bottom_pane/settings_pages/model/model_selection_state/`,
  `code-rs/tui/src/chatwidget/session_tuning_flow/apply_and_sync.rs`, and
  `code-rs/tui/src/app_event.rs`.

  **Implementation:** Use provider-qualified row identities, render endpoint
  headings and catalog state, and update the active model plus provider through
  the existing session reconfiguration path. Do not create, select, or modify a
  profile.

  **Tests:** Equal model IDs from two providers remain independently selectable;
  direct selection changes `model_provider_id` and model while preserving
  `active_profile`; OpenAI-compatible rows never show Horde worker counts.

- [ ] **Task 5.5: Add hosted direct-provider regression coverage and docs**

  **Files:** `.github/workflows/provider-integration-tests.yml`,
  `docs/settings.md`, `docs/config.md`, and `docs/slash-commands.md`.

  **Implementation:** Add a one-worker workflow that runs exact core and TUI
  direct-provider tests after pushes to `main`. Document local no-key endpoints,
  encrypted keys, Chat versus Responses, stale catalogs, and direct selection
  without profiles.

  **Hosted verification:** Push `main`; require `Provider Integration Tests`,
  `Settings and Agent Regression Tests`, and `Build Code` to pass.

## Phase 6: AI Horde Auxiliary Client

- [ ] **Task 6.1: Validate the public API contract before coding**

  **Sources:** Current official AI Horde API documentation and the authoritative
  API repository/schema.

  **Validation:** Confirm text and image submit/check/status/cancel routes,
  worker and model responses, interrogation forms including `vectorize`, Kudos
  fields, cancellation semantics, authentication headers, anonymous key, and
  image payload encoding. Record any required correction in the AI Horde spec
  before implementation.

- [ ] **Task 6.2: Add a focused AI Horde client crate**

  **Files:** `code-rs/ai-horde-client/Cargo.toml`, files under
  `code-rs/ai-horde-client/src/`, and `code-rs/Cargo.toml`.

  **Implementation:** Add typed API models; bounded reqwest operations for
  model/worker listing, text and image async lifecycle, cancellation, and
  interrogation; redact keys from all errors; preserve unknown response fields
  with serde defaults where forward compatibility requires it.

  **Tests:** Mock-server coverage for supplied credentials, anonymous fallback
  `0000000000`, `Client-Agent`, model/worker parsing, timeout, error redaction,
  and every lifecycle route.

- [ ] **Task 6.3: Implement context-aware ordered fallback**

  **Files:** `code-rs/ai-horde-client/src/selection.rs` and
  `code-rs/ai-horde-client/src/generation.rs`.

  **Implementation:** For explicit text models, filter online capable workers,
  sort by `max_context_length` then worker ID, attempt the smallest sufficient
  worker first, cancel before fallback, and stop if cancellation fails. `Auto`
  omits both model and worker restrictions.

  **Tests:** Workers at 4K, 8K, and 32K with a 6K request produce `[8K, 32K]`;
  `Auto` produces no restriction; timeout cancels before the next submit; failed
  cancellation prevents duplicate work.

- [ ] **Task 6.4: Add credentials, cache, and managed artifacts**

  **Files:** `code-rs/core/src/ai_horde.rs`, `code-rs/core/src/lib.rs`,
  `code-rs/core/Cargo.toml`, and the current shared service-state module.

  **Implementation:** Resolve key from encrypted store, then environment, then
  anonymous fallback; expose the credential source without exposing the key;
  maintain short-lived model/worker caches with stale state; atomically write
  size-bounded generated images beneath `CODE_HOME/cache/ai-horde/`.

  **Tests:** Credential precedence, no key leakage, stale cache after API
  failure, atomic artifact paths, invalid base64, and payload size rejection.

- [ ] **Task 6.5: Register built-in AI Horde tools**

  **Files:** `code-rs/core/src/openai_tools/builtin_tools.rs`,
  `code-rs/core/src/openai_tools/registry.rs`,
  `code-rs/core/src/tools/handlers/ai_horde.rs`,
  `code-rs/core/src/tools/handlers/mod.rs`, and
  `code-rs/core/src/tools/router.rs`.

  **Implementation:** Register `horde_list_models`, `horde_generate_text`,
  `horde_generate_image`, `horde_interrogate_image`, and
  `horde_vectorize_image`. Return queue, worker, cancellation, fault, Kudos, and
  artifact metadata without returning credentials or full base64 images.

  **Tests:** Exact schemas, `Auto` versus explicit model behavior, image-only
  vectorization wording, no-capable-worker errors, cancellation, and artifact
  return values.

- [ ] **Task 6.6: Add the TUI auxiliary catalog**

  **Files:** `code-rs/tui/src/app_event.rs`, model-selection state and view
  files, and model-selection tests.

  **Implementation:** Add separate Horde text and image auxiliary targets;
  refresh on open and manual action; render `Auto (AI Horde selects worker)`,
  exact online worker counts, stale/loading/error states, and the anonymous
  queue-priority notice. Never change the primary session model/provider.

  **Tests:** Exact labels, verified zero workers distinct from loading/failure,
  anonymous notice, cached stale state, and auxiliary selection that leaves the
  primary model unchanged.

- [ ] **Task 6.7: Add hosted Horde coverage and documentation**

  **Files:** `.github/workflows/provider-integration-tests.yml`,
  `docs/ai-horde.md`, `docs/settings.md`, and `docs/index.md` if it indexes user
  documentation.

  **Implementation:** Extend the one-worker provider workflow with exact client,
  core, and TUI Horde tests. Document credentials, anonymous priority, Auto,
  worker ordering, cancellation, artifacts, tools, and image-only vectors.

  **Hosted verification:** Push `main`; require `Provider Integration Tests`,
  `Settings and Agent Regression Tests`, and `Build Code` to pass.

## Phase 7: Final Consolidation And Release

- [ ] **Task 7.1: Complete the disposition audit**

  Re-read every entry in the backed status, patch, and untracked archive. Mark
  each ledger task complete only with a commit and hosted run. Confirm no backed
  path remains unclassified and no useful requirement exists only in an old
  implementation plan.

- [ ] **Task 7.2: Delete superseded local branches after evidence is retained**

  Delete only local branches whose commits are ancestors of verified
  `origin/main` or whose unique unmerged content is explicitly classified as
  superseded in this ledger. Do not delete the immutable backup archive.

- [ ] **Task 7.3: Run static final gates locally without compiling**

  Run `git diff --check`, focused `rustfmt --check` for changed Rust files,
  `actionlint` for changed workflows, YAML parsing, stale-symbol searches,
  artifact searches, and a changeset-minimalist review. Do not run `cargo test`,
  `cargo check`, `cargo build`, or repository build scripts locally.

- [ ] **Task 7.4: Submit the final main release build**

  Push local `main` directly to `origin/main`. Monitor all workflows to terminal
  state using `gh run watch`; inspect exact failed job logs and commit fixes
  directly on `main`. Require `Build Code` to compile, package, upload, and
  smoke-test the release binary with one worker.

- [ ] **Task 7.5: Close the release ledger with evidence**

  Record final commit SHAs and GitHub Actions run IDs in this file, verify local
  `main` equals `origin/main`, verify the sole checkout is clean, and leave no
  unresolved required checkbox.

## Final Evidence

- Final `origin/main` commit: pending until Task 7.5.
- Provider workflow run: pending until Tasks 5.5 and 6.7.
- Full `Build Code` run: pending until Task 7.4.
- Remaining blocked items: none known at plan creation.
