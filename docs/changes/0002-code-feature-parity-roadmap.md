# Code Feature Parity and Lost Work Recovery

**Status:** in-progress

## Objective

Recover and plan Code-mod features that were discussed in prior sessions or in
current operator direction but are not covered by the canonical recovery ledger.
This document is a forward feature-parity roadmap, not a replacement for
`docs/changes/0001-canonical-history-recovery.md`.

## Current execution status

2026-10-15: native `session_history` is implemented in commit `31b2ac3d5`;
the original source-gap descriptions below are the earlier audit snapshot.
The current next task is resource guarding for command/history execution.
Tab-isolation and AI-selected session-title changes are a separate verification
track and are not considered complete until their GitHub Actions run passes.
The separate Cosmopolitan environment update remains deferred; its pinned
revision is unchanged by this work.

## Evidence sources

- Prior-session history is authoritative for what was discussed. The key recent
  session is `codex:019ffbfe-e90d-72c2-a00a-3bb1ea1d6a10`, especially the
  2026-08-13 and 2026-08-14 operator turns about wrong base/fork lineage,
  missing OpenRouter/model/CPU/backend fixes, consolidation of scattered
  worktrees, and the need for a Code-mod resource guard.
- `docs/changes/0001-canonical-history-recovery.md` remains authoritative for
  completed September recovery work and its two remaining manual gates.
- Current operator direction in the 2026-10-10 session adds these explicit
  targets: tabs, command execution/command section support, NSFW detection before
  image view, upstream/OpenAI Codex update support, mobile remote development
  parity, and a `session_history` tool for prior sessions.
- Current OpenAI documentation confirms Codex Remote/mobile capabilities:
  mobile can start, steer, review, approve, and organize Codex work against
  connected hosts; Codex Cloud tasks can start/continue from desktop, web, or
  mobile; supported desktop Codex chats are accessible through the ChatGPT mobile
  Remote tab but are separate from normal web/mobile chat history.
  - Sources checked on 2026-10-10:
    <https://openai.com/index/work-with-codex-from-anywhere/>
    and
    <https://developers.openai.com/blog/mastering-codex-remote-for-engineering>.
  - OpenAI Help corroborates the ChatGPT Work/Codex separation and Remote tab
    behavior:
    <https://help.openai.com/en/articles/20001275-chatgpt-work-and-codex>.

## Historical findings

1. **Repository provenance and consolidation were historically confused.**
   Prior sessions say the correct base was `immateria/codex-mod`, while the live
   GitHub fork metadata points at `just-every/code` and is explicitly not
   authoritative. The operator objected to scattered worktrees/variants and
   wanted one source checkout under `/var/home/jack/projects/AI_Project/code`.
2. **Some feature work was merged into the current `code-rs` tree, but not all
   discussed feature surfaces are represented.** The existing recovery ledger
   records many completed repairs, including OpenRouter route preservation,
   agent/model settings, long-session recovery, compaction separation, and
   executable deployment. It does not cover the new mobile-remote parity,
   session-history tool, NSFW image gate, or a durable upstream-update workflow.
3. **Resource containment was discussed after a runaway/OOM incident.** The
   2026-08-14 history says Code-mod should kill a runaway child before it kills
   its parent. The repo now contains execution-limit/cgroup/watchdog code paths,
   but this must be re-verified as part of `session_history` development because
   history scanning can be memory-heavy.
4. **The active repo already contains an upstream merge workflow.**
   `.github/workflows/upstream-merge.yml` targets `openai/codex` by default and
   performs guarded selective merges. That is a foundation, not yet a complete
   product feature for keeping internal Codex-derived code updated safely.
5. **Existing `code-rs` has session/resume/history infrastructure but no exposed
    `session_history` tool.** Searches found resume discovery, app-server thread
    history protocol modules, chat history, memory extraction from session
    history, and current prior-session tooling outside the product. The missing
    feature is a first-class agent tool that searches/reads prior sessions with
    bounded output and privacy controls.

## Evidence map

- **OpenRouter and free-route model support**
  - Prior-session evidence: `code:9f863d9b-ed61-401a-bf5f-598912c21231`
    requested OpenRouter API integration, `*/*:Free` routing, fallback to the
    next free model on error, and warned that any paid packet is costly.
  - Current source status: `0001` already tracks and records completed provider
    and model-setting repairs. This roadmap should not duplicate those tasks.
  - Gap: future model/provider changes must preserve the free-vs-paid route
    boundary and explicit operator model ownership.
  - Proposed verification: rerun the provider/model-picker tests named by
    `0001`, plus a no-network unit test for free-route selection and paid-route
    refusal.

