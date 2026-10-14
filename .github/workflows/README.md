# Workflow Strategy

Rust-specific `rust-ci*.yml` workflows are intentionally removed in this fork.

## Verification Paths

- `bazel.yml` is the primary Rust verification path for pull requests and for `main`.
- `cosmopolitan-code-ape.yml` is the only binary build path. It builds one
  pinned x86-64 APE on Linux and validates the same artifact on Linux and
  Windows; it runs only on `main`.
- Platform-specific Linux/Windows release and preview binary workflows are
  intentionally removed so binary distribution goes through the APE artifact.

## Upstream Merge Guardrail

- `.github/workflows/**` is fork-owned during upstream merges.
- `.github/workflows/rust-ci.yml` and `.github/workflows/rust-ci-full.yml` are
  also listed in `.github/merge-policy.json` under `perma_removed_paths` so
  future upstream syncs keep them deleted.
