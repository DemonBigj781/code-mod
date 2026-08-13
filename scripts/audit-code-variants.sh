#!/usr/bin/env bash
set -euo pipefail

expected_root="$(realpath -m /var/home/jack/projects/AI_Project/code)"
projects_root="$(realpath -m /var/home/jack/projects)"
scan_roots=(
  "$projects_root"
  /var/home/jack/.code-tmp
  /var/home/jack/tmp
  /var/home/jack/backups
)

declare -A seen=()
declare -a candidates=()

record_candidate() {
  local candidate
  candidate="$(realpath -m "$1")"
  if [[ -z "${seen[$candidate]:-}" ]]; then
    seen["$candidate"]=1
    candidates+=("$candidate")
  fi
}

for scan_root in "${scan_roots[@]}"; do
  [[ -d "$scan_root" ]] || continue

  while IFS= read -r -d '' candidate; do
    [[ -f "$candidate/code-rs/Cargo.toml" ]] || continue
    record_candidate "$candidate"
  done < <(
    find "$scan_root" -maxdepth 4 \
      -type d \( \
        -name code-mod -o \
        -name codex-mod -o \
        -name 'code-mod-*' -o \
        -name 'codex-mod-*' \
      \) -print0 2>/dev/null
  )

  while IFS= read -r -d '' config; do
    remote_urls="$(git config --file "$config" --get-regexp '^remote\..*\.url$' 2>/dev/null || true)"
    if rg -q 'github\.com/(immateria/codex-mod|DemonBigj781/code(-mod)?|just-every/code)(\.git)?$' <<<"$remote_urls"; then
      record_candidate "$(dirname "$(dirname "$config")")"
    fi
  done < <(
    find "$scan_root" -maxdepth 5 \
      \( -type d \( -name target -o -name node_modules -o -name .cache \) -prune \) -o \
      \( -type f -path '*/.git/config' -print0 \) 2>/dev/null
  )
done

if [[ "${#candidates[@]}" -ne 1 || "${candidates[0]:-}" != "$expected_root" ]]; then
  echo "canonical-history: expected only $expected_root; found Code source variants:" >&2
  if [[ "${#candidates[@]}" -eq 0 ]]; then
    echo "  (none)" >&2
  else
    printf '  %s\n' "${candidates[@]}" >&2
  fi
  exit 1
fi

echo "canonical-history: verified sole Code source checkout $expected_root"
