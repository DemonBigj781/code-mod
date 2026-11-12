# shellcheck shell=sh
# Shared pinned-runtime and disposable Code workspace preparation.
# The variables flagged below are caller inputs or outputs intentionally exposed
# by this sourced helper.
# shellcheck disable=SC1091,SC2034,SC2154

# The caller must define repo_root and support_dir before sourcing this file.
# shellcheck source=prepare-runtime.sh
. "$support_dir/prepare-runtime.sh"

tmp_dir=$(mktemp -d "$repo_root/o/cosmopolitan-code-build.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM
workspace="$tmp_dir/source"
mkdir -p "$workspace"

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
