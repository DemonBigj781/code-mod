# shellcheck shell=sh
# Shared source validation, resource limits, and runtime preparation.
pins="$support_dir/pins.json"
cosmo_source="$repo_root/o/cosmopolitan-source"
run_script="$cosmo_source/third_party/rust_ape/run.sh"

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
build_root=${COSMO_RUST_BUILD_ROOT:-"$repo_root/o/rust-ape-source-$cosmo_commit-$rust_ape_commit"}
sdk="$build_root/sdk"
sdk_bin="$sdk/vendor/cosmocc/bin"
compiler_bootstrap_metadata="$sdk/generated/local-cosmocc-bootstrap.json"
local_cosmo_root=${COSMOPOLITAN_SOURCE_ROOT:-"$HOME/projects/cosmo"}
local_compiler_archive=${COSMOPOLITAN_BOOTSTRAP_SDK:-"$local_cosmo_root/o/toolchain/compiler-payload/cosmocc-sdk.zip"}
local_compiler_provenance=${COSMOPOLITAN_BOOTSTRAP_PROVENANCE:-"$(dirname "$local_compiler_archive")/BUILD.json"}
source_commit=$(git -C "$repo_root" rev-parse HEAD)
source_mode=${COSMOPOLITAN_CODE_SOURCE_MODE:-clean}

case "$source_mode" in
    clean)
        source_clean=true
        if [ -n "$(git -C "$repo_root" status --porcelain)" ]; then
            echo "Refusing to build a prerelease from a dirty source tree." >&2
            exit 1
        fi
        ;;
    working-tree)
        source_clean=false
        ;;
    *)
        echo "Unsupported Cosmopolitan Code source mode: $source_mode" >&2
        exit 1
        ;;
esac

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

build_jobs=1
if [ "${GITHUB_ACTIONS:-}" = true ] && [ "${RUNNER_ENVIRONMENT:-}" = github-hosted ]; then
    build_jobs=$(getconf _NPROCESSORS_ONLN)
fi
export CARGO_BUILD_JOBS="$build_jobs"
export CMAKE_BUILD_PARALLEL_LEVEL="$build_jobs"
export MAKEFLAGS="-j$build_jobs"
export CARGO_PROFILE_RELEASE_LTO=off
export CARGO_PROFILE_RELEASE_OPT_LEVEL=2
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export COSMO_RUST_BUILD_ROOT="$build_root"
export COSMO_RUST_ARCH=x86_64

runtime_output="$repo_root/o/cosmopolitan-runtime"
runtime_smoke="$runtime_output/runtime-smoke.com"
prepare_runtime() {
    if [ -e "$runtime_output" ]; then
        python3 "$support_dir/verify_runtime_probe.py" "$runtime_output" "$source_commit"
        if [ ! -f "$sdk/generated/fork-runtime.json" ]; then
            echo "The cached runtime SDK is missing; use a fresh build directory." >&2
            exit 1
        fi
        python3 "$support_dir/build_fork_runtime.py" "$cosmo_source" "$sdk" "$cosmo_commit" "$build_jobs"
        cmp "$runtime_output/fork-runtime.json" "$sdk/generated/fork-runtime.json"
        return
    fi
    if [ ! -f "$sdk/generated/x86_64-unknown-linux-musl.json" ] || \
        [ ! -x "$sdk_bin/apelink" ] || [ ! -f "$compiler_bootstrap_metadata" ]; then
        python3 "$support_dir/stage_local_cosmocc.py" \
            "$local_compiler_archive" "$local_compiler_provenance" \
            "$cosmo_source/third_party/rust_ape/upstream/xtask/src/config.rs" \
            "$pins" "$sdk"
        bash "$run_script" setup
    fi

    python3 "$support_dir/build_fork_runtime.py" "$cosmo_source" "$sdk" "$cosmo_commit" "$build_jobs"
    bash "$run_script" gen-shim
    python3 "$support_dir/check_fork_shims.py" "$sdk"

    runtime_tmp=$(mktemp -d "$repo_root/o/cosmopolitan-runtime-build.XXXXXX")
    trap 'rm -rf "$runtime_tmp"' EXIT HUP INT TERM
    cp -R "$support_dir/fixtures/runtime-smoke" "$runtime_tmp/runtime-smoke"
    bash "$run_script" cargo build --manifest-path "$runtime_tmp/runtime-smoke/Cargo.toml" \
        -j"$build_jobs" --release
    probe="$runtime_tmp/probe"
    mkdir -p "$probe"
    runtime_smoke="$probe/runtime-smoke.com"
    "$sdk_bin/apelink" -l "$sdk_bin/ape-x86_64.elf" -o "$runtime_smoke" \
        "$build_root/build/x86_64-unknown-linux-musl/release/cosmo-runtime-smoke.com.dbg"
    sh "$runtime_smoke"
    cp "$sdk/generated/fork-runtime.json" "$probe/fork-runtime.json"
    printf '%s\n' "$source_commit" >"$probe/code-source-commit"
    (cd "$probe" && sha256sum runtime-smoke.com >runtime-smoke.sha256)
    python3 "$support_dir/verify_runtime_probe.py" "$probe" "$source_commit"
    mv "$probe" "$runtime_output"
    rm -rf "$runtime_tmp"
    trap - EXIT HUP INT TERM
    runtime_smoke="$runtime_output/runtime-smoke.com"
}
prepare_runtime
