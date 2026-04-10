# Genesis Kell: Operational Specification

**Version:** 0.1.0
**Role:** Root Kell (Second-in-Command)

## 1. Interface (MCP)

Genesis exposes itself as an MCP server via the Konf Kernel.
-   **Transport:** stdio (tunneled via SSH).
-   **Auth:** Scoped to the 'founder' role via Tailscale IP validation (host-level).
-   **Execution:** Remote connection via OpenCode or other MCP clients.

## 2. Core Workflows (The Roadmap)

### spawn_kell
-   **Input:** name, description, required_tools.
-   **Logic:**
    1.  Create directory `products/{{name}}.kell`.
    2.  Write `project.yaml` and `tools.yaml`.
    3.  Generate initial workflows using `ai:complete`.
    4.  Commit to parent repository.
    5.  Run child via shell command.

### pulse_report
-   **Input:** None.
-   **Logic:**
    1.  Gather system metrics via `system:introspect`.
    2.  Generate a markdown report.
    3.  Commit report to `/docs/reports/`.

## 3. Infrastructure Handshake

Genesis expects the following state:
-   **Podman:** Installed and aliased to `docker` for containerized operations.
-   **Infisical:** CLI authenticated and running locally for secret injection.
-   **LiteLLM/Ollama:** Accessible for local/free LLM tasks.
-   **OpenCode Zen:** Configured for high-level reasoning.
