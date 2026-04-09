# Architecture: The Init Kell (Phase 0 Boot)

**Status:** Proposed / Experimental
**Concept:** PID 1 for Konf OS
**Reference:** [Init Specification](init.md) (for Rust-level bootstrap)

---

## The Philosophy: Kernel vs. User Space

Konf OS distinguishes between the **Kernel** (the Rust binary) and the **User Space** (the Kells/Products).

| Component | Layer | Analogy | Responsibility |
| :--- | :--- | :--- | :--- |
| **`konf-init`** | Kernel | Linux Kernel Boot | Loading config, initializing Rust registries, starting the engine. |
| **`init` Kell** | User Space | `systemd` / `PID 1` | Provisioning external services (Docker, DBs, Secrets), health checks. |

The "Init Kell" is the first product to boot. It is a **Reference Configuration** that manages the environment where all other kells live.

---

## The Boot Sequence (The "Phase 0" Handshake)

1.  **Binary Execution:** The operator runs `konf --product init`.
2.  **Kernel Bootstrap:** `konf-init` (Rust) loads the `init` product config. It has a "thin" set of tools (stdlib): `shell:exec`, `http:get`, `state:set`.
3.  **PID 1 Entry:** The `init` product's entry workflow (e.g., `boot.yaml`) is executed.
4.  **External Provisioning:**
    -   Calls `shell:exec` to run `docker-compose` for Infisical, Postgres, etc.
    -   Calls `http:get` to wait for service health.
    -   Calls `state:set` to mark the infrastructure as "Ready."
5.  **Capability Handover:** Once the infrastructure is ready, other kells (like `assistant` or `devkit`) can be started. They use the secrets and databases provisioned in Phase 0.

---

## Generic Secret Interface (The Standard Library)

To avoid baking specific providers (like Infisical) into the kernel, Konf provides a **Generic Secret Interface**.

- **Interface:** Tools named `secret:get`, `secret:set`, `secret:list`.
- **Default Backend:** Local process environment variables (via `interpolate_env_vars`).
- **Plugin Backend:** An MCP adapter (e.g., `infisical-mcp`) that maps the generic `secret:*` names to a specific provider.

The `init` product is responsible for configuring this mapping in `tools.yaml`.

---

## Why This Architecture?

1.  **Immutability:** The kernel never changes when you switch from Infisical to Vault. Only the `init` product config changes.
2.  **Freedom:** Users can replace the "Standard Init" with their own custom bootstrap logic.
3.  **Security:** The `init` product can have elevated `shell:exec` permissions (to manage Docker), while standard user products are restricted.
4.  **Portability:** The same `init` product can handle `local` vs `cloud` deployment by simply branching in its workflow logic.
