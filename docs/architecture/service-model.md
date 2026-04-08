# Konf Service Model — Design Exploration

**Status:** Draft (brainstorming)
**Date:** 2026-04-08
**Scope:** How Konf runs as a service, tool extensibility layers, API surface

---

## 1. What We've Converged On

### The OS Analogy Is Literal

Konf isn't "like" an OS — it IS an OS for AI agents. The architecture maps directly:

| OS concept | Konf equivalent | What it means |
|-----------|----------------|---------------|
| Kernel | konflux engine (compiled Rust) | Mechanisms: dispatch, execution, registries |
| Init system | konf-init | Reads config, boots engine, registers tools |
| Kernel modules (.ko) | WASM plugins (future) | Loadable at runtime, sandboxed, no recompilation |
| Userspace daemons | MCP servers | Out-of-process tools, any language |
| System calls | Konf API | The interface everything else builds on |
| Home directory | User namespace | User's data, preferences, credentials — their space |
| Dotfiles | User environment | Preferences that workflows read at runtime |
| Package manager | Tool registry (future) | Discover, install, update tools |

### Four-Layer Separation of Concerns

Each layer owns different dimensions. They don't override each other — they're orthogonal.

```
Layer 0: Konf Infra (kernel)
  Owns: mechanisms, core tools, resource limits, what's possible
  Ships: the compiled binary, built-in Rust tools
  Analogy: Linux kernel + built-in drivers

Layer 1: Konf Admin (sysadmin)
  Owns: platform policies, security, multi-tenancy, infrastructure config
  Configures: konf.toml, RLS, audit policies, global resource limits
  Analogy: sysadmin running the server

Layer 2: Product Admin (application developer)
  Owns: product definition — which tools, which workflows, which prompts
  Configures: config/ directory (tools.yaml, workflows/, prompts/, models.yaml)
  Can add: MCP servers, WASM plugins, connect external APIs
  Analogy: app developer shipping their app

Layer 3: End User
  Owns: their experience — preferences, credentials, activated tools
  Configures: their namespace (preferences, OAuth tokens, tool toggles)
  Can add: personal MCP servers (if product admin allows)
  Analogy: user with their home directory and dotfiles
```

### Tool Extensibility — Three Tiers

Nobody writes code against Konf. Tools are added, not coded.

```
Tier 1: Compiled Rust (in-process)
  Who adds: Infra only (part of the binary)
  Latency: ~0ms
  Analogy: built-in kernel drivers
  Examples: workflow validation, config reload, memory backends

Tier 2: WASM plugins (sandboxed, loadable)
  Who adds: Admin (drops .wasm file in plugins/)
  Latency: ~1-5ms
  Analogy: loadable kernel modules (.ko)
  Examples: custom data transforms, domain-specific tools, marketplace tools
  Language: Rust, Go, C, Python (componentize-py), JS — anything that compiles to WASM
  Sandboxed: runs in wasmtime/wasmer, capability-constrained

Tier 3: MCP servers (out-of-process)
  Who adds: Admin or User (config change)
  Latency: ~10-200ms (IPC or network)
  Analogy: userspace daemons
  Examples: Gmail, Calendar, Notion, custom scripts, any MCP-compatible service
  Language: anything (Python fastmcp, Node, Go, Rust — whatever)
```

Key rule: **the agent can't tell the difference.** All three tiers present the same Tool interface. Same metadata, same invocation path.

### User Personalization = User Environment

The user doesn't get a "settings page." The user has a namespace where their environment lives. Workflows read from it at runtime.