- **Read-agent configuration and context handoff**
  - Prior-session evidence: `code:ccb698d4-e83d-44a7-b903-df4c5e31a713` and
    `code:b28bed46-eb53-448a-aeed-61fdc367be55` report unchangeable read-agent
    settings, slow/failing read agents, model self-selection that should belong
    to the operator, and alternate agents starting with near-total amnesia.
  - Current source status: `0001` records completed repairs for operator-owned
    agent settings, the read-agent disable switch, agent handoff context, and
    settings UI scrolling/alignment.
  - Gap: runtime interoperability for external Claude, Copilot, and Gemini CLIs
    remains an explicit integration check in `0001`, not proven by source alone.
  - Proposed verification: keep this under `0001` unless new feature work changes
    the agent launch contract.

- **Delayed, duplicate, or reordered operator input**
  - Prior-session evidence: both September `code:` sessions report late user
    interrupts, duplicate input behavior, and responses arriving several tool
    uses after the operator sent them.
  - Current source status: `0001` records scheduler, ownership, and regression
    coverage for operator-input delivery.
  - Gap: prompt-history and view scrolling still need manual verification in SSH
    and Decky Terminal contexts.
  - Proposed verification: complete the two remaining `0001` manual gates before
    marking this roadmap complete.

- **Long-session slowdown and output cutoff**
  - Prior-session evidence: `code:ccb698d4-e83d-44a7-b903-df4c5e31a713` reports
    long discussions being more likely to cut off output and remote compaction
    often breaking. `code:b28bed46-eb53-448a-aeed-61fdc367be55` adds that
    responses slow down over time.
  - Current source status: `0001` records fixes for repeated completion rebuilds,
    unbounded compact tracing, deterministic input-compression recomputation,
    latency telemetry, compaction separation, display decay, provider truncation,
    and resume ordering.
  - Gap: live long-session capture still requires a restarted Code process and
    inspection of `turn_latency` and `context_management` logs.
  - Proposed verification: keep the live reproduction under `0001`; any
    `session_history` implementation must add separate large-history regression
    coverage before it can be accepted.

- **Temp and project-folder discipline**
  - Prior-session evidence: `code:ccb698d4-e83d-44a7-b903-df4c5e31a713` reports
    a full root `/tmp`, disabled snapshotting, and misplaced folders despite
    dedicated folder patterns.
  - Current source status: `0001` covers the recovery boundary and preservation
    archive. The current operator instructions also define project, MCP, binary,
    and AppImage folder locations.
  - Gap: this roadmap has no new temp-folder feature except ensuring future
    tasks do not regress the folder policy.
  - Proposed verification: source tasks that create artifacts must include a
    path-policy check and must not write new project work outside the documented
    project folders.

- **First-request image downscaling and new NSFW image gate**
  - Prior-session evidence: `code:ccb698d4-e83d-44a7-b903-df4c5e31a713` includes
    the provider error for an oversized image and the operator instruction to
    shrink the image before sending the first time.
  - Current source status: `code-rs/core/src/openai_tools/builtin_tools.rs`
    registers `image_view` directly, and a repo search found no `nsfw`, nudity,
    moderation, or classifier gate around image attachment.
  - Gap: NSFW detection before model-visible image attachment is a new 2026-10-10
    operator request, not a recovered main-legacy feature.
  - Proposed verification: add tests for direct `image_view`, local/pasted image
    attachments, generated-image replay paths, and first-request downscaling.

- **Bounded `session_history` tool**
  - Prior-session evidence: the current 2026-10-10 operator direction asks for a
    tool exposing prior sessions/history to the model. The historical sessions
    also show why this is needed: important project facts were lost across
    context and Git ancestry was non-authoritative.
  - Current source status: `code-rs/app-server-protocol/src/protocol/thread_history.rs`
    can build turns from persisted events, and CLI resume code can discover
    recent sessions. `code-rs/core/src/openai_tools/registry.rs` registers no
    `session_history` tool.
  - Gap: there is no first-class bounded list/search/read-session/read-turn tool
    with redaction, output caps, and citations to session and turn IDs.
  - Proposed verification: unit-test store listing, query filtering, byte caps,
    malformed rollout handling, redaction, and missing metadata. Add a resource
    regression that proves a large history scan cannot OOM the parent process.

