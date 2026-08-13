# Canonical History Recovery

**Status:** in-progress

## Decision

The only authoritative source base is
[`immateria/codex-mod`](https://github.com/immateria/codex-mod). The writable
integration repository is `DemonBigj781/code-mod`. The local canonical checkout
must be `/var/home/jack/projects/AI_Project/code`, on `main`, with no linked Git
worktrees.

`just-every/code` and the former `DemonBigj781/code` history are evidence sources
only. Neither may be merged into canonical history. Individual integrations must
be reapplied or cherry-picked onto the recorded canonical base and then verified
there.

## Reconstructed History

1. Development was intended to continue from `immateria/codex-mod`.
2. Commit `f5dff6a9f2a8e05509aef5669223247137713511` from the wrong fork became the
   parent of separate OpenRouter, CPU/history, remote-compaction, and startup-fix
   branches.
3. Those branches were treated as variants, so no single branch contained every
   integration. A later merge also brought `just-every/code` history into a local
   candidate instead of reconciling features onto the intended base.
4. The feature-complete `code-elf` artifact retained evidence of the intended
   OpenRouter picker and model variants, but it was not a reproducible source
   lineage.
5. On 2026-08-13, local `main` was reset to canonical base commit
   `f471024e1985d32e98231b66469635d36f41ccf7`. OpenRouter routing and remote
   compact fallback were reapplied as new commits. The checkpoint was pushed to
   the writable integration repository at `4d6fa2f54de4055a3d6e09b859adf35f992bcc2b`.
6. The requested backend update was already part of the authoritative base:
   backend-auth, service-tier parity, image-generation events, downstream merge
   repairs, and effective-settings work are ancestors of the canonical anchor.
   They were retained from `immateria/codex-mod`, not reintroduced through the
   rejected `just-every/code` merge.

The merge base between the intended and wrong lines is
`5b0c8300e56fdae096848a1376859e2d326b50e6`; shared ancestry before that point
does not make the later forks interchangeable.

## Prevention

- `.canonical-base` records the upstream URL, integration URL, anchor commit,
  and known forbidden ancestors.
- `scripts/verify-canonical-history.sh` rejects a main line that lacks the
  canonical anchor or contains a known wrong-base ancestor.
- GitHub runs that verifier for every pull request and every push to `main`.
- The repository pre-push hook additionally rejects noncanonical local paths,
  extra linked worktrees, or incorrect remote assignments.
- Integrations are tracked below as one change. A feature branch is temporary
  delivery state, never a product variant; it is deleted after integration.

## Tasks

- [ ] Deliver one canonical Code update
  - [x] Replace wrong-fork `main` ancestry with the recorded immateria base and push the checkpoint.
  - [x] Assign `origin` to `DemonBigj781/code-mod`, assign `upstream` to `immateria/codex-mod`, and remove the `just-every/code` remote.
  - [x] Reconcile OpenRouter routing, picker/provider selection, model variants, CPU/history performance, startup safety, remote compact fallback, and backend changes on canonical `main`.
  - [ ] Build and validate the combined executable from canonical `main`.
  - [ ] Remove all Code-related linked worktrees and obsolete local variant branches.
  - [ ] Rename the sole checkout to `/var/home/jack/projects/AI_Project/code`, enable the pre-push hook, and verify the history guard locally.
