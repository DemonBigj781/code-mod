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
. "$support_dir/prepare-runtime.sh"

tmp_dir=$(mktemp -d "$repo_root/o/cosmopolitan-code-build.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM
workspace="$tmp_dir/source"
mkdir -p "$workspace"
git -C "$repo_root" archive --format=tar "$source_commit" \
    code-rs codex-rs/models-manager/models.json announcement_tip.toml >"$tmp_dir/source.tar"
tar -xf "$tmp_dir/source.tar" -C "$workspace"

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

python3 - "$pins" "$source_commit" "$artifact" "$manifest" "$sdk/generated/fork-runtime.json" <<'PY'
import hashlib
import json
import pathlib
import sys

pins_path, source_commit, artifact_path, manifest_path, runtime_path = sys.argv[1:]
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
    "cosmopolitan_runtime": json.loads(pathlib.Path(runtime_path).read_text(encoding="utf-8")),
    "target_architectures": pins["target_architectures"],
    "build_profile": pins["build_profile"],
    "release_opt_level": pins["release_opt_level"],
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
python3 "$support_dir/check-history-worker.py" "$artifact"
mkdir -p "$(dirname "$output_dir")"
mv "$package_dir" "$output_dir"
printf 'APE artifact: %s/%s\nManifest: %s/%s.json\n' \
    "$output_dir" "$artifact_name" "$output_dir" "$artifact_name"
