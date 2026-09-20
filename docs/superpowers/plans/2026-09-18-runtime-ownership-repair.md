# Runtime Ownership Repair Dossier

> **Status:** Historical failed-repair record. Its completion, build, and
> deployment claims are not current acceptance evidence and were disproved by
> the subsequent operator-input, agent-settings, compaction, provider-state,
> and repository findings. Active work and verified results are tracked only in
> `docs/changes/0001-canonical-history-recovery.md`.
>
> Preserve the body below as an audit artifact; do not resume or mark work from
> this dossier independently of the recovery ledger.
>
> **Execution mode:** Inline only. Do not delegate to read agents or
> sub-agents during this repair. Directory arguments can preload an entire
> subtree, causing excessive context, latency, and failure.

**Goal:** Repair the runtime ownership failures that duplicate or delay
operator input and tool output, destabilize long-session compaction, obscure
effective agent configuration, and allow temporary/project artifacts to escape
their intended lifecycle.

**Architecture principle:** Every runtime item has one canonical owner at each
stage. An item may be persisted once, included in one model request at a given
boundary, and then consumed. Conversation history, pending queues, rollout
recording, and request-only context MUST NOT simultaneously claim the same item
as new input.

**Primary references:**

- `docs/superpowers/specs/2026-08-24-agent-catalog-input-compression-design.md`
- `docs/superpowers/plans/2026-08-24-agent-catalog-input-compression.md`
- `docs/superpowers/plans/2026-08-23-release-runtime-regression-repair.md`

This dossier supersedes the earlier plans only for active-turn input ownership,
long-session context integrity, agent-settings verification, and artifact
lifecycle. Their provider, catalog, and compression requirements remain in
force.

---

## Operator Constraints

- Preserve the dirty worktree and all unrelated operator changes.
- Use `apply_patch` for every source or documentation edit.
- Do not stage, commit, push, reset, clean, rebase, or broadly format unless
  explicitly requested.
- Run every local Rust compile or build with `CARGO_BUILD_JOBS=1`.
- Use `/var/home/jack/tmp/code-build` for `TMPDIR`, `TMP`, and `TEMP`.
- Set `GIT_CEILING_DIRECTORIES=/var/home/jack/tmp/code-build` for tests that
  create repositories below the home directory.
- Do not use a directory as an agent context attachment. Read bounded,
  explicitly named files only.
- Do not deploy over `/var/home/jack/bin/code` without first creating a dated
  backup and verifying the replacement build.
- The active process cannot adopt runtime changes; activation requires a later
  restart approved by the operator.

## Coverage Map

<!-- markdownlint-disable MD013 -->

| Area | Required invariant | Evidence/test | Status |
| --- | --- | --- | --- |
| Normal operator input | One submission identity creates one model-bound user item | `one_operator_submission_reaches_the_model_once_in_compressed_form` | Validated |
| Mid-turn operator input | First available model boundary receives the queued submission exactly once | Single- and multi-tool request-capture integration tests | Validated |
| Tool output | One tool call has one output in history, rollout, compaction input, and each later model request | Request capture and flushed rollout count | Validated |
| Later turns/replay | A consumed queued submission remains once in retained history and is never replayed as new input | Third-request integration assertion and submission-ID history test | Validated |
| Review mode | Queued operator input remains queued and is not cloned into the review model | `review_mode_does_not_send_or_consume_queued_operator_input` | Validated |
| Boundary latency | Input submitted during active work is visible at the earliest safe provider-request boundary | Multi-tool request-capture integration test | Validated |
| Local compaction | Compaction receives structurally valid, non-duplicated history and preserves useful recent context in emergency fallback | Compact sanitization and emergency-history tests | Validated |
| Remote compaction | Remote service failures fall back locally while account/rate-limit failures remain explicit and bounded | Remote fallback classification test | Validated for implemented fallback boundary |
| Long transcript rendering | Complete assistant output remains visible after long histories and compaction | August 23 regressions plus current history-cutoff suite | Validated by seven active cutoff regressions |
| Agent configuration | TUI edits persist, runtime dispatch consumes aliases/canonical names, and effective execution identity reaches progress UI | Persistence/reload/dispatch and TUI progress tests | Validated |
| Agent context loading | Only explicit accessible files are attached; directories are rejected before dispatch | Positive file and negative directory tests | Validated |
| Image preflight | Embedded image inputs are resized below 30,000 32-by-32 patches before the first provider request | Message, function-output, and image-generation replay tests | Validated |
| Temporary artifacts | Product-owned spill files use a marked session root with bounded cleanup; failed plugin sync staging is RAII-owned | Artifact path, retention, exec spill, and plugin failure tests | Validated for product-owned transient producers |
| Project placement | No new project root is introduced; the verified executable is installed under `/var/home/jack/bin` only after backup | Final build/deployment evidence | Validated |

