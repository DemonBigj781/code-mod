# TODO

This is the canonical backlog for standalone Code work. Feature-specific task
details remain in their change documents and are linked here rather than copied,
so each task has one authoritative checkbox.

Compilation and testing for this repository run only through GitHub Actions.
Current build, release, and repository constraints are documented in
[`HANDOFF.md`](HANDOFF.md).

## Active Change Documents

- [Code recovery and repository normalization](docs/changes/0001-canonical-history-recovery.md)
- [Code feature parity and lost work recovery](docs/changes/0002-code-feature-parity-roadmap.md)

## Standalone Backlog

- [ ] Evaluate and integrate [Box64](https://github.com/ptitSeb/box64) as an
  optional compatibility layer for running the x86-64 Cosmopolitan APE on
  supported non-AMD64 Linux hosts. Keep native architecture builds preferred,
  and first verify whether Box64 can execute the APE directly or requires an
  extracted/assimilated ELF payload.

## Upstream Integration Backlog

Imported from `INTEGRATION_BACKLOG.md`, last updated there on April 1, 2026.
`codex-rs/` remains the read-only upstream landing zone; production changes
belong in `code-rs/`.

### High Priority

- [ ] Complete the remaining `code-hooks` parity and cleanup audit:
  - audit upstream schemas and method names;
  - review stop/continuation edge cases;
  - close remaining app-server and client hook-surface gaps.
- [ ] Complete secrets integration follow-through:
  - audit remaining credential environment-variable call sites;
  - add richer in-TUI secret creation and editing;
  - migrate connector and plugin secret flows that still require environment
    variables.

### Medium Priority

- [ ] Map `EscalationExecution::Permissions(...)` into shell escalation instead
  of supporting only unsandboxed escalation.
- [ ] Broaden shell-escalation heuristics and add focused policy tests.
- [ ] Port structured feedback capture and submission from `codex-rs/feedback/`.
- [ ] Port artifact packaging and retention beyond the transcript from
  `codex-rs/artifacts/`.

### Lower Priority / Tooling

- [ ] Evaluate and selectively port `codex-rs/app-server-client/`,
  `codex-rs/app-server-test-client/`, and `codex-rs/tui_app_server/` for
  integration testing and external client tooling.
- [ ] Decide whether converging on `codex-rs/codex-api/` and
  `codex-rs/codex-client/` provides enough value to replace the current backend
  client stack.
- [ ] Evaluate `codex-rs/lmstudio/` as an optional local-model integration.
