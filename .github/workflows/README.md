# Workflow Strategy

Rust-specific `rust-ci*.yml` workflows are intentionally removed in this fork.

## Verification Paths

- `compile-main.yml` is the standard Linux binary build path for the `Main`
  branch. It runs the repository's `build-fast.sh` script with one Cargo build
  job and uploads the testable x86-64 `code` binary.
- `cosmopolitan-code-ape.yml` builds one pinned x86-64 APE on Linux and
  validates the same artifact on Linux and Windows; it runs only on `Main`.
- Platform-specific Linux/Windows release and preview binary workflows are
  intentionally removed so binary distribution goes through the APE artifact.

## Upstream Merge Guardrail

- `.github/workflows/**` is fork-owned during upstream merges.
- `.github/workflows/rust-ci.yml` and `.github/workflows/rust-ci-full.yml` are
  also listed in `.github/merge-policy.json` under `perma_removed_paths` so
  future upstream syncs keep them deleted.