- **Resource guard and runaway child containment**
  - Prior-session evidence: `codex:019ffbfe-e90d-72c2-a00a-3bb1ea1d6a10`
    reports the Code-mod runaway/OOM concern and the requirement to kill a
    runaway program before it kills its parent.
  - Current source status: `code-rs/core/src/cgroup.rs` defines cgroup limits,
    pids limits, a process-group fallback memory watchdog, and a regression that
    kills a 64 MiB Python child at a 32 MiB limit. Exec-limit settings pages also
    exist under `code-rs/tui/src/bottom_pane/settings_pages/exec_limits/`.
  - Gap: that evidence covers exec children, not necessarily future in-process
    history indexing or all nested session-history scan paths.
  - Proposed verification: run the existing watchdog test with one test thread,
    then add new tests for nested children and large/malformed history scans.

- **Tabs and bottom-pane panel parity**
  - Prior-session evidence: the current 2026-10-10 direction says tabs did not
    make it into active Code.
  - Current source status: legacy `codex-rs/tui/src/bottom_pane/selection_tabs.rs`
    and related tab-aware bottom-pane files exist. Active `code-rs/tui` has many
    settings and bottom-pane modules, but no matching `selection_tabs.rs` file.
  - Gap: the intended tab scope remains ambiguous: plugin tabs, settings tabs,
    multi-chat/session tabs, or all of those surfaces.
  - Proposed verification: before porting, write VT100/snapshot tests for the
    chosen tab surfaces and compare them against legacy behavior.

- **Command execution and AGENTS command-section support**
  - Prior-session evidence: the current 2026-10-10 direction says command
    execution or command-section support did not make it cleanly.
  - Current source status: active `code-rs` has shell, streamable shell, exec
    command, exec server, and exec policy crates. `code-rs/core/src/project_doc.rs`
    reads and injects AGENTS/project docs, but the inspected code does not show
    a dedicated parser for `[commands]` sections as executable command metadata.
  - Gap: execution primitives exist, but documented project command sections are
    not proven to become structured, safe, copyable command affordances.
  - Proposed verification: test both directions independently: project-doc
    command sections reach agent context, and model/tool execution honors the
    shell-profile, one-line command, and safety policies.

- **Upstream OpenAI Codex update support**
  - Prior-session evidence: `code:9f863d9b-ed61-401a-bf5f-598912c21231`
    records confusion between updating Codex and updating the embedded
    Code-mod-derived executable, and the operator said changes made to Codex had
    to be added to Code-mod.
  - Current source status: `.github/workflows/upstream-merge.yml` exists and
    targets `openai/codex`, with one-thread Cargo settings in the workflow.
  - Gap: the workflow is infrastructure, not yet a local, reviewable update
    product that summarizes crate impact and refuses mutation without an
    explicit command. Its scheduled automation also needs policy review before
    being treated as an approved workflow path.
  - Proposed verification: add a local dry-run summarizer before any merge path
    changes, and audit the workflow against the current fork layout.

- **Mobile/Remote parity planning**
  - Prior-session evidence: this is current 2026-10-10 operator direction rather
    than legacy recovery evidence.
  - Current source status: official OpenAI sources confirm Remote/mobile behavior.
    Active Code has app-server, browser, Code Bridge, and thread-history pieces
    that could support a local-first control surface.
  - Gap: there is not yet a Code-mod feature spec mapping connected hosts,
    approvals, diffs, screenshots, terminal output, branch/worktree selection,
    pairing, and local-secret boundaries.
  - Proposed verification: produce a security-reviewed local-first design before
    implementation, with no cloud relay or mobile pairing enabled by default.

## Task list

- [ ] Audit prior history for all Code-mod feature discussions before coding.
  - [x] Identify the recent high-signal session
        `codex:019ffbfe-e90d-72c2-a00a-3bb1ea1d6a10`.
  - [x] Cross-check it against `docs/changes/0001-canonical-history-recovery.md`.
  - [x] Inspect the older `code:` sessions named by `0001`:
        `code:9f863d9b-ed61-401a-bf5f-598912c21231`,
        `code:ccb698d4-e83d-44a7-b903-df4c5e31a713`, and
        `code:b28bed46-eb53-448a-aeed-61fdc367be55` for feature requests that
        did not survive into `main`.
  - [x] Produce a final evidence table mapping each discussed feature to current
        source status, tests, and remaining gap.

