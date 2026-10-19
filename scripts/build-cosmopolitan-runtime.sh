#!/bin/sh
set -eu
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
support_dir="$script_dir/cosmopolitan-code"
. "$support_dir/prepare-runtime.sh"
printf 'Source runtime probe: %s\n' "$runtime_smoke"
