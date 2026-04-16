#!/usr/bin/env bash
# entrypoint.sh — long-running idle container for on-demand MCP servers.
#
# The container is *not* interactive. Host claude-cli spawns the actual
# MCP servers as needed via:
#
#   podman exec -i konf-repo-manager konf-mcp           # product MCP
#   podman exec -i konf-repo-manager claude mcp serve   # generic tools MCP
#
# Each such exec inherits this PID 1's env (populated from --env-file)
# and cwd. This script just:
#
#   1. Sanity-checks the bind mount.
#   2. Sets a sandbox-local git identity.
#   3. Pre-builds konf-mcp so the first MCP call doesn't pay a 2-minute
#      cargo-build hit.
#   4. Runs `sleep infinity` as PID 1 (via CMD) so exec can attach.

set -euo pipefail

KONF_ROOT="${KONF_ROOT:-/home/konf-agent/konf-dev-stack/konf}"
KONF_MCP_BIN="${KONF_ROOT}/target/release/konf-mcp"
PRODUCT_DIR="${KONF_ROOT}/products/repo-manager"

# ------------------------------------------------------------------
# Sanity
# ------------------------------------------------------------------
if [ ! -d "${KONF_ROOT}" ]; then
  echo "[entrypoint] FATAL: ${KONF_ROOT} missing — bind-mount the konf-dev-stack repo." >&2
  exit 1
fi
if [ ! -d "${PRODUCT_DIR}" ]; then
  echo "[entrypoint] FATAL: ${PRODUCT_DIR} missing." >&2
  exit 1
fi

# ------------------------------------------------------------------
# Sandbox-local git identity (self_improve commits land as this actor
# unless the host's ~/.gitconfig is bind-mounted read-only).
# ------------------------------------------------------------------
if ! git config --global user.name >/dev/null 2>&1; then
  git config --global user.name  "konf-agents[bot]"
fi
if ! git config --global user.email >/dev/null 2>&1; then
  git config --global user.email "konf-agents@users.noreply.github.com"
fi
git config --global advice.detachedHead false
git config --global --add safe.directory "${KONF_ROOT}"

# ------------------------------------------------------------------
# Pre-build konf-mcp so the first MCP call isn't a 2-minute wait.
# ------------------------------------------------------------------
if [ ! -x "${KONF_MCP_BIN}" ]; then
  echo "[entrypoint] Pre-building konf-mcp (one-time)."
  (
    cd "${KONF_ROOT}"
    cargo build --release -p konf-mcp
  )
fi

echo "[entrypoint] Ready. Container is idle — host will spawn MCP servers via 'exec -i'."
echo "[entrypoint] Tools: ${KONF_MCP_BIN}, $(command -v claude), $(command -v github-mcp-server)"

# Hand off to CMD (sleep infinity) so tini stays alive and exec has
# something to attach to.
exec "$@"
