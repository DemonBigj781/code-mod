#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/../.." && pwd)

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 ARTIFACT MANIFEST" >&2
    exit 2
fi

artifact=$1
manifest=$2
pecheck_root=${COSMO_RUST_BUILD_ROOT:-"$repo_root/o/rust-ape"}
pecheck="$pecheck_root/sdk/vendor/cosmocc/bin/pecheck"

python3 "$script_dir/artifact_contract.py" "$artifact" "$manifest"
file "$artifact"
if [ ! -x "$pecheck" ]; then
    echo "Cosmopolitan pecheck is missing: $pecheck" >&2
    exit 1
fi
"$pecheck" "$artifact"
"$artifact" --version
"$artifact" --help >/dev/null
