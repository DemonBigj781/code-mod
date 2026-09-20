# Code Recovery and Repository Normalization

**Status:** in-progress

## Objective

Recover the product from the failed September repair, correct the runtime defects
identified across prior sessions, and leave one checkout at
`/var/home/jack/projects/AI_Project/code` with one branch named `main` and no
linked or copied worktrees in the project area.

This document is the single recovery ledger. Do not create parallel recovery
plans for the same work.

## Evidence authority and boundary

Historical claims in this recovery come only from the read-only prior-sessions
MCP corpus spanning Code and Codex. The current Git graph, GitHub fork labels,
commit messages, repository names, generated documentation, and binary snapshots
are recovery artifacts; they do not establish provenance.

The manually exported OpenAI account archive is older than the currently
connected corpus but has not been connected yet. Its future import may extend or
correct the earliest chronology. Until then, the ledger must distinguish
confirmed session evidence from unknown history.

GitHub currently labels `DemonBigj781/code-mod` as a fork of
`just-every/code`. That live fork-network metadata is known to be historically
wrong and remains non-authoritative; the source remote for the actual Code Mod
codebase is recorded separately as `immateria/codex-mod`.

## Confirmed chronology

1. In `code:9f863d9b-ed61-401a-bf5f-598912c21231` on 2026-07-31, the
   operator identified `code-elf` as a compiled output of `code-mod` and
   described the source lineage as the operator's fork of `code-mod`, itself a
   fork of `just-every/code`, which was a fork of Codex.
2. Development and deployment continued for months across Code and Codex
   sessions. The current repository was not the start of the project.
3. In `codex:019ffbfe-e90d-72c2-a00a-3bb1ea1d6a10` on 2026-08-13, a
   consolidation attempted to reconstruct history from Git ancestry, created
   the former “canonical history” guard, and marked recovery complete. That
   method is invalid because the repository metadata was itself created and
   rewritten to satisfy tooling and did not represent the real source history.
4. August and September Code sessions repeatedly report delayed or duplicate
   operator input, late events, response truncation, long-session slowdown,
   broken model/agent settings, and unreliable compaction.
5. In `code:ccb698d4-e83d-44a7-b903-df4c5e31a713` on 2026-09-18, the
   operator reported that remote compaction frequently breaks and falls back to
   a session summary plus context clearing, and that read agents are slow and
   failure-prone.
6. In `code:b28bed46-eb53-448a-aeed-61fdc367be55` on 2026-09-19, the
   most recent repair added regression coverage and part of a shared
   operator-input inbox, but left a failing multi-tool interruption test and did
   not complete source integration, repository cleanup, or deployment.

## Pre-repair preservation

Before new edits, the current local artifact state was archived at
`/var/home/jack/backups/code-pre-repair-20260920T012638Z`. Its manifest records
the branch tips, remotes, stale worktree pointer, and evidence boundary. The
archive's verified SHA-256 is
`419e7375e7302c29082c4f4859e01658f29621031b80e9b5ab2c6b663f7b0da5`.

The pre-repair checkout was clean at
`300e231b28e5ef89dc67bd511c2beff28cf692b2` on
`feat/codex-0.153.4-gpt6`, with ten local branches. The stale copied worktree
`code-ci-snapshot-BAK` and the verified `code-update-20260909` binary snapshot
were both included in the archive.

## Product repair coverage

### Operator input and event ownership

- [x] Reproduce the lost second tool-output failure with an automated regression.
- [x] Route every finalized tool call through the common scheduler before
      execution, independent of model-family parallel-tool support.
- [x] Verify all eight `operator_input_delivery` integration tests with one test
      thread.
- [x] Audit model/session reconfiguration so queued input and partial responses
      retain one owner.
  - Settings rebuilds are deferred and coalesced while a task owns the session,
    recheck idleness before replacement, and acknowledge the actual settings
    submission instead of the startup ID.
  - The integration regression proves the active turn completes without
    `TurnAborted`, remains in replacement-session history, and the next request
    uses the new model. The pending-input handoff unit test also passes.
- [x] Audit late post-final tool/search events and stale layout-height gaps.
  - Submission ownership rejects same-turn late search/exec events, preserves
    events owned by other submissions, and spacer/scroll-position regressions
    pass under bounded VT100 tests.
- [ ] Verify prompt-history scrolling and view scrolling independently in SSH
      and Decky Terminal contexts without controlling a game.

### Long-session and compaction reliability

