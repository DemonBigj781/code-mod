# Main Release Consolidation Design

## Goal

Finish the main release by recovering every valid intended implementation from
the mixed development state, completing the documented provider features, and
submitting coherent changes to `origin/main` for hosted one-worker compilation
and verification.

The consolidation starts from commit
`d8a80334438402abd237b083429a67b9dc06aa1c`. The dirty source worktree at
`/var/home/jack/.config/superpowers/worktrees/code/feat-mcp-reload` remains
read-only evidence until every edit has been classified.

## Required Scope

The release MUST include:

- Direct OpenAI-compatible `/v1` endpoint and model selection from the TUI.
- The AI Horde auxiliary client for text generation, image generation, image
  interrogation, and image vectorization.
- Every coherent, valid fix recoverable from the mixed worktree, including the
  identified review-event ordering, late-event suppression, transcript
  scrolling, empty-composer navigation, interface-setting persistence,
  extended-context, resource-request, and MCP lifecycle clusters.
- One authoritative release ledger that replaces the current fragmented plan
  state.
- Documentation and release automation reconciled with the final behavior.

The consolidation MUST NOT blindly copy the mixed working tree. Existing
resource-request and MCP-reload implementations on `origin/main` are the
authoritative starting point. Dirty hunks that duplicate, regress, or conflict
with those implementations must be rejected or reconstructed against current
code.

## Inventory And Classification

Every modified or untracked path in the mixed worktree, every local branch not
merged into `origin/main`, and every retained feature plan MUST appear in the
release ledger under exactly one classification:

- `already integrated`: Behavior is present and verified on `origin/main`.
- `required and incomplete`: Approved release behavior has not been
  implemented.
- `valid dirty fix`: A coherent implementation exists only in the mixed tree
  and must be reconstructed.
- `superseded or duplicate`: The edit is obsolete, already replaced, or would
  regress current behavior.
- `excluded`: The edit is unrelated to the approved release and has an explicit
  exclusion reason.

Classification is based on behavior, tests, and current source structure, not
on filename-level similarity. Overlapping files must be reviewed hunk by hunk.

## Delivery Architecture

Work is divided into independently reviewable streams. Each stream begins in a
fresh isolated worktree created from the latest verified `origin/main`:

1. Release ledger and full dirty-hunk inventory.
2. Small correctness fixes and missing resource/MCP edge coverage.
3. Review-event ordering and transcript-scrolling fixes.
4. Direct OpenAI-compatible `/v1` model selection.
5. AI Horde auxiliary client.
6. Documentation, release automation, and final release verification.

If the inventory identifies another coherent required stream, it is added to
the ledger with dependencies and acceptance tests before implementation.

Each implementation stream is reconstructed rather than copied wholesale. A
stream may reuse a dirty hunk only after confirming that its surrounding source
and assumptions still match current `origin/main`.

## Submission And Verification Flow

Rust compilation does not run locally. Local checks are limited to safe static
validation that does not compile the workspace, such as patch checks, syntax or
format checks that do not invoke a build, workflow validation, and source
audits.

For each stream:

1. Add regression tests or a deterministic validation harness before the
   implementation where practical.
2. Reconstruct the implementation against the latest verified `origin/main`.
3. Perform a hunk-level changeset review and static validation.
4. Commit the isolated stream.
5. Submit and push the commit to `origin/main`.
6. Allow GitHub Actions to compile and test with exactly one worker.
7. Inspect exact hosted logs on failure, patch the same stream, and push again.
8. Mark the stream verified only after all applicable hosted workflows pass.

Dependent streams do not start from an unverified commit. Independent audit and
design work may proceed while a hosted workflow runs, but no later stream is
submitted on top of a failed baseline.

## Provider Feature Boundaries

### Direct OpenAI-Compatible `/v1`

The existing design remains the behavioral baseline: users add an endpoint in
the model selector, discover provider-scoped models, store optional credentials
through the encrypted secret store, and select a model for the active session
without creating a profile or restarting Code.

Before implementation, the old plan must be reconciled with the current model
provider, remote model cache, secret storage, and session reconfiguration APIs.
It must not introduce a second request runtime where existing Chat Completions
or Responses transports already apply.

### AI Horde

The existing design remains the behavioral baseline: AI Horde is an auxiliary
service and never replaces the primary tool-capable Code model. It provides
text, image, interrogation, and image-vectorization tools, authenticated or
anonymous operation, live worker counts, bounded polling, cancellation, and
managed image artifacts.

Before implementation, endpoint shapes and lifecycle assumptions must be
validated against current authoritative AI Horde documentation. Credentials and
large base64 payloads must never enter configuration, logs, tool history, or
conversation history.

## Correctness And Failure Handling

- Bidirectional behavior is tested independently. Enabling after disabling,
  scrolling up and down, and accepting or rejecting approvals each require
  separate coverage.
- Completion events must prevent stale events for the same submission from
  mutating finalized history while preserving unrelated later submissions.
- Transcript geometry uses widths capable of representing histories larger
  than `u16` and keeps keyboard, mouse, and scrollbar positions consistent.
- Provider failures retain usable stale catalogs where the approved design
  requires it and expose fresh, loading, stale, authentication-failure, and
  connection-failure states distinctly.
- Asynchronous AI Horde work is cancelled before fallback or abandonment so
  duplicate generation and duplicate Kudos use are not possible.
- A failed hosted workflow blocks verification of that stream and is resolved
  from its exact logs rather than bypassed with broad guards or disabled tests.

## Plan And Documentation Model

One release ledger is authoritative. It records:

- Every discovered stream and source hunk.
- Classification and rationale.
- Dependencies and implementation order.
- Regression tests and hosted workflow coverage.
- Submitted commit and GitHub Actions run identifiers.
- Final disposition of superseded and excluded edits.

Existing feature specifications may be retained as supporting documents after
they are reconciled with current source. Obsolete implementation plans are not
used as competing trackers; useful requirements are migrated into the release
ledger and current per-stream plans.

## Completion Criteria

The main release is complete only when:

- Every mixed-worktree edit, untracked file, and unmerged local branch has a
  recorded disposition.
- Both mandatory provider features are implemented and documented.
- Every valid dirty fix is reconstructed and covered by a regression test or a
  documented deterministic validation.
- No stale implementation is reintroduced over the verified resource-request
  or MCP-reload code.
- The final `origin/main` tree is clean and contains no accidental artifacts.
- All focused hosted workflows pass on their submitted commits.
- The final full hosted build and release-binary smoke verification pass with
  one worker.
- The release ledger contains the final commit and run evidence and has no
  unresolved required item.
