#!/usr/bin/env bash
# Verified Temurin 21 for the required JVM migration oracle.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=.github/scripts/download-tool.sh
source "$script_dir/download-tool.sh"
prepare_job_temp
case "$(uname -s)/$(uname -m)" in
Linux/aarch64)
  architecture=aarch64
  jdk_digest=e3522e707fc7dbe2a72c30746b1e7068301a0213ecd213d4491f35a14eb55578
  ;;
Linux/x86_64)
  architecture=x64
  jdk_digest=d7f5d5cf865a2902b70ff549b97586a7d3f234e016f041ab447ea740c7d66d1c
  ;;
*)
  echo 'JVM fixture bootstrap supports Linux ARM64 and x86-64.' >&2
  exit 1
  ;;
esac
jdk_version=21.0.13
jdk_build=9
setup_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-java.XXXXXXXX")
trap 'rm -r "$setup_dir"' EXIT
jdk_archive="OpenJDK21U-jdk_${architecture}_linux_hotspot_${jdk_version}_${jdk_build}.tar.gz"
download_verified_tool "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-${jdk_version}%2B${jdk_build}/$jdk_archive" "$jdk_digest" "$setup_dir/jdk.tar.gz"

tool_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-tools"
mkdir -p "$tool_root"
tar -xzf "$setup_dir/jdk.tar.gz" -C "$tool_root"
export JAVA_HOME="$tool_root/jdk-$jdk_version+$jdk_build"
"$JAVA_HOME/bin/java" -version
if [[ -n "${GITHUB_PATH:-}" ]]; then
  printf '%s\n' "$JAVA_HOME/bin" >>"$GITHUB_PATH"
fi
if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf 'JAVA_HOME=%s\n' "$JAVA_HOME" >>"$GITHUB_ENV"
fi
publish_job_temp
