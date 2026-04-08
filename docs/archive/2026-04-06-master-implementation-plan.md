# Konf Platform — Master Implementation Plan

**Date:** 2026-04-06 (updated for all-Rust architecture)
**Status:** Phase A+B done, Phase C in progress
**Ref:** `docs/specs/2026-04-06-konf-backend-spec-v2.md`, `docs/research/2026-04-06-rust-ecosystem-survey.md`

---

## Overview

The Konf platform consists of four packages built in four phases:

```
Phase A: Harden existing crates (smrti + konflux) — DONE
Phase B: Build konf-runtime (Rust) — DONE (39 tests)
Phase C: Build konf-backend (Rust/axum, all-in-one) — IN PROGRESS
Phase D: Unspool migration (validation)
```

### Package map

| Package | Language | What it does | Status |
|---|---|---|---|
| **smrti** | Rust + PyO3 | Graph memory, search, session state | Done (48 tests) |
| **konflux** | Rust + PyO3 | YAML workflow engine | Done (49 tests, Phase A hardened) |
| **konf-runtime** | Rust | Process management, capabilities, monitoring | Done (39 tests) |
| **konf-backend** | Rust (axum) | API server with all tools, auth, scheduling, SSE | In progress |
| ~~konf-tools~~ | ~~Python~~ | ~~Eliminated — tools moved to Rust inside konf-backend~~ | N/A |

### Dependency graph (all Rust, single binary)

```
konf-backend (Rust/axum) — the single binary
    ├── embeds konf-runtime (Rust crate, direct)
    ├── embeds konflux (Rust crate, direct)
    ├── embeds smrti (Rust crate, direct)
    ├── Core tools in Rust: rig (LLM), rmcp (MCP), reqwest (HTTP), fastembed (embed)
    ├── Custom Python tools (opt-in via PyO3 feature flag)
    └── loads project configs (YAML via figment)
```

---

## Phase A: Harden Existing Crates

**Goal:** Prepare smrti and konflux for runtime integration.
**Plan:** `docs/plans/2026-04-06-phase-a-hardening.md`

Summary of changes:
- konflux: CancellationToken, ExecutionHooks, global timeout, thread-safe registry, config exposure, YAML limits, hardcoded values → config
- smrti: expose PgPool, concurrent stress tests, operational docs
- Both: CI/CD (GitHub Actions), Cargo.toml metadata, documentation gaps

---

## Phase B: konf-runtime

**Goal:** Build the OS-like management layer.
**Plan:** `docs/plans/2026-04-06-phase-b-konf-runtime.md`

New Rust crate providing:
- ProcessTable (papaya concurrent hashmap)
- Process tree (parent-child tracking)
- Parameterized capability routing with namespace injection
- Session lifecycle (create, cancel, kill, monitor)
- Event journal (Postgres)
- Runtime hooks (connects executor to process table)
- Monitoring API (list runs, get tree, metrics)
- Python bindings via PyO3

---

## Phase C: konf-backend (Rust/axum — single binary)

**Goal:** Build the API server with all tools, auth, scheduling, and SSE.
**Spec:** `docs/specs/2026-04-06-konf-backend-spec-v2.md`
**Plan:** `docs/plans/2026-04-06-phase-c-konf-backend.md`

### All-in-one Rust binary
- axum HTTP server with SSE streaming
- Auth: JWT/JWKS verification (jsonwebtoken crate)
- Config: figment loading from YAML/TOML/env
- Core tools in Rust: memory (smrti direct), LLM (rig), HTTP (reqwest), MCP (rmcp), embeddings (fastembed)
- Scheduling: apalis with Postgres backend
- Admin + monitoring API
- Custom Python tools: opt-in via PyO3 feature flag
- Starter workflow templates embedded in binary

---

## Phase D: Unspool Migration

**Goal:** Validate the platform by migrating Unspool.
**Plan:** `docs/plans/2026-04-06-phase-d-unspool-migration.md`

Write the config directory:
- prompts/system.md, extraction.md
- workflows/context.yaml, chat.yaml, extraction.yaml, synthesis.yaml, maintenance.yaml
- tools.yaml, models.yaml, project.yaml, schedules.yaml
- Custom tools: get_plate, compute_stats

Verify all existing Unspool functionality works on Konf.

---

## Documentation to write before coding

| Document | Location | Purpose |
|---|---|---|
| YAML Workflow Schema | `docs/specs/workflow-schema.json` | Formal JSON Schema for workflow YAML |
| konf-runtime API spec | `docs/specs/2026-04-06-konf-runtime-spec.md` | Full API specification |
| konf-backend API (v2) | `docs/specs/2026-04-06-konf-backend-spec-v2.md` | Rust/axum API server |
| ~~konf-tools protocol~~ | `docs/specs/2026-04-06-konf-tools-spec.md` | SUPERSEDED (tools in Rust) |
| ~~konf-backend API (v1)~~ | `docs/specs/2026-04-06-konf-backend-spec.md` | SUPERSEDED by v2 |
| Integration guide | `docs/guides/integration.md` | How components connect (updated for all-Rust) |
| Rust ecosystem survey | `docs/research/2026-04-06-rust-ecosystem-survey.md` | Crate selection rationale |

---

## Execution strategy

All documentation and specs written first. Then:
- Phase A: Done (smrti + konflux hardened)
- Phase B: Done (konf-runtime, 39 tests)
- Phase C: konf-backend (Rust/axum) — 11 implementation phases, sequential
- Phase D: Unspool migration — depends on Phase C

**Crate dependencies:** konf-backend depends on konf-runtime, konflux, and smrti as git dependencies. Changes in smrti/konflux don't trigger konf-backend rebuilds unless the git ref changes.

**No rushing. No beta deadlines. Quality over speed.**
