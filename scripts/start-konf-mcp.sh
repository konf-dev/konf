#!/usr/bin/env bash
# Start konf-mcp with a fresh GitHub App installation token.
# Called by .mcp.json — generates a short-lived token on each start.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Generate fresh GitHub App token (1 hour TTL)
export KONF_GITHUB_TOKEN=$(python3 "$SCRIPT_DIR/github-app-token.py")

exec "$REPO_ROOT/target/release/konf-mcp" \
  --config "$REPO_ROOT/sandbox/workspace/config"
