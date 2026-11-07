#!/usr/bin/env bash
# Site jobs use the fixed Linux x64 hosted runner; verify Bun before extraction.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=.github/scripts/download-tool.sh
source "$script_dir/download-tool.sh"
prepare_job_temp
if [[ "$(uname -s)/$(uname -m)" != Linux/x86_64 ]]; then
  echo 'Site bootstrap requires Linux x86-64.' >&2
  exit 1
fi
version=1.4.4
digest=bf8ccbfb2b0b80159e0f9a6d15c55317add8077534dbc6130a3edb52fccb395c
setup_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-bun.XXXXXXXX")
trap 'rm -r "$setup_dir"' EXIT
download_verified_tool "https://github.com/oven-sh/bun/releases/download/bun-v$version/bun-linux-x64.zip" "$digest" "$setup_dir/bun.zip"
unzip -q "$setup_dir/bun.zip" -d "$setup_dir"
install_dir="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-bun-$version"
mkdir -p "$install_dir"
cp "$setup_dir/bun-linux-x64/bun" "$install_dir/bun"
chmod 0700 "$install_dir/bun"
"$install_dir/bun" --version
if [[ -n "${GITHUB_PATH:-}" ]]; then printf '%s\n' "$install_dir" >>"$GITHUB_PATH"; fi
publish_job_temp
