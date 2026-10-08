#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
support_dir="$script_dir/cosmopolitan-code"
pins="$support_dir/pins.json"
cosmo_source="$repo_root/o/cosmopolitan-source"
build_root="$repo_root/o/rust-ape"
run_script="$cosmo_source/third_party/rust_ape/run.sh"
sdk="$build_root/sdk"
sdk_bin="$sdk/vendor/cosmocc/bin"
output_dir=${COSMOPOLITAN_CODE_OUTPUT:-"$repo_root/o/cosmopolitan-code"}

read_pin() {
    python3 - "$pins" "$1" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    print(json.load(source)[sys.argv[2]])
PY
}

cosmo_commit=$(read_pin cosmopolitan_commit)
cosmo_repository=$(read_pin cosmopolitan_repository)
rust_ape_commit=$(read_pin rust_ape_commit)
artifact_name=$(read_pin artifact_name)
source_commit=$(git -C "$repo_root" rev-parse HEAD)

if [ -n "$(git -C "$repo_root" status --porcelain)" ]; then
    echo "Refusing to build a prerelease from a dirty source tree." >&2
    exit 1
fi
if [ -e "$output_dir" ]; then
    echo "Refusing to overwrite existing output directory: $output_dir" >&2
    exit 1
fi

if [ ! -f "$run_script" ]; then
    printf 'Pinned Cosmopolitan sources are missing. Fetch %s at %s into %s first.\n' \
        "$cosmo_repository" "$cosmo_commit" "$cosmo_source" >&2
    exit 1
fi

actual_cosmo_repository=$(git -C "$cosmo_source" remote get-url origin)
actual_cosmo_repository=${actual_cosmo_repository%.git}
expected_cosmo_repository=${cosmo_repository%.git}
if [ "$actual_cosmo_repository" != "$expected_cosmo_repository" ]; then
    printf 'Cosmopolitan repository mismatch: expected %s, found %s\n' \
        "$expected_cosmo_repository" "$actual_cosmo_repository" >&2
    exit 1
fi
actual_cosmo_commit=$(git -C "$cosmo_source" rev-parse HEAD)
if [ "$actual_cosmo_commit" != "$cosmo_commit" ]; then
    printf 'Cosmopolitan source pin mismatch: expected %s, found %s\n' \
        "$cosmo_commit" "$actual_cosmo_commit" >&2
    exit 1
fi
if [ -n "$(git -C "$cosmo_source" status --porcelain)" ]; then
    echo "Pinned Cosmopolitan source checkout is dirty: $cosmo_source" >&2
    exit 1
fi
if ! grep -F "Commit: $rust_ape_commit" "$cosmo_source/third_party/rust_ape/UPSTREAM" >/dev/null; then
    echo "The pinned Rust-APE snapshot does not match $rust_ape_commit" >&2
    exit 1
fi

export CARGO_BUILD_JOBS=1
export CMAKE_BUILD_PARALLEL_LEVEL=1
export MAKEFLAGS=-j1
export CARGO_PROFILE_RELEASE_LTO=off
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export COSMO_RUST_BUILD_ROOT="$build_root"
export COSMO_RUST_ARCH=x86_64

if [ ! -f "$sdk/generated/x86_64-unknown-linux-musl.json" ] || [ ! -x "$sdk_bin/apelink" ]; then
    bash "$run_script" setup
fi

tmp_dir=$(mktemp -d "$repo_root/o/cosmopolitan-code-build.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM
workspace="$tmp_dir/source"
mkdir -p "$workspace"
git -C "$repo_root" archive --format=tar "$source_commit" \
    code-rs codex-rs/models-manager/models.json announcement_tip.toml > "$tmp_dir/source.tar"
tar -xf "$tmp_dir/source.tar" -C "$workspace"

patch --batch -d "$workspace/code-rs" -p1 \
    < "$support_dir/overrides/cosmopolitan-musl.patch"

patch_config="patch.crates-io.rama-tls-rustls.path=\"$workspace/code-rs/vendor/rama-tls-rustls-cosmo\""
(
    cd "$workspace/code-rs"
    bash "$run_script" cargo --config "$patch_config" update \
        --manifest-path "$PWD/Cargo.toml" -p libc --precise 0.2.189
    sh "$support_dir/check-tls-backends.sh" "$PWD"
    bash "$run_script" cargo --config "$patch_config" build \
        --manifest-path "$PWD/Cargo.toml" --locked -j1 -p code-cli --bin code \
        --no-default-features --features code-cli/cosmopolitan-ape --release
)

debug_elf="$build_root/build/x86_64-unknown-linux-musl/release/code.com.dbg"
if [ ! -f "$debug_elf" ]; then
    echo "Cosmopolitan Rust executable is missing: $debug_elf" >&2
    exit 1
fi

package_dir="$tmp_dir/package"
mkdir -p "$package_dir"
artifact="$package_dir/$artifact_name"
manifest="$package_dir/$artifact_name.json"
"$sdk_bin/apelink" -l "$sdk_bin/ape-x86_64.elf" -o "$artifact" "$debug_elf"

python3 - "$pins" "$source_commit" "$artifact" "$manifest" <<'PY'
import hashlib
import json
import pathlib
import sys

pins_path, source_commit, artifact_path, manifest_path = sys.argv[1:]
pins = json.loads(pathlib.Path(pins_path).read_text(encoding="utf-8"))
artifact = pathlib.Path(artifact_path)
digest = hashlib.sha256()
with artifact.open("rb") as stream:
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(chunk)
manifest = {
    "schema_version": pins["schema_version"],
    "source_commit": source_commit,
    "source_clean": True,
    "cosmopolitan_commit": pins["cosmopolitan_commit"],
    "rust_ape_commit": pins["rust_ape_commit"],
    "target_architectures": pins["target_architectures"],
    "build_profile": pins["build_profile"],
    "release_lto": pins["release_lto"],
    "release_codegen_units": pins["release_codegen_units"],
    "artifact_name": pins["artifact_name"],
    "artifact_size_bytes": artifact.stat().st_size,
    "artifact_sha256": digest.hexdigest(),
}
pathlib.Path(manifest_path).write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY

sh "$support_dir/test-artifact.sh" "$artifact" "$manifest"
mkdir -p "$(dirname "$output_dir")"
mv "$package_dir" "$output_dir"
printf 'APE artifact: %s/%s\nManifest: %s/%s.json\n' \
    "$output_dir" "$artifact_name" "$output_dir" "$artifact_name"
