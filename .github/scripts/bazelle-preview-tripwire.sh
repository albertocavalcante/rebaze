#!/usr/bin/env bash
# Preview must inspect tool presence without executing this program.
set -euo pipefail
: >"${REBAZE_PREVIEW_PROBE_MARKER:?}"
exit 1
