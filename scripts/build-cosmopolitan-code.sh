#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
support_dir="$script_dir/cosmopolitan-code"
if [ "$#" -gt 0 ]; then
    exec python3 "$support_dir/commands.py" "$@"
fi
output_dir=${COSMOPOLITAN_CODE_OUTPUT:-"$repo_root/o/cosmopolitan-code"}
if [ -e "$output_dir" ]; then
    echo "Refusing to overwrite existing output directory: $output_dir" >&2
    exit 1
fi
# shellcheck source-path=SCRIPTDIR
# shellcheck source=cosmopolitan-code/prepare-runtime.sh
. "$support_dir/prepare-runtime.sh"

tmp_dir=$(mktemp -d "$repo_root/o/cosmopolitan-code-build.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM
workspace="$tmp_dir/source"
mkdir -p "$workspace"

browser_runtime_source=${COSMOPOLITAN_BROWSER_RUNTIME_SOURCE:-"$repo_root/o/camoufox-runtime-prototype"}
browser_runtime_archive=${COSMOPOLITAN_BROWSER_RUNTIME_ARCHIVE:-}
if [ -n "$browser_runtime_archive" ]; then
    if [ ! -f "$browser_runtime_archive" ]; then
        echo "Configured browser runtime archive is missing: $browser_runtime_archive" >&2
        exit 1
    fi
elif [ -d "$browser_runtime_source" ]; then
    browser_runtime_archive="$tmp_dir/browser-runtime.zip"
    python3 "$support_dir/package-browser-runtime.py" \
        "$browser_runtime_source" "$browser_runtime_archive" \
        --pins "$support_dir/pins.json" \
        --report "$tmp_dir/browser-runtime-report.json"
else
    browser_runtime_cache=${COSMOPOLITAN_BROWSER_RUNTIME_CACHE:-"$repo_root/o/camoufox-runtime-cache"}
    browser_runtime_archive=$(PYTHONDONTWRITEBYTECODE=1 python3 \
        "$support_dir/cache_browser_runtime.py" \
        "$support_dir/pins.json" "$browser_runtime_cache")
fi

source_snapshot_sha256=
if [ "$source_mode" = working-tree ]; then
    source_snapshot_sha256=$(python3 "$support_dir/snapshot_worktree.py" "$repo_root" "$workspace")
else
    git -C "$repo_root" archive --format=tar "$source_commit" \
        code-rs codex-rs/models-manager/models.json announcement_tip.toml >"$tmp_dir/source.tar"
    tar -xf "$tmp_dir/source.tar" -C "$workspace"
fi

patch --batch -d "$workspace/code-rs" -p1 \
    <"$support_dir/overrides/cosmopolitan-musl.patch"
cp "$support_dir/Cargo.lock" "$workspace/code-rs/Cargo.lock"

patch_config="patch.crates-io.rama-tls-rustls.path=\"$workspace/code-rs/vendor/rama-tls-rustls-cosmo\""
core_features=code-core/cosmopolitan-ape,code-core/browser-automation,code-core/managed-network-proxy
tui_features="$core_features,code-tui/cosmopolitan-ape,code-tui/code-fork,code-tui/browser-automation,code-tui/managed-network-proxy"
(
    cd "$workspace/code-rs"
    sh "$support_dir/check-tls-backends.sh" "$PWD"
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-keyring-store \
        --features code-keyring-store/cosmopolitan-ape --release -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core \
        --test conversation_identity --test session_history_native --test agent_tool_contract --test operator_tools \
        --test session_background_isolation --test agent_completion_wake --no-default-features \
        --features "$core_features" --release -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        tab_isolation -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        session_history_worker -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        fallback_memory_watchdog -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        ssh_registry -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        operator_tool -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-session-history --lib \
        --features code-session-history/cosmopolitan-ape --release -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-tui --lib \
        --no-default-features --features "$tui_features" --release \
        tab_isolation -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-tui --lib \
        --no-default-features --features "$tui_features" --release \
        ssh_ -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-tui --lib \
        --no-default-features --features "$tui_features" --release \
        operator_tool -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-tui --lib \
        --no-default-features --features "$tui_features" --release \
        agent_shortlist -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-tui --lib \
        --no-default-features --features "$tui_features" --release \
        settings -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        agent_ -- --test-threads=1
    bash "$run_script" cargo --config "$patch_config" build \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-cli --bin code \
        --no-default-features --features code-cli/cosmopolitan-ape --release
)

debug_elf="$build_root/build/x86_64-unknown-linux-musl/release/code.com.dbg"
if [ ! -f "$debug_elf" ]; then
    echo "Cosmopolitan Rust executable is missing: $debug_elf" >&2
    exit 1
fi

package_dir="$tmp_dir/package"
mkdir -p "$package_dir"
cp "$runtime_smoke" "$package_dir/runtime-smoke.com"
artifact="$package_dir/$artifact_name"
manifest="$package_dir/$artifact_name.json"
"$sdk_bin/apelink" -l "$sdk_bin/ape-x86_64.elf" -o "$artifact" "$debug_elf"
cat "$browser_runtime_archive" >>"$artifact"

python3 - "$support_dir" "$artifact" "$manifest" "$sdk/generated/fork-runtime.json" \
    "$compiler_bootstrap_metadata" "$browser_runtime_archive" "$source_commit" "$source_clean" \
    "$source_snapshot_sha256" <<'PY'
import sys

support_dir, artifact, manifest, runtime, bootstrap, browser_runtime, source_commit, source_clean, snapshot = sys.argv[1:]
sys.path.insert(0, support_dir)
from artifact_contract import write_artifact_manifest

write_artifact_manifest(
    artifact,
    manifest,
    runtime,
    bootstrap,
    source_commit,
    browser_runtime_path=browser_runtime,
    source_clean=source_clean == "true",
    source_snapshot_sha256=snapshot or None,
)
PY

mkdir -p "$(dirname "$output_dir")"
mv "$package_dir" "$output_dir"
printf 'APE artifact: %s/%s\nManifest: %s/%s.json\n' \
    "$output_dir" "$artifact_name" "$output_dir" "$artifact_name"
