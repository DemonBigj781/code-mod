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
2026-10-16: resource guarding for command/history execution passed the remote
and Steam Deck acceptance checks recorded below. The next feature task is the
NSFW image-attachment gate.
Tab-isolation and AI-selected session-title changes are a separate verification
track and are not considered complete until their GitHub Actions run passes.
The separate Cosmopolitan environment update remains deferred until all known
objectives are addressed and the operator is ready; its pinned revision is
unchanged by this work.

2026-10-16: the operator-prioritized dynamic SSH PC registry is implemented in
`28f6c97cb`. Actions run `38959868287` passed its 12 core registry tests and five
TUI/privacy tests, plus the existing Linux/Windows artifact gates. Registration
does not yet provide remote command execution or SCP; those remain unchecked
below. Image gating and `/blind` are still pending, not implied by registry work.

2026-10-18: Actions run `39096452473` passed for `cb1966a3f`, including 13 SSH
registry tests, 10 SSH UI/privacy tests, 265 settings tests, 87 agent-related core
tests, and the real tool-dispatch contract test against an isolated fake provider.
The same APE passed Linux and Windows artifact checks. `/settings ssh` and local
registry notices are implemented. Reader creation now selects one model from a
session-stable random shortlist of up to five eligible models. See the tasks
below for live-provider and transport boundaries; no installed binary is changed
by this validation record.

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

