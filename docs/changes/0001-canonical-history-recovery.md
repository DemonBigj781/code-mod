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
- [x] Trace remote `/responses/compact` failure paths and preserve the exact
      error instead of silently treating summary/context clearing as equivalent.
  - Fallback-eligible endpoint and service failures now emit the original
    status/body before local summarization, while authentication and rate-limit
    failures remain failures instead of being disguised as fallback success.
- [ ] Separate deterministic operator-input compression, remote compaction, and
      local summary fallback in code, configuration, telemetry, and tests.
- [ ] Reproduce and fix response truncation or one-character output after long
      conversations.
  - [x] The bounded long-transcript rendering probe passes all seven active
        cutoff regressions; its one diagnostic scan remains intentionally
        ignored. This rules out rendered-history tail loss but does not yet
        prove provider-stream completion.
- [ ] Verify resume after fallback does not duplicate, omit, or reorder operator
      input, tool output, or assistant content.

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

## Repository recovery and cleanup

- [x] Inventory the exact checkout, branch tips, registered worktrees, stale
      copied worktree, remotes, and deployment artifact.
- [x] Preserve Git metadata, reflogs, unreachable objects, cleanup targets, and
      the deployment snapshot in a checksummed archive.
- [x] Reopen this falsely completed recovery record and remove Git ancestry as
      the asserted source of historical truth.
- [ ] Reconcile every required source change onto `main` using the session
      ledger and source/test behavior, not commit ancestry as provenance.
- [x] Build and test the currently repaired source with exactly one compiler
      thread.
  - `CARGO_BUILD_JOBS=1 cargo build -j1 -p code-cli --bin code` passed, as did
    the focused regression suites recorded above; the latest incremental build
    of checkpoint `a942ed771` completed in 6 minutes 4 seconds.
- [x] Verify the deployed executable separately from compilation and tests.
  - `/var/home/jack/bin/code` matches the built candidate SHA-256
    `a6d53577f65ea9b270b94c1cf8112f8e6dc69131f4ec5c783e7fbc1d2797a070`;
    `--version`, generated Bash completion syntax, and `doctor` pass.
  - The immediately replaced binary is preserved at
    `/var/home/jack/backups/code-installed-predeploy-20260920T075659Z/code`
    with SHA-256
    `afbdf29e046ebf131523d6e404b3922222764195099b555ce0c79d529c51dd94`;
    the earlier installed binaries remain in the timestamped `064835Z`,
    `052810Z`, and `043514Z` backups.
- [x] Remove the stale copied worktree and obsolete binary snapshot from the
      project directory after their archive is re-verified.
  - The recovery archive again passed its recorded SHA-256 and `zstd -t` before
    `code-ci-snapshot-BAK` and `code-update-20260909` were removed.
- [x] Rename the surviving branch to `main` and delete every other local
      branch only after reachability and content checks pass.
  - The runtime repair commit is `f02f45d3e0e041a0cada90f0bdc37e6286567368`;
    every
    removed pre-repair branch tip remains listed in the backup manifest.
- [ ] Delete obsolete remote branches and make remote `main` match the verified
      result after GitHub authentication is restored.
- [x] Verify one `main` branch, one registered worktree, one Code source
      checkout in active project/temp locations, and a clean worktree.
  - `scripts/verify-repository-layout.sh --local` passes.

## Current blockers

The configured GitHub token and stored account token are invalid. Local recovery,
testing, and cleanup can continue, but remote branch deletion, fork-metadata
correction, and the final push require authentication to be restored.

## Acceptance gates

Recovery is complete only when all product and repository tasks above are
verified, `scripts/verify-repository-layout.sh --local` passes, the one-thread
build and relevant test suites pass, the installed executable is verified
separately, and this ledger contains no historical claims derived only from Git.
