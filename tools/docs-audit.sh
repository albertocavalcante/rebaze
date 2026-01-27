#!/usr/bin/env bash
# Accessibility and quality audit for rebaze docs
# Usage: ./tools/docs-audit.sh [URL]
# Requires: bun (for bunx)

set -euo pipefail

URL="${1:-http://localhost:4321/rebaze}"
REPORT_DIR="./docs-audit-reports"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== rebaze Docs Audit ===${NC}"
echo "URL: $URL"
echo "Reports: $REPORT_DIR"
echo ""

# Create report directory
mkdir -p "$REPORT_DIR"

# Check if docs server is running
if ! curl -s "$URL" > /dev/null 2>&1; then
    echo -e "${RED}Error: Docs server not running at $URL${NC}"
    echo "Start it with: cd docs && bun dev"
    exit 1
fi

echo -e "${BLUE}[1/4] Running Lighthouse audit...${NC}"
bunx lighthouse "$URL" \
    --output=json,html \
    --output-path="$REPORT_DIR/lighthouse-$TIMESTAMP" \
    --only-categories=accessibility,best-practices,seo,performance \
    --chrome-flags="--headless --no-sandbox" \
    --quiet 2>/dev/null || true

# Extract scores from JSON
if [ -f "$REPORT_DIR/lighthouse-$TIMESTAMP.report.json" ]; then
    echo ""
    echo -e "${GREEN}Lighthouse Scores:${NC}"
    bunx -y jq -r '
        "  Performance:    " + (.categories.performance.score * 100 | floor | tostring) + "%",
        "  Accessibility:  " + (.categories.accessibility.score * 100 | floor | tostring) + "%",
        "  Best Practices: " + (.categories["best-practices"].score * 100 | floor | tostring) + "%",
        "  SEO:            " + (.categories.seo.score * 100 | floor | tostring) + "%"
    ' "$REPORT_DIR/lighthouse-$TIMESTAMP.report.json"
fi

echo ""
echo -e "${BLUE}[2/4] Running Pa11y accessibility check...${NC}"
bunx pa11y "$URL" --reporter json > "$REPORT_DIR/pa11y-$TIMESTAMP.json" 2>/dev/null || true

PA11Y_COUNT=$(bunx -y jq 'length' "$REPORT_DIR/pa11y-$TIMESTAMP.json" 2>/dev/null || echo "0")
if [ "$PA11Y_COUNT" -gt 0 ]; then
    echo -e "${YELLOW}  Found $PA11Y_COUNT accessibility issues${NC}"
    echo "  Top issues:"
    bunx -y jq -r '.[0:5] | .[] | "    - " + .code + ": " + (.message | .[0:80])' "$REPORT_DIR/pa11y-$TIMESTAMP.json" 2>/dev/null || true
else
    echo -e "${GREEN}  No accessibility issues found!${NC}"
fi

echo ""
echo -e "${BLUE}[3/4] Checking internal links...${NC}"
# Simple link check using curl
BROKEN_LINKS=0
for link in $(curl -s "$URL" | grep -oE 'href="(/rebaze[^"]*)"' | sed 's/href="//;s/"$//' | sort -u | head -20); do
    FULL_URL="${URL%/rebaze}$link"
    STATUS=$(curl -s -o /dev/null -w "%{http_code}" "$FULL_URL" 2>/dev/null || echo "000")
    if [ "$STATUS" != "200" ]; then
        echo -e "  ${RED}✗ $link ($STATUS)${NC}"
        ((BROKEN_LINKS++)) || true
    fi
done
if [ "$BROKEN_LINKS" -eq 0 ]; then
    echo -e "${GREEN}  All internal links valid${NC}"
else
    echo -e "${YELLOW}  Found $BROKEN_LINKS broken links${NC}"
fi

echo ""
echo -e "${BLUE}[4/4] Checking page load performance...${NC}"
LOAD_TIME=$(curl -s -o /dev/null -w "%{time_total}" "$URL")
echo "  Page load time: ${LOAD_TIME}s"
if (( $(echo "$LOAD_TIME < 1.0" | bc -l) )); then
    echo -e "${GREEN}  Fast! Under 1 second${NC}"
elif (( $(echo "$LOAD_TIME < 2.0" | bc -l) )); then
    echo -e "${YELLOW}  Acceptable (1-2 seconds)${NC}"
else
    echo -e "${RED}  Slow! Over 2 seconds${NC}"
fi

echo ""
echo -e "${BLUE}=== Reports Generated ===${NC}"
echo "  HTML Report:  $REPORT_DIR/lighthouse-$TIMESTAMP.report.html"
echo "  JSON Report:  $REPORT_DIR/lighthouse-$TIMESTAMP.report.json"
echo "  Pa11y Report: $REPORT_DIR/pa11y-$TIMESTAMP.json"
echo ""
echo "Open HTML report: open $REPORT_DIR/lighthouse-$TIMESTAMP.report.html"
