#!/usr/bin/env bash
# Shared download boundary. Callers select immutable URLs and committed digests.
download_verified_tool() {
  local url=$1 digest=$2 destination=$3
  curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 180 --retry 2 "$url" --output "$destination"
  if ! printf '%s  %s\n' "$digest" "$destination" | sha256sum --check --status; then
    echo 'Tool checksum mismatch; downloaded executable was not run.' >&2
    return 1
  fi
  chmod 0700 "$destination"
}

# Compilers and rustdoc use the default temp directory, not our installer path.
prepare_job_temp() {
  if [[ -n "${RUNNER_TEMP:-}" ]]; then export TMPDIR="$RUNNER_TEMP"; fi
}

publish_job_temp() {
  if [[ -n "${GITHUB_ENV:-}" && -n "${RUNNER_TEMP:-}" ]]; then
    printf 'TMPDIR=%s\n' "$TMPDIR" >>"$GITHUB_ENV"
  fi
}