<!-- markdownlint-enable MD013 -->

## Frozen Runtime Evidence

### User submission persistence

Session `ccb698d4-e83d-44a7-b903-df4c5e31a713` records each reported operator
message once in `/var/home/jack/.code/history.jsonl` and once as a rollout user
`response_item`. The repeated delivery therefore does not originate from two
keyboard submissions or two history records.

### Cross-session tool-output duplication

The comparison below was generated from the rollout JSONL files, grouping
`function_call_output` records by `call_id`:

<!-- markdownlint-disable MD013 -->

| Session | Output records | Unique call IDs | Duplicated call IDs |
| --- | ---: | ---: | ---: |
| `84458d1a-34b0-48ae-905f-c5ea8477e892` | 9,475 | 4,738 | 4,737 |
| `ccb698d4-e83d-44a7-b903-df4c5e31a713` | 530 | 265 | 265 |

<!-- markdownlint-enable MD013 -->

The short control session received the operator's test message once, but it
still duplicated nearly every tool output. The difference is that the control
message arrived as an ordinary idle-turn input, while the affected messages in
the active session arrived through `Op::QueueUserInput`.

### Automated RED reproduction

`code-rs/core/tests/operator_input_delivery.rs` now contains
`queued_operator_input_and_tool_output_reach_each_model_request_once`.

The test performs this sequence:

1. Submit an ordinary user turn.
2. Receive a shell tool call and wait for `ExecCommandBegin`.
3. Submit `Op::QueueUserInput` while the tool is still running.
4. Capture the immediate follow-up model request.
5. Submit another ordinary turn and capture its model request.
6. Require one queued user item and one output for `call-1` in both requests.

Unfixed result on September 18, 2026:

```text
single-tool requests: [(request 2, queued 2, output 1),
                       (request 3, queued 1, output 1)]
persisted outputs for call-1: 2

multi-tool request 2: queued 2, output call-1 1, output call-2 1
persisted outputs: call-1 2, call-2 2

review request queued copies: 1
later normal request queued copies: 1
```

The earlier run with compression enabled first proved the test setup needed to
disable deterministic input rewriting before counting exact text. After doing
so, the tests failed for the intended ownership violations. The request body
shows one tool output because `Prompt::get_formatted_input` filters duplicate
`FunctionCallOutput` values by `call_id` immediately before serialization. That
defensive formatter does not repair conversation history, rollout persistence,
or compaction input.

## Current Ownership Map

```text
TUI submit
  -> Op::QueueUserInput
  -> Session.pending_user_input
  -> run_agent drains queued and internal pending items
  -> queued item is recorded into conversation history
  -> same queued item is appended again as turn_input extra
  -> provider request sees two user copies

provider tool result
  -> run_agent records function call + output into history
  -> same output is copied into Session.pending_input
  -> next iteration records pending output into history again
  -> same output is appended again as turn_input extra
  -> rollout and pre-format history contain duplicate outputs
  -> Prompt::get_formatted_input hides extra output copies from the wire request
```

Relevant code:

- `code-rs/core/src/codex/streaming/submission/mod.rs`: receives
  `Op::QueueUserInput` and chooses running-task queue versus immediate turn.
- `code-rs/core/src/codex/session.rs`: owns `pending_input`,
  `pending_user_input`, queue draining, history recording, and full request
  assembly.
- `code-rs/core/src/codex/streaming/agent/run.rs`: drains queues, invokes hooks,
  records items, constructs provider input, requeues tool responses, compacts,
  and starts any remaining queued turn.
- `code-rs/core/src/codex/streaming/turn/mod.rs`: executes one provider attempt
  and its returned tool calls.
- `code-rs/core/src/codex/compact.rs`: local compaction, sanitization, orphan
  pruning, and emergency history replacement.
- `code-rs/core/src/codex/compact_remote.rs`: remote compaction retry, trimming,
  local fallback selection, and emergency fallback.

## Resolved Contract Contradiction

The former `Session::get_pending_input_filtered(false)` path cloned queued user
input into review requests while also preserving it for a later normal turn.
The repair replaces that ambiguous API with `drain_pending_input`, which drains
only internal model-visible context. Queued operator submissions retain their
separate owner until an eligible normal-turn boundary. The review-mode
integration regression now proves that review neither sends nor consumes the
queued submission.

## Root-Cause Hypotheses