- [ ] Design and implement a bounded `session_history` tool.
  - [ ] Reuse existing local session stores where possible instead of inventing
        a new history database.
  - [ ] Provide at least search, list, read-session, and read-turn operations.
  - [ ] Bound output size, redact sensitive content, and make tool results cite
        session IDs/turn IDs so later answers can distinguish memory-derived
        evidence from current repo evidence.
  - [ ] Add resource limits and regression tests covering large histories,
        runaway scans, malformed rollouts, and missing metadata.

- [ ] Re-verify and harden resource guarding around command/session-history
      execution.
      In progress: supervise history in a resource-limited child of the native
      executable, retain import write-policy checks, bound IPC and deadlines,
      and clean owned import staging after worker termination. Remote tests are
      required before checking off the subtasks below; host-specific validation
      on the Steam Deck remains a separate gate.
  - [ ] Confirm current process-group memory watchdog behavior on this host.
  - [ ] Add focused tests for nested child processes and history scans that exceed
        memory or process limits.
  - [ ] Document when cgroup enforcement, process-group fallback, or explicit
        unsupported behavior is used.

- [ ] Add NSFW detection/gating before `image_view` attaches local images.
  - [ ] Define policy and UX for blocking, warning, or requiring explicit user
        approval before model-visible image attachment.
  - [ ] Prefer a local, deterministic, bounded classifier/gate suitable for the
        Steam Deck; do not send image contents to a remote classifier without an
        explicit privacy decision.
  - [ ] Cover direct `image_view`, pasted/local image attachments, and replay or
        generated-image paths that can attach images before provider requests.

- [ ] Recover/port tabs support.
  - [ ] Compare current `code-rs/tui` against legacy `codex-rs/tui` tabbed
        bottom-pane and plugin-selection infrastructure.
  - [ ] Identify which tab UX is intended: plugin tabs, settings tabs,
        multi-chat/session tabs, or all of them.
  - [ ] Add focused VT100/snapshot tests before porting UI behavior.

- [ ] Recover/port command execution and AGENTS command-section support.
  - [ ] Audit current command execution tools, local shell handling, and
        `AGENTS.md`/project-doc command-section parsing.
  - [ ] Ensure command examples in assistant output remain one-line copyable
        commands, not fenced or boxed blocks when the operator needs pasteable
        shell input.
  - [ ] Test both directions: commands described in project docs reaching the
        agent context, and model/tool command execution honoring safety and
        shell-profile policy.

- [ ] Add an upstream/OpenAI Codex update strategy for internal borrowed code.
  - [ ] Treat `.github/workflows/upstream-merge.yml` as existing infrastructure
        and audit whether it still works with the current fork layout.
  - [ ] Add a local/manual dry-run command that compares upstream `openai/codex`
        to current `main`, summarizes affected crates, and refuses to mutate
        without an explicit merge/update command.
  - [ ] Preserve fork-owned behavior, branding, tools, and safety gates while
        selectively adopting upstream fixes.

- [ ] Add Codex Remote/mobile parity planning.
  - [ ] Map OpenAI Remote capabilities into Code-mod concepts: connected host,
        workspace/project picker, branch/worktree selection, queue vs steer,
        approvals, diffs, terminal output, screenshots, side chats, goals, skills,
        plugins, and remote SSH/cloud-environment boundaries.
  - [ ] Decide which pieces can be implemented locally first through app-server,
        Code Bridge, or a small mobile/web control surface before any cloud relay
        exists.
  - [ ] Define security requirements for pairing, authorization, local secrets,
        and command approvals.

- [ ] Reconcile known existing recovery gates from `0001` before marking this
      roadmap complete.
  - [ ] Prompt-history and view scrolling must still be verified independently in
        SSH and Decky Terminal contexts.
  - [ ] Long-session slowdown must still be reproduced after restarting Code and
        inspecting `turn_latency` and `context_management` logs.

## Initial implementation order

1. Finish prior-history audit and evidence table.
2. Implement `session_history` as a bounded read/search tool, with resource
   guarding tests.
3. Re-verify resource guard behavior under session-history load.
4. Add NSFW image gate because it affects model-visible user data.
5. Port command-section/command execution fixes.
6. Port tab UI features.
7. Expand upstream-update and mobile-remote parity as larger feature tracks.

## Acceptance gates

- No implementation begins from Git ancestry alone; each recovered item must
  reference prior-session evidence, current source evidence, or both.
- Every new tool or execution path has bounded output, one-thread-compatible
  tests, and resource-limit behavior.
- The final installed binary is verified separately from compilation.
- `docs/changes/0001-canonical-history-recovery.md` remains the recovery ledger;
  this document only tracks forward feature parity and lost-work recovery.