- [ ] Reproduce the long-session send/receive/processing slowdown with bounded
      instrumentation and identify the growing data structures or repeated work.
  - [x] Stop rebuilding completion notifications from every historical operator
        message on every provider iteration; notifications now consume only the
        submissions accepted for the current agent run.
  - [x] Stop constructing an unbounded diagnostic rendering of the entire
        conversation at every provider boundary when compact tracing is off.
        Explicit `CODEX_COMPACT_TRACE` diagnostics now inspect at most the most
        recent 32 items, emit at most 8 KiB, and report payload sizes without
        copying tool-output bodies.
  - [x] Stop recomputing deterministic operator-input compression for every
        historical user message on every normal provider iteration. A
        per-session cache now reuses results by immutable submission ID,
        content slot, and standard/aggressive mode; it is capped at 16,384
        submissions or 32 MiB of compressed text, stores no duplicate text for
        unchanged/protected inputs, and resets when history is replaced. All
        eight compression unit tests and all ten operator-input integration
        tests pass with one test thread.
  - [x] Add bounded phase timing at turn schedule, stream, completion, and
        failure boundaries. `codex.turn_latency` records phase duration and
        inter-phase gap alongside pending queue counts, prompt item/status
        counts, and token usage; debug mode also writes session-local JSONL.
        A live long-session capture from the newly installed binary still
        requires the operator to restart Code and reproduce the slowdown.
- [x] Trace remote `/responses/compact` failure paths and preserve the exact
      error instead of silently treating summary/context clearing as equivalent.
  - Fallback-eligible endpoint and service failures now emit the original
    status/body before local summarization, while authentication and rate-limit
    failures remain failures instead of being disguised as fallback success.
- [x] Separate deterministic operator-input compression, remote compaction, and
      local summary fallback in code, configuration, telemetry, and tests.
  - Deterministic input compression has its own persisted settings and tests,
    retains original operator text in the rollout, and now has bounded
    session-local reuse. Remote compaction and local emergency summary behavior
    remain separate recovery paths. `codex.context_management` now records
    distinct input-compression, remote-compaction, local-summary, and emergency
    paths with explicit outcomes, durations, bounded counts, and preserved
    failure detail. Debug mode writes the same payloads to session-scoped
    `context_management` JSONL. The schema test, nine compression tests,
    session-log regression, 13 local-compaction tests, and two remote-fallback
    tests pass with one test thread. Live reproduction remains part of the
    separate long-session gate above.
- [x] Reproduce and fix the display-side form of long-session response decay
      that the operator called "late prompt text entropy."
  - Prior-session evidence records the operator first identifying a broken
    display and asking for the answer to be printed again, then reporting that
    long-session responses appeared to decay below a couple of characters.
    The same dated diagnostic window repeatedly reports stale reasoning-cell
    heights with nonzero cached rows and zero recomputed rows.
  - Height reconciliation now replaces the stale cache entry, invalidates
    prefix sums and append-only assumptions, schedules virtualization rebuild,
    and requests a redraw. The bounded long-transcript rendering probe passes
    all seven active cutoff regressions; its one diagnostic scan remains
    intentionally ignored.
- [x] Reproduce provider-side response truncation independently of the repaired
      display path and verify bounded recovery for the failure classes present
      in the historical evidence.
  - Two full conversation regressions now begin with a one-character assistant
    delta. One ends in typed `response.incomplete`; the other feeds a malformed
    event and closes before `response.completed`, matching the historical
    WebSocket deserialization-failure class. Both retry once, carry the fragment
    only in a bounded `[EPHEMERAL:RETRY_HINT]`, finalize the complete replacement
    response, and prove on the following turn that neither the fragment nor the
    retry hint entered retained history. Both tests pass with one test thread.
  - The affected 2026-08-18 log contains one WebSocket deserialization failure
    at 14:02, no recorded typed incomplete response or exhausted retry, and then
    the independently repaired stale height-cache flood beginning at 15:52.
    Live ChatGPT transport confirmation after process restart remains a runtime
    acceptance check, not an unverified source-code claim.
- [x] Verify resume after fallback does not duplicate, omit, or reorder operator
      input or tool output.
  - Test-first regressions reproduced two source defects: all recovered calls
    were grouped ahead of their outputs, reordering queued operator input, and
    a pending output was retained even when rebuilt history already owned it.
    Reconciliation now returns one chronological tail, inserts each missing
    call immediately before its output, preserves intervening queued input, and
    tracks rebuilt output ownership. All nine attempt-recovery tool-output
    tests and all ten operator-input integration tests pass with one test
    thread; assistant partial/final ownership is covered by the two provider
    retry regressions above.

### Agent, model, provider, and navigation settings

- [x] Make operator settings the sole owner of read-agent enablement, model
      selection, and count; the assistant may submit only a task.
  - The public create schema and both deserializers now accept only `task`;
    model-supplied names, context, attachments, output goals, model lists, and
    permission overrides are rejected. Three focused contract tests pass.