- Preferences (personality, tone, accessibility) → stored in user namespace
- Credentials (OAuth tokens for MCP servers) → stored in user namespace
- Tool activation (which optional tools they've enabled) → stored in user namespace
- The product defines what dimensions exist. The user fills in values.

---

## 2. Open Question: How Does Konf Run as a Service?

### The Problem

Konf needs to run as a long-lived service that:
- Starts on boot (or on demand)
- Exposes an API that everything else builds on
- Manages its own lifecycle (restart, upgrade, health)
- Is reachable by all consumers (CLI, web UI, MCP clients, other services)

### What "Service" Means — Options

#### Option A: Systemd service (Linux)
```
konf.service → runs konf-backend binary
Exposes: HTTP API on localhost:8000 + MCP over stdio/SSE
Managed by: systemd (start, stop, restart, logs via journald)
```
- Simplest for Linux servers and homelabs
- Standard for self-hosted software (Postgres, Redis, Grafana all do this)
- Socket activation possible (systemd starts Konf on first request)

#### Option B: Container (Docker/OCI)
```
konf container → runs konf-backend binary inside container
Exposes: HTTP API on mapped port + MCP
Managed by: Docker, Podman, K8s, etc.
```
- Already have Dockerfile + docker-compose
- Standard for cloud deployment
- Isolation from host

#### Option C: Launchd (macOS) / Windows Service
- Platform-specific equivalents of systemd
- Needed for desktop/laptop deployments

#### Option D: Embedded library (no separate service)
```
Your app → links konf-init as a Rust crate → engine runs in-process
```
- No network hop, no separate process
- For embedding Konf into other Rust applications

### The API Surface Question

Whatever the service model, the API is what matters. Everything builds on it:

```
                    ┌─────────────────────────┐
                    │      Konf Service        │
                    │   (however it runs)      │
                    └────────┬────────────────┘
                             │
                    ┌────────▼────────────────┐
                    │       Konf API           │
                    │  (the contract)          │
                    └────────┬────────────────┘
                             │
           ┌─────────────────┼──────────────────┐
           │                 │                  │
    ┌──────▼──────┐  ┌──────▼──────┐  ┌────────▼──────┐
    │  HTTP/REST  │  │  MCP (SSE/  │  │  Direct Rust  │
    │  transport  │  │   stdio)    │  │  API (library) │
    └─────────────┘  └─────────────┘  └───────────────┘
```

The API is the same regardless of transport. The transport is just how you reach it.

### What the API needs to cover

- Tool invocation
- Workflow execution (start, stream, cancel)
- Config management (read, write, validate, reload)
- User environment (preferences, credentials, tool activation)
- Memory operations (search, store, browse)
- Monitoring (runs, process tree, health)
- Admin (products, users, audit log)
- Auth (token exchange, session management)

### Questions to resolve:
1. Should the API be defined as a formal spec (OpenAPI) before building transports?
2. Is the HTTP API the "primary" and MCP a "secondary" transport, or are they peers?
3. Should Konf support running without any network transport (pure library mode)?
4. How do WASM plugins interact with the API? Do they go through the same dispatch as MCP?

---

## 3. What Needs To Be Built Next

Based on this model, the gaps in the current codebase:

### Already built:
- [x] Engine (konflux) — workflow execution, tool/resource/prompt registries
- [x] Runtime — process management, capabilities, namespace injection
- [x] Init system — config loading, boot sequence
- [x] HTTP transport (konf-backend) — REST API
- [x] MCP transport (konf-mcp) — MCP server
- [x] Core tools — memory, LLM, HTTP, embed, MCP client
- [x] Namespace hierarchy — konf:product:user
- [x] Capability lattice — attenuation only
- [x] Config model — platform (TOML) + product (YAML)
- [x] CI + Docker

### Needs building:
- [ ] Self-modification tools (workflow validate/write/reload, config management)
- [ ] User environment schema (how preferences/credentials/tool activation are stored)
- [ ] WASM plugin runtime (wasmtime integration, plugin loading, capability sandboxing)
- [ ] Formal API spec (OpenAPI or equivalent, transport-independent)
- [ ] Service packaging (systemd unit, launchd plist, Windows service wrapper)
- [ ] Tool marketplace / registry (discover and install MCP servers + WASM plugins)
- [ ] Admin console (Appsmith or equivalent, already specced)
- [ ] OpenTelemetry export (G3 from master plan)

---

## 4. Design Principles (additions to existing 10)

11. **Tools are added, not coded** — No stakeholder writes code against Konf. Infra ships compiled tools. Admin adds WASM plugins and MCP servers via config. Users authenticate into tools.
12. **The API is the product** — Every transport (HTTP, MCP, library) is a view of the same API. The API contract is defined independently of how you reach it.
13. **User environment, not user settings** — Users don't have a settings page. They have a namespace with their environment (preferences, credentials, tool activation). Workflows read from it.
