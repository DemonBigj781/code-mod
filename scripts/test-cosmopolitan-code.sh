#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
support_dir="$script_dir/cosmopolitan-code"

# shellcheck source-path=SCRIPTDIR
# shellcheck source=cosmopolitan-code/prepare-code-workspace.sh
. "$support_dir/prepare-code-workspace.sh"

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
for test_filter in tab_isolation session_history_worker fallback_memory_watchdog ssh_registry operator_tool agent_; do
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-core --lib \
        --no-default-features --features "$core_features" --release \
        "$test_filter" -- --test-threads=1
done
bash "$run_script" cargo --config "$patch_config" test \
    --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-session-history --lib \
    --features code-session-history/cosmopolitan-ape --release -- --test-threads=1
for test_filter in tab_isolation ssh_ operator_tool agent_shortlist settings; do
    bash "$run_script" cargo --config "$patch_config" test \
        --manifest-path "$PWD/Cargo.toml" --locked -j"$build_jobs" -p code-tui --lib \
        --no-default-features --features "$tui_features" --release \
        "$test_filter" -- --test-threads=1
done
