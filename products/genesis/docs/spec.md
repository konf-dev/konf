# Genesis Kell: Operational Specification

**Version:** 0.1.0
**Role:** Root Kell (CEO)

## 1. Interface (MCP)

Genesis exposes itself as an MCP server via the Konf Kernel.
-   **Transport:** stdio (tunneled via SSH).
-   **Auth:** Scoped to the 'founder' role via Tailscale IP validation (host-level).

## 2. Core Workflows (The Roadmap)

### spawn_kell
-   **Input:** name, description, required_tools.
-   **Logic:**
    1.  Create directory `products/{{name}}.kell`.
    2.  Write `project.yaml` and `tools.yaml`.
    3.  Generate a `chat.yaml` workflow using `ai:complete`.
    4.  Initialize git in the new directory (if multi-repo) or commit to parent.
    5.  Run child via shell command.

### pulse_report
-   **Input:** None.
-   **Logic:**
    1.  Gather system metrics via `system:introspect`.
    2.  Query revenue data (Stripe mock for Gen 0).
    3.  Generate a markdown report.
    4.  Commit report to `/docs/reports/`.

## 3. Infrastructure Handshake

Genesis expects the following state to be provisioned by the Phase 0 (`init`) boot:
-   **PostgreSQL:** Available at localhost:5432.
-   **Infisical:** CLI authenticated and running locally.
-   **Docker:** Available for sandboxed execution of child kells.
