# Workflow Strategy

Rust-specific `rust-ci*.yml` workflows are intentionally removed in this fork.

## Verification Paths

- `cosmopolitan-code-ape.yml` is the binary build path for the `Main` branch.
  It builds one pinned x86-64 Cosmopolitan APE on Linux and validates the same
  artifact on Linux and Windows on every relevant `Main` push or manual run.
- `test-only.yml` is a manual `Main`-only fast path for source, project-scope,
  image, memory, snapshot, Rustwright, and workflow contract tests. It does not
  build or package the portable APE.
- `check-only.yml` is a manual `Main`-only `cargo check` gate for product crates,
  feature combinations, test targets, and the agent schema harness.
- `compile-only.yml` is a manual `Main`-only APE build path that skips the test
  gates and uploads `code.com` plus its manifest in an artifact named `code.com`.
- `complete-artifact-creation.yml` is the reusable check/test/cross-platform
  artifact pipeline. It stops before publishing, defaults to prerelease version
  arguments, and accepts `release` to produce stable version metadata instead.
- `pre-release-only.yml` builds the supplied `X.Y.Zb` tag as a prerelease,
  includes the workflow attempt count in `code --version`, and publishes only
  the exact artifact validated on Linux and Windows.
- `release-only.yml` builds the same tag as a stable artifact without the
  attempt count, requires that exact artifact to pass Linux and Windows, then
  replaces the matching prerelease assets before promoting the tag to latest.
- `code-comeplete-pipeline.yml` calls the complete artifact workflow and then
  publishes a prerelease by default. Its secondary `release` choice promotes the
  matching prerelease with the separately validated stable assets.

## Version Contract

Code keeps the upstream Codex compatibility version separate from this fork's
mod version. Stable builds report `codex version: X Mod version: Y`; prerelease
builds append `build attempts: N`. The mod version must be newer than the
original `0.6.196` Code-mod baseline and retain the trailing `b` fork marker,
starting with `0.6.197b`.
- Platform-specific Linux/Windows release and preview binary workflows are
  intentionally removed so binary distribution goes through the APE artifact.

## Upstream Merge Guardrail

- `.github/workflows/**` is fork-owned during upstream merges.
- `.github/workflows/rust-ci.yml` and `.github/workflows/rust-ci-full.yml` are
  also listed in `.github/merge-policy.json` under `perma_removed_paths` so
  future upstream syncs keep them deleted.