- [x] Re-verify and harden resource guarding around command/session-history
      execution.
      Verified 2026-10-16 at `adc8f6bc2e511bd9fd5a1a0fb884f61befdf5c7f`;
      the existing implementation required no additional runtime changes.
      GitHub Actions run `38830145137` passed on Linux and Windows. Its Linux
      job passed all seven worker-supervisor tests, both memory-watchdog tests,
      all 17 history-library tests, and the native history integration test.
      Both platforms passed the shipped-worker smoke checks for retrieval,
      malformed/oversized input, import write policy, allocation-heavy imports,
      and the worker-local deadline.
  - [x] Confirm current process-group memory watchdog behavior on this host.
        Compiled the current `code-rs/core/src/cgroup.rs` directly as a test
        crate against cached Rust 1.93.0 dependencies with one codegen unit and
        one linker thread. All five tests passed with `--test-threads=1`,
        including single-child and aggregate nested-child RSS termination.
        The standalone compile emitted unused-item warnings because it excludes
        the surrounding core callers; it was not a full workspace rebuild.
  - [x] Add focused tests for nested child processes and history scans that exceed
        memory or process limits.
        Existing tests above cover these paths. On the Steam Deck,
        `scripts/cosmopolitan-code/check-history-worker.py /var/home/jack/bin/code`
        passed against the installed artifact, whose SHA-256 matches the
        successful CI artifact manifest:
        `973af390ef5912d4fc660042d102bded042b2ee50699d42e518db6877d6c7daf`.
        Fixtures used temporary synthetic stores, not personal session history.
  - [x] Document when cgroup enforcement, process-group fallback, or explicit
        unsupported behavior is used.
        See [Resource containment](../session-history.md#resource-containment).
        Live `/proc` inspection confirmed a 512 MiB address-space limit,
        30-second CPU limit, and disabled core dumps. The worker stayed in the
        inherited terminal cgroup on this host: dedicated cgroup memory/PID
        enforcement was not exercised here. The RSS fallback was verified
        separately above; it does not provide an equivalent process-count cap.
        Windows evidence is for the APE artifact, not the native Windows Job
        Object implementation. These checks do not establish native macOS support.

- [ ] Add NSFW detection/gating before `image_view` attaches local images.
  - [ ] Define policy and UX for blocking, warning, or requiring explicit user
        approval before model-visible image attachment.
        2026-10-16 operator decisions: screening applies to OpenAI models only;
        other models bypass both layers without classifier initialization.
        Determine scope from the actual selected/routed model, not merely an
        OpenAI-compatible API transport; re-evaluate scope on model changes.
        For OpenAI models, layer 1 is enabled by default; layer 2 is disabled
        by default for performance. Layer 1 flags and holds suspect assets
        before an OpenAI request. The operator can preview a held asset
        locally in the terminal; previewing alone must not attach its bytes to
        model input, tool results, replay, or automatic screenshot delivery.
        Sending a held asset requires a distinct operator-only "Send to AI"
        action, scoped to the exact reviewed image and owning session. If layer
        2 is enabled, its blocking decision still applies before transmission.
  - [ ] Add independent, persisted operator controls to skip layer 1 or layer 2.
        Skipping layer 1 routes directly to enabled layer 2. Skipping layer 2
        retains layer-1 flag/review behavior. Skipping both disables NSFW
        screening with a visible warning, not file-validation/resource limits.
        Disabling an enabled layer for OpenAI requires an operator-visible
        warning followed by explicit confirmation before applying or saving
        the change; cancellation preserves the previous setting. The default
        disabled layer 2 does not prompt at startup, and automatic bypass for
        non-OpenAI models does not require this confirmation.
        Tool arguments and model responses cannot change these controls or
        approve held images. Missing/failed enabled classifiers hold the image
        with an actionable error rather than silently allowing transmission.
  - [ ] Add `/blind <dir> [<file_name_pattern>] on|off` as a persisted image
        privacy rule. The command is `/blind`, not `/bind`. Both the directory
        and optional filename pattern accept wildcards; filename patterns match
        basenames. Omitting the filename pattern covers all images under the
        matching directories, including descendants. Preserve wildcard rules
        for future matches rather than expanding only currently existing paths.
        Unlike NSFW screening, blind rules apply to every model and take
        precedence over layer skips and per-image "Send to AI" approval.
        `on` withholds matching images from future
        model attachments while preserving local terminal preview. `off`
        removes that exact directory/pattern rule; other matching or enclosing
        blind rules still win. The operator can enable or disable rules. The AI
        may enable blindness when instructed, but may not disable or weaken it;
        removal remains an explicit operator action. Support quoted paths and
        patterns and resolve relative directory patterns against the session
        working directory. Invalid patterns must fail without changing rules.
        Canonical path/symlink checks and image provenance
        must prevent attachment/replay bypasses; cached approvals must not
        override newly enabled blindness. Rules cannot retract data already
        sent to a provider.
        Accept local targets such as `/blind /mnt/* on` and remote targets such
        as `/blind jack@10.0.0.5/mnt/* on`, retaining the optional filename
        pattern before `on|off`. Remote rules are scoped to a normalized SSH
        connection identity and remote path, not to a same-named local path.
        Resolve connection aliases consistently and preserve remote provenance
        on SCP downloads, project working copies, previews, and replay so
        copying a protected asset locally does not remove its restriction.
  - [ ] Prefer a local, deterministic, bounded classifier/gate suitable for the
        Steam Deck; do not send image contents to a remote classifier without an
        explicit privacy decision.
  - [ ] Cover direct `image_view`, pasted/local image attachments, and replay or
        generated-image paths that can attach images before provider requests.

- [ ] Add registered SSH connections, SSH/SCP tools, and remote project folders.
  - [x] Register named connection profiles with an SSH endpoint/URL, username,
        port, and password, key-file, or SSH-agent authentication. Use
        `/ssh add <user@host[,user@host...]> <pwd|key> <name>`; comma-separated
        endpoints are ordered alternatives for one profile. Try endpoints outside
        `100.64.0.0/10` left to right first, then endpoints in `100.64.0.0` through
        `100.127.255.255` left to right. Preserve the entered ordering in storage;
        the range preference is not an endpoint-identity or trust check. Addresses
        such as `100.63.*` and `100.128.*` are outside the deferred range. Support
        quoted profile names. `pwd` selects a masked password prompt; `key`
        selects a local key-file/SSH-agent flow, with a masked passphrase prompt
        if needed.
        These arguments select authentication modes, never inline secrets.
        Subsequent SSH/SCP calls resolve saved credentials from the profile;
        callers never repeat the password, private key, or passphrase.
        Verify endpoint identities before using shared credentials. Failover
        handles unreachable endpoints, not host-key mismatches or authentication
        rejection; do not retry or duplicate a command once execution may have
        started. Remote project identity and blind rules remain profile-scoped
        when switching between registered endpoints. Registration
        and credential changes are operator actions; tools use profile IDs and
        must not expose passwords, private-key contents, or passphrases to the
        model, command arguments, transcripts, diagnostics, or replay.
        Collect secrets through masked local UI; persist only credential-store
        references in configuration. Reuse `code-keyring-store` where supported,
        and report unavailable secure persistence rather than silently writing
        plaintext credentials. Key files remain local references.
        Implemented in `code-rs/core/src/ssh_registry.rs` with dynamic add/list/
        show/update/rename/auth/remove commands, stable IDs, atomic private
        metadata, inter-process locking, and a masked TUI credential form.
        See [SSH connections](../ssh-connections.md). Remote test evidence is
        recorded above; secure-store tests use an isolated test double, not real
        PC credentials. Real-host authentication and DNS-resolved endpoint
        failover remain transport acceptance work in the following task.
  - [x] Add an SSH settings section and repair `/ssh list` notice delivery.
        The overlay and compact pane share add/edit/auth/remove controls, explicit
        empty/error states, and stale-edit protection. Registry results now use
        a dedicated notice event instead of the assistant-stream channel. Tests
        cover both settings routes and model-label metadata in both directions.
  - [x] Perform the operator-authorized read-only system SSH check on Xavier.
        The existing `sshxavier` wrapper identified `jack@100.78.110.50`; its
        saved endpoint host key matched. Strict host-key verification and batch
        authentication succeeded, returning user `jack`, host `tegra-ubuntu`,
        `Linux aarch64`, and device model `Jetson-AGX`. No remote settings were
        changed and no Code profile was automatically registered. This is not
        validation of the still-unimplemented agent-facing SSH/SCP transport.
  - [ ] Expose bounded SSH command execution and SCP upload/download operations
        using registered profiles. Use one stable `ssh` tool with arguments
        such as `{"name":"mint","command":"ls"}`, not dynamically registered
        `ssh.mint` function names. Mint is the operator's PC next to the Steam
        Deck; the operator plans to register Mint and Xavier after this feature
        is ready. Keep names generic: neither machine is a built-in connection,
        and examples alone do not authorize registration or connection. The
        separately authorized Xavier check is recorded above.
        Allow an optional `auth` selector with values
        `pwd` or `key`, for example
        `{"name":"mint","auth":"key","command":"ls"}`. Omission uses the
        profile's default; an explicit selection must use an authentication
        method already configured for that profile, never inline credentials
        or an implicit registration/change. Remote names resolve only to registered
        connections; a model cannot supply raw authentication material or silently
        register a host. Reserve `self`, `127.0.0.1`, and `0.0.0.0` as equivalent
        local-execution aliases, for example `{"name":"self","command":"ls"}`.
        These aliases delegate to the existing local shell backend with its
        session working directory, approvals, sandbox, cancellation, and resource
        limits; they do not require SSH credentials or a local SSH daemon.
        Treat `0.0.0.0` strictly as an input alias, never as a network destination
        or listener binding. Registered profiles cannot override these aliases,
        and the existing shell tool remains available. SCP uses the same
        named-profile/local-endpoint lookup. A display label
        may show `ssh.mint` without making that the provider-facing tool name.
        Also accept the structured shell-tool alias
        `shell ssh <name> [pwd|key] [--] <command>`. For example,
        `shell({"command":["ssh","<name>","pwd","ls"]})` selects the saved
        password authentication for that profile; `pwd` is a mode token, not
        an inline password. Omitting the mode uses the profile default.
        `shell({"command":["ssh","<name>","--","pwd"]})` instead executes the
        literal working-directory command. Reject a missing command rather than
        treating an authentication selector as executable text. Both forms route
        through the dedicated `ssh` tool's dispatcher; the reserved local
        aliases retain local execution. Preserve the command argument as
        supplied, without textual replacement inside scripts or quoted strings,
        and do not install host-level shell aliases. Both tool entry points must
        share connection lookup, authentication, authorization, blind rules,
        cancellation, resource/output limits, and errors. Test equivalent
        routing through both entry points and malformed alias arguments.
        Verify host keys; first-use trust and changed
        keys require explicit operator handling, not automatic acceptance.
        Preserve command/transfer authorization, cancellation, timeouts, output
        caps, and safe argument handling; never interpolate untrusted paths into
        a local shell command.
  - [ ] Allow an SCP-accessible remote directory to be registered and selected as
        a project folder. Track its connection and remote root separately from
        any local working copy; SCP transfer is not a filesystem mount. Remote
        commands use the remote project root. Transfers must detect conflicts
        and avoid silent overwrites or deletions of remote/local work.
  - [ ] Integrate remote projects and transfers with the image-gating `/blind`
        rules above; keep local-only preview separate from model attachment.
        Test SSH/SCP in both directions, including local-alias equivalence and
        unchanged local execution protections, authentication failures,
        host-key changes, quoted paths, credential redaction, cancellation,
        project identity, and remote-to-local blind-rule preservation.

- [ ] Finish reader-agent orchestration and live-provider validation.
  - [x] Let the AI select one model from up to five randomly sampled,
        operator-enabled, authentication-eligible candidates. The sample is
        stable for a session/eligibility snapshot, and each create call has one
        launch site. Disabled models and the master switch remain authoritative.
        This operator-directed bounded model choice supersedes the earlier
        task-only create contract without allowing permission/context overrides.
  - [x] Repair status/result/wait/cancel/list argument contracts and preserve
        session ownership. Agent-ID-only operations work, list needs no batch,
        and missing IDs fail promptly. The old hidden batch requirement was
        reproduced against the running beta before the fix.
  - [x] Remove the full-transcript clone during bounded parent handoff; advertise
        only the candidate model guide; align CLI, TUI metadata, and prompt
        examples with the singular `create.model` field. The production path
        previously launched the full allowed list, while the five-model helper
        existed only under tests. These are source-backed latency/resource
        findings, not measured external-provider speedups.
  - [x] Validate contracts, shortlist stability, and cross-session isolation on
        Actions. The final run above also fixes stale portable settings-test
        navigation rather than skipping tests; an early schema preflight and
        bounded waits prevent repeats of the six-hour CI hang.
  - [ ] Measure live startup/queue/inference behavior with an operator-approved
        reader provider after enabling read agents. The operator's master
        switch remains off; no real read agents were launched during this work.

- [ ] Recover/port tabs support.
  - [ ] Compare current `code-rs/tui` against legacy `codex-rs/tui` tabbed
        bottom-pane and plugin-selection infrastructure.
  - [ ] Identify which tab UX is intended: plugin tabs, settings tabs,
        multi-chat/session tabs, or all of them.
        2026-10-11 clarification: the operator meant the top-row tab feature,
        not plugin-selection tabs; do not mark plugin-tab exploration as this
        task's completion.
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

- [ ] Evaluate Rustwright as a built-in browser automation tool.
  - [ ] Review `https://github.com/Skyvern-AI/rustwright` as a candidate for
        Code's browser tool/runtime surface. It advertises Playwright-compatible
        Python/Node APIs on a native Rust CDP engine, plus agent-focused CLI and
        standalone MCP package options.
  - [ ] Include `https://github.com/Pedrvisk/camoufox-rust` in the same browser
        runtime evaluation as a Camoufox/Firefox-oriented companion or
        alternative. It advertises Rust-managed Camoufox binaries, fingerprint
        and persona handling, native Juggler automation, CLI launch/verify flows,
        and persistence backends.
  - [ ] Include `https://github.com/9prodhi/camoufox-rs` in the Camoufox track
        as a pure Rust Juggler-protocol client candidate with process launch,
        pipe transport, session routing, Browser/Context/Page-style APIs, and an
        optional daemon CLI.
  - [ ] Include the EvilFox Camoufox anti-detect change
        `https://github.com/Evil0ctal/EvilFox/commit/2e933e16ef0fb4253af4d8fbe99198ec2c6f32d4`
        as a concrete Camoufox hardening input. The commit advertises improved
        resistance to advanced bot detection and changes Camoufox anti-detect
        helpers plus browser property configuration.
  - [ ] Include `https://github.com/adambenhassen/crw-camofox` in the adjacent
        crawler/search-tool evaluation as a self-hosted, Firecrawl-compatible
        Rust service with API and MCP server surfaces for agent workflows.
  - [ ] Compare Rustwright against the existing `code-rs/browser` runtime and
        `browser` OpenAI tool, and compare Camoufox against that same surface,
        before adding dependencies or replacing control paths.
  - [ ] Decide whether integration should be native crate/API, bundled CLI, or
        MCP-backed, and verify Chromium install, sandboxing, session ownership,
        screenshots, console events, Firefox/Camoufox compatibility, anti-detect
        privacy/safety policy, and resource limits on the Steam Deck.

- [ ] Add an upstream/OpenAI Codex update strategy for internal borrowed code.
  - [ ] Treat `.github/workflows/upstream-merge.yml` as existing infrastructure
        and audit whether it still works with the current fork layout.
  - [ ] Add a local/manual dry-run command that compares upstream `openai/codex`
        to current `main`, summarizes affected crates, and refuses to mutate
        without an explicit merge/update command.
  - [ ] Preserve fork-owned behavior, branding, tools, and safety gates while
        selectively adopting upstream fixes.

- [ ] Add Codex Remote/mobile parity planning.
      Reuse the registered SSH/SCP connection and remote-project task above;
      do not create a second credential or remote-workspace implementation.
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
