#!/usr/bin/env bash
# Pinned oracle tool for the explicit native migration fixture.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=.github/scripts/download-tool.sh
source "$script_dir/download-tool.sh"
prepare_job_temp
version=3.31.10
case "$(uname -s)/$(uname -m)" in
Linux/aarch64)
  platform=linux-aarch64
  digest=a343c6294f770742904e6a6792e0956b5ff8212abfb63cac99237de2e210fa0f
  ;;
Linux/x86_64)
  platform=linux-x86_64
  digest=3cb3dd247b6a1de2d0f4b20c6fd4326c9024e894cebc9dc8699758887e566ca7
  ;;
*)
  echo 'CMake bootstrap supports Linux ARM64 and x86-64.' >&2
  exit 1
  ;;
esac
setup_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-cmake.XXXXXXXX")
trap 'rm -r "$setup_dir"' EXIT
archive="cmake-$version-$platform.tar.gz"
download_verified_tool "https://github.com/Kitware/CMake/releases/download/v$version/$archive" "$digest" "$setup_dir/$archive"
tool_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-tools"
mkdir -p "$tool_root"
tar -xzf "$setup_dir/$archive" -C "$tool_root"
tool_bin="$tool_root/cmake-$version-$platform/bin"
"$tool_bin/cmake" --version
"$tool_bin/ctest" --version
if [[ -n "${GITHUB_PATH:-}" ]]; then printf '%s\n' "$tool_bin" >>"$GITHUB_PATH"; fi
publish_job_temp
