#!/usr/bin/env bash
# The fixture CLI and harness share one isolated Bazel invocation boundary.
set -euo pipefail
if [[ "${1:-}" == --version ]]; then exec "${REBAZE_FIXTURE_BAZEL:?}" --version; fi
exec "${REBAZE_FIXTURE_BAZEL:?}" --batch --nosystem_rc --nohome_rc \
  "--output_user_root=${REBAZE_FIXTURE_CACHE:?}" "$@"
