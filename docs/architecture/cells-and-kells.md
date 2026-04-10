# Cells and Kells: The Anatomy of a Deployment

**Status:** Authoritative
**Scope:** Understanding the difference between Reference Kells, Living Cells, and how they map to infrastructure.

---

## 1. The Core Concepts

Konf OS enforces a strict separation between the engine that runs logic (The Kernel) and the configuration that defines behavior (The Kells).

### What is a Kell?
A **Kell** is a single computational boundary. Physically, it is a directory containing a `project.yaml`, `tools.yaml`, workflows, and prompts. Logically, it is a "smart bubble" that defines an agent's identity, permissions, resource limits, and behaviors.

*   **Stateless Execution:** Kells do not hold runtime state across restarts. If the process dies, it wakes up, reads its persistent memory (e.g., Postgres/pgvector), and starts fresh.
*   **Security Membrane:** A Kell cannot perform actions outside its defined capabilities. The Kernel enforces this isolation through namespace injection.

### What is a Cell?
A **Cell** is a collection of fundamental, minimal, and generic Kells that are deployed and run together within a single environment (like a VPS or a cluster).
*   Instead of building monolithic "super agents," Konf encourages composing simple Kells together.
*   A Cell represents a complete, living deployment.

---

## 2. Reference Kells vs. Living Cells

It is crucial to distinguish between the Kells provided by the Konf Kernel and the Kells you build to run your organization.

### Reference Kells (The "System Apps")
Located in the core repository under `konf/products/`. These are static, "factory-installed" templates and utilities provided by the platform.

*   **`init` (The Bootloader):** The PID 1 Kell. Its sole job is to provision the basic infrastructure (databases, secret vaults) before any other product loads.
*   **`devkit` / `architect`:** Specialized toolbelts for developers. They operate close to the metal to run tests, commit code, and validate YAML.
*   **`assistant`:** A generic template demonstrating the standard "Search Memory -> Think -> Respond" loop.

Changing these Kells usually implies you are improving the underlying Konf platform itself.

### Living Cells (Your "User Space")
Located in independent repositories (e.g., `konf-genesis.kell`). This is where your actual company or project lives.

*   **The Operator (Genesis):** The root-level Kell in your production Cell. It sits at the console, manages the sandbox ("The Construct"), monitors system health, and coordinates the deployment of specialized operatives (other Kells).
*   **Operational History:** A Living Cell generates logs, writes its own files, and commits its "thoughts" back to its repository. Keeping this separate from the core Kernel repo ensures the platform code remains pure and secure.

---

## 3. The Matrix Analogy

To solidify the mental model, Konf adopts naming conventions inspired by *The Matrix*:

*   **Zion (The Metal):** The underlying hardware (VPS, Proxmox cluster).
*   **The Hovercraft (The Kernel):** The Konf OS execution environment providing life support and tool access.
*   **The Construct (The Sandbox):** A secure, isolated Podman/Docker container where untrusted or generated code is executed safely (`shell:exec`).
*   **The Hardline:** The secure Tailscale network membrane connecting the host to the outside world.
*   **The Operator:** The primary Kell (Genesis) that manages the Construct, loads programs (tools), and acts as the single interface for the Founder.
*   **Operatives:** The specialized, minimal Kells spawned by The Operator to perform specific missions.
