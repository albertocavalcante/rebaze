#!/usr/bin/env bash
# Bootstrap rustup on a minimal Linux runner, then use rust-toolchain.toml.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=.github/scripts/download-tool.sh
source "$script_dir/download-tool.sh"
prepare_job_temp

# Runner temporary storage is on its writable disk; /tmp may be a small noexec tmpfs.
if [[ -n "${RUNNER_TEMP:-}" ]]; then
  export CARGO_HOME="${CARGO_HOME:-$RUNNER_TEMP/cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$RUNNER_TEMP/rustup}"
fi
cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
export PATH="$cargo_bin:$PATH"

if ! command -v rustup >/dev/null 2>&1; then
  # Immutable rustup 1.29.3 archives; digests verified against upstream .sha256 files.
  version=1.29.3
  case "$(uname -s)/$(uname -m)" in
  Linux/aarch64)
    target=aarch64-unknown-linux-gnu
    digest=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282
    ;;
  Linux/x86_64)
    target=x86_64-unknown-linux-gnu
    digest=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
    ;;
  *)
    echo 'Rust bootstrap supports Linux ARM64 and x86-64.' >&2
    exit 1
    ;;
  esac
  setup_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-rustup.XXXXXXXX")
  trap 'rm -r "$setup_dir"' EXIT
  installer="$setup_dir/rustup-init"
  download_verified_tool "https://static.rust-lang.org/rustup/archive/$version/$target/rustup-init" "$digest" "$installer"
  "$installer" -y --no-modify-path --default-toolchain none --profile minimal
fi

rustup toolchain install --no-self-update
# Publish only after successful setup. The current step already uses PATH above.
if [[ -n "${GITHUB_PATH:-}" ]]; then
  printf '%s\n' "$cargo_bin" >>"$GITHUB_PATH"
fi
if [[ -n "${GITHUB_ENV:-}" && -n "${RUNNER_TEMP:-}" ]]; then
  printf 'CARGO_HOME=%s\nRUSTUP_HOME=%s\n' "$CARGO_HOME" "$RUSTUP_HOME" >>"$GITHUB_ENV"
fi

publish_job_temp
