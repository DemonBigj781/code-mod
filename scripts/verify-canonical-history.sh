#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
base_file="$repo_root/.canonical-base"

fail() {
  echo "canonical-history: $*" >&2
  exit 1
}

[[ -f "$base_file" ]] || fail "missing .canonical-base"

value_for() {
  local key="$1"
  sed -n "s/^${key}=//p" "$base_file" | head -n 1
}

canonical_base="$(value_for base_commit)"
canonical_upstream="$(value_for upstream_url)"
integration_url="$(value_for integration_url)"

[[ -n "$canonical_base" ]] || fail "base_commit is not configured"
git cat-file -e "${canonical_base}^{commit}" 2>/dev/null \
  || fail "canonical base commit $canonical_base is unavailable"
git merge-base --is-ancestor "$canonical_base" HEAD \
  || fail "HEAD is not descended from immateria/codex-mod base $canonical_base"

while IFS= read -r forbidden; do
  [[ -n "$forbidden" ]] || continue
  if git cat-file -e "${forbidden}^{commit}" 2>/dev/null \
    && git merge-base --is-ancestor "$forbidden" HEAD; then
    fail "HEAD contains forbidden wrong-base ancestor $forbidden"
  fi
done < <(sed -n 's/^forbidden_ancestor=//p' "$base_file")

if [[ "${1:-}" == "--local" ]]; then
  expected_root="/var/home/jack/projects/AI_Project/code"
  expected_real_root="$(realpath -m "$expected_root")"
  actual_real_root="$(realpath -m "$repo_root")"
  [[ "$actual_real_root" == "$expected_real_root" ]] \
    || fail "canonical local checkout must be $expected_root (found $repo_root)"

  worktree_count="$(git worktree list --porcelain | sed -n 's/^worktree //p' | wc -l)"
  [[ "$worktree_count" -eq 1 ]] \
    || fail "canonical checkout has $worktree_count registered worktrees; expected 1"

  [[ "$(git remote get-url upstream)" == "$canonical_upstream" ]] \
    || fail "upstream must be $canonical_upstream"
  [[ "$(git remote get-url origin)" == "$integration_url" ]] \
    || fail "origin must be $integration_url"

  "$repo_root/scripts/audit-code-variants.sh"
fi

echo "canonical-history: verified $canonical_base"
