# Workflow Strategy

Rust-specific `rust-ci*.yml` workflows are intentionally removed in this fork.

## Verification Paths

- `cosmopolitan-code-ape.yml` is the binary build path for the `Main` branch.
  It builds one pinned x86-64 Cosmopolitan APE on Linux and validates the same
  artifact on Linux and Windows on every relevant `Main` push or manual run.
- `test-only.yml` is a manual `Main`-only fast path for source, project-scope,
  image, memory, snapshot, Rustwright, and workflow contract tests. It does not
  build or package the portable APE.
- `compile-only.yml` is a manual `Main`-only compile gate for the product crates,
  feature combinations, test targets, and agent schema harness. It does not run
  tests or package the portable APE.
- Platform-specific Linux/Windows release and preview binary workflows are
  intentionally removed so binary distribution goes through the APE artifact.

## Upstream Merge Guardrail

- `.github/workflows/**` is fork-owned during upstream merges.
- `.github/workflows/rust-ci.yml` and `.github/workflows/rust-ci-full.yml` are
  also listed in `.github/merge-policy.json` under `perma_removed_paths` so
  future upstream syncs keep them deleted.