- [x] Add and verify a master read-agent disable switch; disabled models must
      never be selected.
  - The agent tool schema rejects model and permission overrides, the launcher
    never substitutes an unconfigured fallback agent, and `subagents.enabled`
    is persisted from the Settings > Agents “Read agents” row.
- [x] Preserve role instructions, conversation, and task context for alternative
      agents at the launcher boundary.
  - Each launch now receives configured per-agent instructions, the task as a
    separate prompt, and an automatic parent-session handoff containing bounded
    base instructions, operator/project instructions, working directory, and
    recent user/assistant conversation. The handoff is capped at 64 KiB and
    prioritizes the newest conversation; both focused regressions pass.
  - This verifies the common launch contract. Runtime interoperability for each
    external Claude, Copilot, and Gemini CLI remains an explicit integration
    check rather than an inferred result.
- [x] Serialize `config.toml` read-modify-write settings transactions and surface
      agent-save failures in the TUI instead of discarding them.
  - The deterministic concurrent-agent-update regression previously lost one
    entry and now passes; all 38 `config_edit` tests and both affected TUI agent
    editor tests pass with one test thread.
- [x] Repair scrolling, alignment, and independent role toggles in the agent
      model settings UI.
  - The prior checkbox was not supported by the actual overview renderer: it
    always rendered from content row zero, mouse hit-testing ignored viewport
    position, and the model-name cell opened the editor instead of controlling
    the complete model entry.
  - The overview now derives rendering and mouse hit-testing from the same
    bounded scroll offset, keeps the selected row visible, and exposes the model
    name as a separate keyboard and mouse target. Selecting that target changes
    Session, Sub-agent, Review, and Auto Drive together through one persisted
    configuration transaction; individual role columns remain independently
    selectable, and `E` opens model configuration.
  - The original bounded-viewport regression failed with model 11 selected while
    models 0-2 remained visible. The repaired 11-test overview module, atomic
    all-role persistence regression, existing single-role regression, and
    direct-provider role regression all pass with one test thread.
- [x] Verify unified selectors for general, agent, subagent, review, and
      autodrive roles, including persistent settings.
- [x] Repair irreversible provider/model switching and distinguish free versus
      paid OpenRouter routes before dispatch.
  - Provider transitions now preserve one complete provider/profile snapshot;
    the previously failing direct-provider → OpenRouter → ordinary sequence and
    the full 27-test provider group pass.
- [x] Verify Stable Horde and other configured providers are discoverable in the
      TUI.
- [x] Verify multiline input and navigation bindings, including Shift+Enter.

## Earlier repair coverage retained

- [x] Retain the Colibri built-in OpenAI-compatible provider, `/v1/models`
      discovery classification, and independent-server documentation without
      changing the host's swap or zram configuration. The current branch passes
      all 14 core provider tests and all four direct-provider TUI tests.
- [x] Keep ghost-commit snapshot tempdirs on the home-backed temporary root
      when available; `ghost_index_tempdir_uses_home_tmp_when_available` passes.
- [x] Resize oversized embedded images before the first provider request and in
      image-generation replay. The utility downscale test, three formatted-input
      tests, and replay regression all pass.
- [x] Keep generated agent artifacts session-scoped and delete only marked
      product-owned sessions outside the retention window. Both ownership and
      retention regressions pass.
- [x] Remove failed plugin marketplace staging clones through scoped ownership;
      `failed_marketplace_sync_removes_staging_clone` passes.

## Repository recovery and cleanup

- [x] Inventory the exact checkout, branch tips, registered worktrees, stale
      copied worktree, remotes, and deployment artifact.
- [x] Preserve Git metadata, reflogs, unreachable objects, cleanup targets, and
      the deployment snapshot in a checksummed archive.
- [x] Reopen this falsely completed recovery record and remove Git ancestry as
      the asserted source of historical truth.
- [x] Reconcile every required source change onto `main` using the session
      ledger and source/test behavior, not commit ancestry as provenance.
  - The audit cross-checked authoritative prior sessions
    `code:ccb698d4-e83d-44a7-b903-df4c5e31a713` and
    `code:b28bed46-eb53-448a-aeed-61fdc367be55` against current source and
    focused tests. Remaining manual-runtime and remote-authentication gates are
    tracked separately below; they are not missing source changes.