### Confirmed

1. Conversation history and request extras both claim drained queued user input.
2. Tool output is recorded when produced and then recorded again when drained
   from `pending_input`.
3. Request assembly adds the drained tail even after history already contains
   it.
4. Compaction sanitization truncates items and removes orphaned outputs, but it
   does not remove duplicate outputs sharing a valid call ID.
5. The review-mode filtered queue behavior contradicts its comments and caller
   expectations.
6. Outbound `FunctionCallOutput` de-duplication is a late safety net that masks
   the persisted duplication from request-capture tests unless rollout state is
   checked independently.

### Strong but not yet fully proven

1. Reported multi-tool interrupt latency occurs because queue draining happens
   only around the outer agent loop, while one provider response may execute
   several tool calls before the next provider request.
2. Long-session cutoffs and high remote-compaction failure rates are amplified
   by duplicate tool outputs and oversized context attachments.
3. Emergency compaction's replacement history prevents a hard crash but loses
   useful working context, making it unsuitable as routine recovery.

### Separate audits required

1. Agent settings have multiple possible owners: generated defaults, persisted
   config, TUI editing state, session snapshots, tool-schema allowlists, and
   dispatch arguments.
2. Agent launch context can become unbounded when a directory is accepted where
   explicit files were intended.
3. Temporary files have multiple producers and no proven global retention
   policy. The ghost-index `$HOME/tmp` repair covers only one producer.
4. Project and generated-output paths require validation against the dedicated
   `/var/home/jack/projects`, `/var/home/jack/projects/Mcp_project`,
   `/var/home/jack/bin`, and `/var/home/jack/appimages` roots.

## Pending-Input Producer Classification

<!-- markdownlint-disable MD013 -->

| Producer | Current path | Intended ownership |
| --- | --- | --- |
| Tool execution result | `run.rs` -> `add_pending_input` | Already durable beside its call; use only as a loop-continuation signal, never queue the payload again |
| `image_view` attachment | `image_view.rs` -> `add_pending_input` | New model-visible item; record once before the next request |
| Background developer context | `Op::AddPendingInputDeveloper` -> `enqueue_out_of_turn_item_while_running` | New model-visible item; record once while the active turn can still consume it |
| Agent-completion wake | `enqueue_agent_completion_wake` -> `enqueue_out_of_turn_item_while_running` | New model-visible item; record once, guarded by batch identity |
| Manual compact request | `Op::Compact` -> `inject_input` plus `enqueue_manual_compact` | Control request, not ordinary conversation content; audit separately before changing its semantics |
| Queued operator submission | `pending_user_input` | Keep separate until an eligible non-review boundary, run hooks once, record once, then consume |
| `state::TurnState.pending_input` | `state/turn.rs` | Unused scaffolding in the active session path; do not create a second owner during this repair |

<!-- markdownlint-enable MD013 -->

## Target Ownership Model

The implementation must satisfy these transitions without generic text
deduplication:

```text
operator submission
  received -> queued once -> hook checked once -> persisted once
  -> visible in first eligible provider request once -> consumed

tool result
  produced -> persisted with its call once
  -> visible in next provider request through history once
  -> never re-recorded as pending input

request-only context
  produced -> attached to one provider request -> discarded
  -> never written into durable conversation history
```

Likely implementation direction, pending complete call-site validation:

- Keep queued operator submissions separate until an eligible boundary.
- Persist an accepted queued submission before request construction.
- Build ordinary provider input from the canonical history, not from history
  plus a second copy of the same newly persisted items.
- Use tool-response presence as a continuation signal; do not requeue an output
  already stored in history merely to cause the next iteration.
- Give genuinely request-only context an explicit path rather than overloading
  `pending_input`.
- Preserve submission IDs; never collapse separate submissions containing equal
  text.

## Image Preflight Boundary

Provider-bound image normalization now happens before the first request rather
than after a provider rejection:

- `Prompt::get_formatted_input` normalizes base64 image data URLs in ordinary
  messages, status items, and function-call outputs.
- `rewrite_image_generation_calls_for_input` normalizes persisted image
  generation results at the later replay-conversion boundary.
- Remote URLs remain unchanged. Invalid embedded data URLs are retained and
  logged rather than destroying the request.
- The shared image utility preserves in-bounds PNG/JPEG bytes and resizes larger
  images to at most 2048 by 768. At 32-by-32 provider patches, that upper bound
  is 1,536 patches, below the reported 30,000-patch rejection threshold.

RED/GREEN coverage includes ordinary messages, function outputs, image
generation replay, malformed embedded payloads, and remote URLs.

## Storage Producer Audit

