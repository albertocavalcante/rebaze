#!/usr/bin/env bash
# Build script for GitHub Pages static export
# Temporarily disables incompatible features (dynamic API routes)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCS_DIR="$(dirname "$SCRIPT_DIR")"
BACKUP_DIR="$DOCS_DIR/.static-export-backup"

# Routes incompatible with static export
DISABLED_ROUTES=(
  "src/app/api/search"
  "src/app/llms.mdx"
)

cleanup() {
  # Restore disabled routes
  for route in "${DISABLED_ROUTES[@]}"; do
    route_path="$DOCS_DIR/$route"
    backup_path="$BACKUP_DIR/$route"
    if [[ -d "$backup_path" ]]; then
      echo "Restoring $route..."
      rm -rf "$route_path"
      mkdir -p "$(dirname "$route_path")"
      mv "$backup_path" "$route_path"
    fi
  done
  rm -rf "$BACKUP_DIR"
}

trap cleanup EXIT

echo "Building for GitHub Pages (static export)..."

# Temporarily disable incompatible routes
mkdir -p "$BACKUP_DIR"
for route in "${DISABLED_ROUTES[@]}"; do
  route_path="$DOCS_DIR/$route"
  if [[ -d "$route_path" ]]; then
    echo "Temporarily disabling $route..."
    backup_path="$BACKUP_DIR/$route"
    mkdir -p "$(dirname "$backup_path")"
    mv "$route_path" "$backup_path"
  fi
done

# Build with static export
cd "$DOCS_DIR"
GITHUB_PAGES=true bun run build

echo "Build complete! Output in: $DOCS_DIR/out"
