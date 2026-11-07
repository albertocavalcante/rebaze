#!/usr/bin/env bash
# Verified Gradle distribution for the explicit JVM migration oracle.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=.github/scripts/download-tool.sh
source "$script_dir/download-tool.sh"
prepare_job_temp
version=9.8.2
digest=21bd5da2de49b642904077f38c0efdc7778e92b3ed0889392dca61b97e389607
setup_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-gradle.XXXXXXXX")
trap 'rm -r "$setup_dir"' EXIT
download_verified_tool "https://services.gradle.org/distributions/gradle-$version-bin.zip" "$digest" "$setup_dir/gradle.zip"
tool_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-tools"
mkdir -p "$tool_root"
unzip -q "$setup_dir/gradle.zip" -d "$tool_root"
export GRADLE_USER_HOME="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-gradle-home"
"$tool_root/gradle-$version/bin/gradle" --version
if [[ -n "${GITHUB_PATH:-}" ]]; then printf '%s\n' "$tool_root/gradle-$version/bin" >>"$GITHUB_PATH"; fi
if [[ -n "${GITHUB_ENV:-}" ]]; then printf 'GRADLE_USER_HOME=%s\n' "$GRADLE_USER_HOME" >>"$GITHUB_ENV"; fi
publish_job_temp