The live audit separated product-owned transient data from operator-owned or
diagnostic evidence. No existing `.code` data was deleted.

<!-- markdownlint-disable MD013 -->

| Root observed before deployment | Approximate size | Producer and owner | Lifecycle decision |
| --- | ---: | --- | --- |
| `/var/home/jack/.code/agents` | 3.8 GB | Legacy oversized exec/progress/result spills rooted from process `cwd` | Future writes redirected; legacy tree left untouched because it lacks trustworthy ownership markers |
| `/var/home/jack/.code/sessions` | 1.6 GB | Rollout recorder | Existing startup housekeeping retains seven days by default |
| `/var/home/jack/.code/backups` | 926 MB | No active runtime producer found in the audited Rust paths; includes operator binary backups | Treated as operator-owned and never auto-deleted |
| `/var/home/jack/.code/debug_logs` | 406 MB | Provider request/response and tracing diagnostics | Preserved as explicit diagnostic evidence; no automatic deletion added |
| `/var/home/jack/.code/.tmp` | 106 MB | Activated plugin cache plus failed plugin staging clones | Activated cache remains; future failed `plugins-clone-*` staging directories self-clean through `TempDir` ownership; existing stale clone left untouched |
| `/var/home/jack/.code/diagnostics` | 13 MB | Operator-generated diagnostic captures | Treated as operator-owned and never auto-deleted |
| `/var/home/jack/.code/cache` | 12 MB | Provider/model caches | Existing TTL and atomic replacement policies retained |
| `code_home/artifacts/agents/<session UUID>` | New root | Oversized user, exec, progress, and agent-result spill files | Session-scoped `.code-owned.json` marker; startup cleanup removes only valid marked roots older than seven days |

<!-- markdownlint-enable MD013 -->

Test-only `tempfile` use remains process-scoped. The ghost-commit temporary Git
index remains under `code_home/tmp` when a home root is available. Product
cleanup deliberately refuses malformed markers, unmarked directories, legacy
artifact trees, backups, debug logs, and diagnostics.

## Implementation Phases

### Phase 0: Complete the evidence map

- [x] Compare history and rollout persistence for affected user messages.
- [x] Compare function output duplication across two independent sessions.
- [x] Stop obsolete full build before editing runtime source.
- [x] Add and run the immediate-plus-later request RED test.
- [x] Capture the exact number and order of queued input and function outputs in
  model requests and flushed rollout JSONL.
- [x] Inspect all `pending_input` producers and classify their ownership.
- [x] Inspect multi-tool execution and lock the first post-batch provider
  boundary with a RED regression.
- [x] Add and run a review-mode queued-input RED test.

### Phase 1: Repair canonical turn ownership

- [x] Remove duplicate insertion at its source rather than filtering equal text.
- [x] Ensure tool call/output pairs remain structurally valid for Responses and
  Chat wire APIs.
- [x] Ensure queued user hooks run once and blocked input is neither sent nor
  replayed.
- [x] Ensure queued items arriving after the final drain do not start a phantom
  model turn.
- [x] Ensure separate submissions with identical text each remain once by using
  submission identity rather than text equality.
- [x] Run operator input, lifecycle hook, and completion-wake tests.

### Phase 2: Harden compaction and long-session behavior

- [x] Add duplicate-call/output sanitization before local and remote compaction.
- [x] Classify remote service/transport failures for local fallback while
  preserving explicit account, usage-limit, rate-limit, and interruption
  failures.
- [x] Define when local compaction is safe fallback versus when the active turn
  must remain unchanged with an actionable error.
- [x] Bound emergency fallback and preserve recent user/assistant context rather
  than replacing history with only a generic warning.
- [x] Re-run the complete long-history rendering regression suite from the
  August 23 plan during the final verification gate.
  - The later recovery audit ran all seven active history-cutoff regressions on
    the current branch; the separate diagnostic scan remains intentionally
    ignored.

### Phase 3: Repair agent configuration ownership

- [x] Trace generated defaults -> config load -> TUI edit -> persistence ->
  active session refresh -> tool schema -> dispatch arguments.
- [x] Add independent session, review, and auto-drive role persistence tests.
- [x] Prove persisted aliases reload into canonical runtime dispatch with the
  configured command, arguments, instructions, and roles.
- [x] Surface effective execution identity through agent progress consumed by
  the TUI.
- [x] Propagate the current edited agent catalog through live
  `ConfigureSession` operations.
- [x] Reject directory-wide context attachment and accept regular files.

### Phase 4: Repair filesystem lifecycle