- [x] Build and test the currently repaired source with exactly one compiler
      thread.
  - `CARGO_BUILD_JOBS=1 cargo build -j1 -p code-cli --bin code` passed, as did
    the focused regression suites recorded above. The latest full build of
    context-telemetry checkpoint `2562ab30a` completed in 9 minutes 14 seconds;
    the preceding agent-overview checkpoint `0678deda9` completed in 4 minutes
    00 seconds.
  - The final guarded push built the `dev-fast` CLI, passed its curated smokes,
    and ran the workspace with `CARGO_BUILD_JOBS=1`,
    `NEXTEST_TEST_THREADS=1`, and `RUST_TEST_THREADS=1`: all 3,048 tests passed
    and 9 were skipped. `NO_COLOR` was cleared for the run because the host's
    exported value suppresses the ANSI sequences exercised by two vendored
    Crossterm parser tests.
  - Commit `1a07d0dca` closes failures that the emergency repair had left past
    its release gate: stale config and app-server protocol schemas, pairing that
    updated only its test instead of the production enable path, Shift+Enter
    setup, OpenRouter profile restoration, missing-agent role state, and the
    unreachable stale-reasoning-height repair. Focused regressions pass for each
    corrected source path; serial execution distinguishes the remaining
    full-suite resource contention from product failures.
  - The first full gate exposed one remaining production defect rather than a
    cleanup-only problem: an authenticated-account reconnect removed the old
    remote client but cancellation also suppressed its `ConnectionClosed`
    event. Commit `2e7350418` makes that cancellation-time notification
    nonblocking, preserving prompt saturated-queue shutdown. The reconnect and
    saturation regressions pass in both directions, all 108 app-server library
    tests pass, and the final full gate is green.
- [x] Verify the deployed executable separately from compilation and tests.
  - `/var/home/jack/bin/code` matches the built candidate SHA-256
    `376fb247eab5b476dad73a061c26cce482db54bc80df52dd51c08d84d108a863`;
    `--version`, generated Bash completion syntax, and `doctor` pass.
  - The immediately replaced binary is preserved at
    `/var/home/jack/backups/code-installed-predeploy-20260920T163525Z/code`
    with SHA-256
    `632a678f472b6868baaa94f69f3701447cd25ca9098e6265c34c715343fcbe49`.
  - The preceding repair binary is preserved at
    `/var/home/jack/backups/code-installed-predeploy-20260920T120250Z/code`
    with SHA-256
    `8523d0318ff8cf9ba95ad743456e55e107e8a13a3ea6ccc99ff841796d7d0299`;
    the earlier installed binaries remain in the timestamped `095939Z`,
    `092743Z`, `090550Z`, `075659Z`, `064835Z`, `052810Z`, and `043514Z`
    backups.
- [x] Remove the stale copied worktree and obsolete binary snapshot from the
      project directory after their archive is re-verified.
  - The recovery archive again passed its recorded SHA-256 and `zstd -t` before
    `code-ci-snapshot-BAK` and `code-update-20260909` were removed.
- [x] Rename the surviving branch to `main` and delete every other local
      branch only after reachability and content checks pass.
  - The runtime repair commit is `f02f45d3e0e041a0cada90f0bdc37e6286567368`;
    every
    removed pre-repair branch tip remains listed in the backup manifest.
- [x] Delete obsolete remote branches and make remote `main` match the verified
      result after GitHub authentication is restored.
  - GitHub authentication was restored. Remote branches
    `ci/automatic-build-artifacts` and `ci/development-snapshot-20260907` were
    deleted only after their recoverable tips were recorded in the preservation
    manifest as `0cffac3393931caf0e31cf712324d427cb59eea2` and
    `ed801c2275836697eaa433dbc22fc869ca4f5493` respectively.
  - The guarded push was a normal fast-forward from `84c6dde86` to
    `2e7350418`. A live `git ls-remote --heads origin` now returns only
    `refs/heads/main` at `2e73504182ab97577d34e82b657e6e44559513ea`.
- [x] Verify one `main` branch, one registered worktree, one Code source
      checkout in active project/temp locations, and a clean worktree.
  - `scripts/verify-repository-layout.sh --local` passes.

## Remaining manual and external gates

- The operator must verify prompt-history scrolling and view scrolling
  independently in SSH and Decky Terminal contexts.
- The operator must restart Code manually, reproduce the long-session slowdown,
  and then inspect the new session's `turn_latency` and `context_management`
  debug logs. This recovery did not restart the active Code process.
- GitHub still reports the historically wrong `just-every/code` fork parent.
  Correcting a repository's fork-network parent is an external GitHub metadata
  operation, not a branch, ref, or local-history cleanup; it must not be
  represented as repaired by the normalized `main` ref.

## Acceptance gates

Recovery is complete only when all product and repository tasks above are
verified, `scripts/verify-repository-layout.sh --local` passes, the one-thread
build and relevant test suites pass, the installed executable is verified
separately, and this ledger contains no historical claims derived only from Git.
