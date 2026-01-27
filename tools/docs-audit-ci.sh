#!/usr/bin/env bash
# Quick accessibility check for CI pipelines
# Returns exit code 1 if accessibility score < 90%
# Usage: ./tools/docs-audit-ci.sh [URL]

set -euo pipefail

URL="${1:-http://localhost:4321/rebaze}"
MIN_SCORE="${MIN_ACCESSIBILITY_SCORE:-90}"

echo "Auditing: $URL"
echo "Minimum accessibility score: $MIN_SCORE%"

# Run Lighthouse and extract accessibility score
RESULT=$(bunx lighthouse "$URL" \
    --output=json \
    --only-categories=accessibility \
    --chrome-flags="--headless --no-sandbox" \
    --quiet 2>/dev/null)

SCORE=$(echo "$RESULT" | bunx -y jq '.categories.accessibility.score * 100 | floor')

echo "Accessibility score: $SCORE%"

if [ "$SCORE" -lt "$MIN_SCORE" ]; then
    echo "FAIL: Score $SCORE% is below minimum $MIN_SCORE%"
    exit 1
else
    echo "PASS: Score $SCORE% meets minimum $MIN_SCORE%"
    exit 0
fi
