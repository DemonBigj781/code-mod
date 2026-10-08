#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
workspace=${1:-$script_dir/code-rs}
run_script=${COSMOPOLITAN_RUST_APE_RUN:-$script_dir/o/cosmopolitan-source/third_party/rust_ape/run.sh}
build_root=${COSMO_RUST_BUILD_ROOT:-$script_dir/o/rust-ape}
patched_rama="$script_dir/code-rs/vendor/rama-tls-rustls-cosmo"

if [ -f "$patched_rama/Cargo.toml" ]; then
  patch_config="patch.crates-io.rama-tls-rustls.path=\"$patched_rama\""
  tree=$(CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$build_root" \
    bash "$run_script" cargo --config "$patch_config" tree \
      --manifest-path "$workspace/Cargo.toml" --locked \
      --no-default-features --features code-cli/cosmopolitan-ape -p code-cli -e normal)
  ring_tree=$(CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$build_root" \
    bash "$run_script" cargo --config "$patch_config" tree \
      --manifest-path "$workspace/Cargo.toml" --locked \
      --no-default-features --features code-cli/cosmopolitan-ape -p code-cli -e normal -i ring)
  zstd_tree=$(CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$build_root" \
    bash "$run_script" cargo --config "$patch_config" tree \
      --manifest-path "$workspace/Cargo.toml" --locked \
      --no-default-features --features code-cli/cosmopolitan-ape -p code-cli -e features -i zstd-sys)
else
  tree=$(CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$build_root" \
    bash "$run_script" cargo tree --manifest-path "$workspace/Cargo.toml" \
      --locked --no-default-features --features code-cli/cosmopolitan-ape -p code-cli -e normal)
  ring_tree=$(CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$build_root" \
    bash "$run_script" cargo tree --manifest-path "$workspace/Cargo.toml" \
      --locked --no-default-features --features code-cli/cosmopolitan-ape -p code-cli -e normal -i ring)
  zstd_tree=$(CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$build_root" \
    bash "$run_script" cargo tree --manifest-path "$workspace/Cargo.toml" \
      --locked --no-default-features --features code-cli/cosmopolitan-ape -p code-cli -e features -i zstd-sys)
fi

case "$tree" in
  *'aws-lc-sys v'*)
    echo 'Cosmopolitan CLI dependency graph still includes unsupported AWS-LC C code.' >&2
    exit 1
    ;;
esac

case "$tree" in
  *'openssl-sys v'*)
    echo 'Cosmopolitan CLI dependency graph still includes the native-musl OpenSSL link-forcing dependency.' >&2
    exit 1
    ;;
esac

case "$tree" in
  *'wayland-sys v'*)
    echo 'Cosmopolitan CLI dependency graph still includes Linux host Wayland dialog libraries.' >&2
    exit 1
    ;;
esac

case "$ring_tree" in
  *'ring v'*) ;;
  *)
    echo 'Cosmopolitan CLI dependency graph does not include the supported Rustls ring backend.' >&2
    exit 1
    ;;
esac

case "$zstd_tree" in
  *'zstd-sys feature "no_asm"'*) ;;
  *)
    echo 'Cosmopolitan CLI dependency graph must enable zstd-sys/no_asm for the portable C implementation.' >&2
    exit 1
    ;;
esac

echo 'Cosmopolitan CLI uses ring, excludes AWS-LC, and disables zstd assembly.'
