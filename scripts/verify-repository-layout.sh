#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"

fail() {
  echo "code-layout: $*" >&2
  exit 1
}

current_branch="$(git branch --show-current)"
[[ "$current_branch" == "main" ]] \
  || fail "the only working branch must be main (found ${current_branch:-detached HEAD})"

branch_count="$(git for-each-ref --format='%(refname)' refs/heads | wc -l)"
[[ "$branch_count" -eq 1 ]] \
  || fail "found $branch_count local branches; expected only main"

worktree_count="$(git worktree list --porcelain | sed -n 's/^worktree //p' | wc -l)"
[[ "$worktree_count" -eq 1 ]] \
  || fail "found $worktree_count registered worktrees; expected only the primary checkout"

if [[ "${1:-}" == "--local" ]]; then
  expected_root="/var/home/jack/projects/AI_Project/code"
  expected_real_root="$(realpath -m "$expected_root")"
  actual_real_root="$(realpath -m "$repo_root")"
  [[ "$actual_real_root" == "$expected_real_root" ]] \
    || fail "the Code checkout must be $expected_root (found $repo_root)"

  "$repo_root/scripts/audit-code-variants.sh"
fi

echo "code-layout: verified one main branch and one worktree"