- [x] Inventory persistent high-growth roots and distinguish product-owned
  transient data from operator-owned diagnostics and backups.
- [x] Record root, naming, owner, cleanup trigger, retention limit, and crash
  recovery for each producer.
- [x] Keep repair build/test data under `/var/home/jack/tmp/code-build`.
- [x] Add startup cleanup only where ownership is unambiguous; do not
  delete unknown operator files.
- [x] Move oversized user, exec, progress, and agent-result spills to
  `code_home/artifacts/agents/<session UUID>/...` with an ownership marker and
  seven-day default retention.
- [x] Preserve legacy `.code/agents`, backups, diagnostics, and debug logs
  untouched during this repair.
- [x] Make failed plugin marketplace staging clones self-clean through RAII;
  existing stale clones are intentionally not deleted.
- [x] Validate final executable placement under `/var/home/jack/bin` during
  deployment.

### Phase 5: Verification and deployment

- [x] Run focused RED/GREEN tests with one compiler worker.
- [x] Run affected focused core, TUI, compaction, agent, git-tooling, and
  provider tests.
- [x] Stop the additional broad crate/TUI matrix after the operator requested no
  more disproportionate testing; retain focused regression and root-build
  evidence instead.
- [x] Run `git diff --check` on all touched paths.
- [x] Run `CARGO_BUILD_JOBS=1 ./build-fast.sh` with home-backed temporary paths.
- [x] Back up `/var/home/jack/bin/code` with a timestamp.
- [x] Atomically install the verified binary.
- [x] Report that the active process still requires restart before the repair is
  exercised live.

## Verification Commands

All commands run from `code-rs` unless noted otherwise and include:

```text
TMPDIR=/var/home/jack/tmp/code-build
TMP=/var/home/jack/tmp/code-build
TEMP=/var/home/jack/tmp/code-build
GIT_CEILING_DIRECTORIES=/var/home/jack/tmp/code-build
CARGO_BUILD_JOBS=1
```

Focused queue regression:

<!-- markdownlint-disable MD013 -->

```text
cargo test -p code-core --test operator_input_delivery queued_operator_input_and_tool_output_reach_each_model_request_once -- --nocapture
```

<!-- markdownlint-enable MD013 -->

Required final build from the repository root:

```text
./build-fast.sh
```

## Live Status Log

- **September 18, 2026:** Two prior provider/keyring failures and the ghost
  snapshot temp-root failure were fixed and focused tests passed.
- **September 18, 2026:** The pre-fix full build was stopped because new runtime
  findings made its output obsolete.
- **September 18, 2026:** Cross-session JSONL analysis proved systemic duplicate
  tool-output recording and single-copy user persistence.
- **September 18, 2026:** The first exactly-once queued-input integration test
  failed with two model-bound copies, establishing RED before production edits.
- **September 18, 2026:** Flushed integration rollouts reproduced two persisted
  outputs per tool call while outbound request formatting reduced them to one.
- **September 18, 2026:** Review-mode and multi-tool regressions failed with the
  expected queued-input leakage/duplication, completing Phase 0 evidence locks.
- **September 18, 2026:** A directory-scoped read-agent audit was cancelled due
  excessive context/latency risk. Remaining investigation is inline and
  file-bounded.
- **September 19, 2026:** Canonical queue ownership regressions passed: four
  operator-input delivery tests and two post-final completion-wake tests.
- **September 19, 2026:** Compaction regressions passed for duplicate output
  sanitization, remote-service local fallback classification, and bounded
  emergency retention of recent user and assistant context.
- **September 19, 2026:** Agent persistence/reload/dispatch, explicit-file
  attachment boundaries, live catalog propagation, and effective execution
  progress tests passed.
- **September 19, 2026:** Embedded message, function-output, and image-generation
  replay payloads were proven RED before preflight resizing and GREEN afterward;
  the processed images remain below 30,000 32-by-32 patches.
- **September 19, 2026:** The storage audit identified 3.8 GB of legacy
  unmarked agent spill files and a 51 MB leaked plugin staging clone. Future
  spills are marked and retained for seven days; future failed plugin staging
  attempts self-clean. Existing user data was not deleted.
- **September 19, 2026:** The required one-threaded `./build-fast.sh` completed
  successfully. The candidate SHA-256 was
  `ca5b936c598c16c5bb94af5b6774060fa5d043fb42124d0c8d524ffd74c21a4d`.
- **September 19, 2026:** The previous `/var/home/jack/bin/code` was preserved as
  `/var/home/jack/bin/code.backup-20260919T013449Z`; the verified candidate was
  atomically installed and reported `code 0.153.4`. Restart remains required.
