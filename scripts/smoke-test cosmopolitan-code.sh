#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
support_dir="$script_dir/cosmopolitan-code"

if [ "$#" -gt 1 ]; then
    echo "Usage: $0 [ARTIFACT_DIRECTORY]" >&2
    exit 2
fi

artifact_dir=${1:-"$repo_root/o/cosmopolitan-code"}
artifact_name=$(python3 - "$support_dir/pins.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    print(json.load(source)["artifact_name"])
PY
)
artifact="$artifact_dir/$artifact_name"
manifest="$artifact.json"
runtime_smoke="$artifact_dir/runtime-smoke.com"

for required in "$artifact" "$manifest" "$runtime_smoke"; do
    if [ ! -f "$required" ]; then
        echo "Required APE artifact file is missing: $required" >&2
        exit 1
    fi
done

set -- "$support_dir/artifact_contract.py" "$artifact" "$manifest"
if [ -n "${GITHUB_SHA:-}" ]; then
    set -- "$@" --source-commit "$GITHUB_SHA"
fi
python3 "$@"
file "$artifact"
sh "$runtime_smoke"
"$artifact" --version
"$artifact" --help >/dev/null
python3 "$support_dir/check-history-worker.py" "$artifact"
