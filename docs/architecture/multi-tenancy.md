# Konf Multi-Tenancy & Permissions

**Status:** Authoritative
**Scope:** Namespace hierarchy, capability lattice, actor roles, audit logging

> **Note:** Multi-tenancy is enforced at the tool level via VirtualizedTool namespace injection, not at the database level. This makes it backend-agnostic — any memory backend (Postgres, SurrealDB, SQLite) works with the same permission model. See [overview.md](overview.md) for platform context.

---

## 1. The Problem

The platform serves multiple stakeholders with different access needs. Without a clear permission model, we'll either be too permissive (security risk) or too restrictive (usability problem). Every platform we studied (Supabase, Grafana, GitLab, Temporal) faced this — and the ones that delayed RBAC regretted it.

**Key insight from research:** Build RBAC into self-hosted from day one. Temporal and Grafana both ship self-hosted with weak/no auth, then add it as a paid cloud feature. Users hate this. We won't do that.

---

## 2. Stakeholder Hierarchy

Six roles across three levels, each with a human and an agent counterpart:

```
Level 0: Infrastructure
├── Infra Admin (human) — runs the deployment
└── Infra Agent (system) — manages platform health, cross-product operations

Level 1: Product
├── Product Admin (human) — manages one product (e.g., Unspool)
└── Product Agent (system) — gateway agent for the product

Level 2: User
├── User (human) — end user of a product
└── User Agent (system) — agent serving one user
```

### What each role can do

| | Config (read) | Config (write) | User data (read) | User data (write) | Memory (read) | Memory (write) | Workflows | Monitoring | Users |
|---|---|---|---|---|---|---|---|---|---|
| **Infra Admin** | All | All | All (audited) | All (audited) | All (audited) | All (audited) | All | All | Manage all |
| **Product Admin** | Own product | Own product | Own product users (audited) | Own product users (audited) | Own product namespaces | Own product namespaces | Own product | Own product | Manage product users |
| **User** | Own preferences | Own preferences | Own data | Own data | Own namespace | Own namespace | Trigger allowed | Own sessions | Self only |
| **Infra Agent** | All | System config only | Cross-product (scoped) | Cross-product (scoped) | As granted | As granted | System workflows | All | None |
| **Product Agent** | Own product | Runtime only | Product users (scoped) | Product users (scoped) | Product namespaces | Product namespaces | Product workflows | Product | None |
| **User Agent** | None | None | Own user | Own user | Own namespace | Own namespace | User-scoped | Own session | None |

**Key principle: admin access to user data is always audited.** An infra admin CAN read user memory — but the action is logged with who, what, when, and why.

---

## 3. Namespace Hierarchy

Namespaces are hierarchical, not flat. This enables permission scoping at any level.

```
konf                           ← infrastructure root
├── konf:unspool               ← product namespace
│   ├── konf:unspool:user_123  ← user namespace
│   ├── konf:unspool:user_456
│   └── konf:unspool:shared    ← shared product knowledge base
├── konf:fitness_app
│   ├── konf:fitness_app:user_789
│   └── konf:fitness_app:shared
└── konf:system                ← platform system data
```

**Rules:**
- A capability grant for `konf:unspool:*` matches all namespaces under that product
- A grant for `konf:unspool:user_123` matches only that user
- A grant for `konf:*` matches everything (infra admin level)
- Namespace isolation is enforced at the memory backend level (every query includes `WHERE namespace = $1` or `WHERE namespace LIKE $1 || '%'`)

**This is the single most important architectural decision.** Hierarchical namespaces mean we never have to rework data isolation when adding new permission levels. Everything flows from the namespace.

---

## 4. Data Access Control

### 4.1 Database-level isolation (RLS)

Following Supabase's model — RLS is the gold standard because it's enforced by Postgres, not application code.

The memory backend already isolates by namespace in every query. But currently namespace is passed as a parameter — the application could pass the wrong one. With RLS:

```sql
-- Enable RLS on all memory tables
ALTER TABLE nodes ENABLE ROW LEVEL SECURITY;
ALTER TABLE edges ENABLE ROW LEVEL SECURITY;
ALTER TABLE events ENABLE ROW LEVEL SECURITY;
ALTER TABLE session_state ENABLE ROW LEVEL SECURITY;

-- Policy: users can only access their own namespace
CREATE POLICY namespace_isolation ON nodes
    USING (namespace = current_setting('app.namespace', true));
```

**Why this matters:** Even if a bug in the runtime passes the wrong namespace, Postgres blocks the access. Defense in depth.

**Implementation note — SET LOCAL, not after_connect:**

RLS context MUST be set per-transaction using `SET LOCAL` (or `set_config(..., true)`), NOT via `after_connect` hooks. With connection pooling, `after_connect` runs once when a connection is created — subsequent users reusing that connection would see stale namespace settings.

The correct pattern (following Supabase's approach):

```rust
// The memory backend wraps every operation in a transaction with namespace set
let mut tx = pool.begin().await?;
sqlx::query("SELECT set_config('app.namespace', $1, true)")  // true = transaction-local
    .bind(&namespace)
    .execute(&mut *tx)
    .await?;
// ... execute queries within tx ...
tx.commit().await?;
// On commit, set_config is automatically cleared — connection returns to pool clean
```

This is how Supabase handles RLS with PgBouncer in transaction mode. The third parameter `true` to `set_config` makes it transaction-local, automatically reset on COMMIT/ROLLBACK.

### 4.2 Application-level access control

The runtime enforces capability grants before tool dispatch (already designed). The new addition is **parameterized namespace binding**:

```yaml
# Product agent's scope:
capabilities:
  - pattern: "memory_*"
    bindings: { namespace: "konf:unspool:*" }  # wildcard = all users in product

# User agent's scope:
capabilities:
  - pattern: "memory_*"
    bindings: { namespace: "konf:unspool:user_123" }  # exact = one user only
```

The runtime's `VirtualizedTool` injects the bound namespace before every tool call. The LLM never sees it. Cannot be overridden.

### 4.3 Config access control

Configs are files on disk (or in a database for managed mode). Access control:

| Config type | Who can read | Who can write |
|---|---|---|
| `project.yaml` | Product admin, infra admin | Product admin, infra admin |
| `prompts/*.md` | Product admin, product agent | Product admin |
| `workflows/*.yaml` | Product admin, product agent | Product admin |
| `tools.yaml` | Product admin | Product admin |
| `models.yaml` | Product admin | Product admin |
| User preferences | User, product admin (audited) | User |

For self-hosted v1, config files are on disk — whoever has filesystem access can modify them. The admin console provides a UI layer with auth checks, but the filesystem is the source of truth.

For managed/multi-product, configs move to a database table with proper RBAC.

---

## 5. Audit Log

**Every data access by a higher-privilege entity is logged.** This is non-negotiable.

```sql
CREATE TABLE audit_log (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    actor_type TEXT NOT NULL,          -- 'human', 'agent', 'system'
    actor_id TEXT NOT NULL,            -- user_id, agent_id, or 'system'
    actor_role TEXT NOT NULL,          -- 'infra_admin', 'product_admin', 'user', etc.
    action TEXT NOT NULL,              -- 'read', 'write', 'delete', 'config_change', 'impersonate'
    resource_type TEXT NOT NULL,       -- 'memory', 'config', 'workflow', 'user', 'session'
    resource_id TEXT,                  -- namespace, config path, workflow_id
    target_namespace TEXT,             -- which namespace was accessed
    details JSONB,                     -- action-specific context
    ip_address TEXT,
    config_version TEXT                -- which config version was active
);

CREATE INDEX idx_audit_actor ON audit_log (actor_id, timestamp DESC);
CREATE INDEX idx_audit_namespace ON audit_log (target_namespace, timestamp DESC);
CREATE INDEX idx_audit_action ON audit_log (action, timestamp DESC);
```

**What's logged:**
- Admin reads user memory → `{action: "read", resource_type: "memory", target_namespace: "konf:unspool:user_123", actor_role: "infra_admin"}`
- Config change → `{action: "config_change", details: {file: "prompts/system.md", diff_hash: "abc123"}}`
- Workflow cancellation → `{action: "cancel", resource_type: "workflow", resource_id: "run_abc"}`
- User data deletion (GDPR) → `{action: "delete", resource_type: "namespace", target_namespace: "konf:unspool:user_123"}`

**Who can read audit logs:**
- Infra admin: all logs
- Product admin: logs for their product's namespaces
- User: their own logs only

---

## 6. Agent Identity

Agents need identities so their actions can be audited and scoped.

```yaml
# Agent identity is defined in the execution scope
scope:
  namespace: "konf:unspool:user_123"
  agent_id: "user_agent:user_123:sess_abc"
  agent_role: "user_agent"
  capabilities: [...]
```

When an agent calls a tool, the runtime attaches `agent_id` and `agent_role` to the tool context metadata. If the tool modifies data (memory_store, config:update), the audit log records who did it.

**Agent roles map to the same hierarchy:**
- `infra_agent` → can access `konf:*`
- `product_agent` → can access `konf:unspool:*`
- `user_agent` → can access `konf:unspool:user_123`

---

## 7. Admin Console

### What it needs to do

| Feature | Infra Admin | Product Admin | User |
|---|---|---|---|
| **Process tree** | All products, all sessions | Product sessions | Own session |
| **Logs** | All audit logs, all runtime events | Product logs | Own logs |
| **Config editor** | All configs | Product configs | Preferences |
| **User management** | All users, all products | Product users | Self |
| **Memory browser** | All namespaces (audited) | Product namespaces | Own namespace |
| **Workflow monitor** | All running workflows | Product workflows | Own session |
| **Metrics** | Platform-wide | Product-wide | N/A |

### Implementation: Don't build a custom UI

**Recommendation: Use Appsmith (Apache 2.0, self-hostable).**

Appsmith connects to Postgres directly, has built-in auth, and lets you build admin pages visually. We expose the data via:
- Postgres tables (audit_log, runtime_events, scheduled_jobs)
- Backend API endpoints (`/v1/admin/runs`, `/v1/admin/users`, `/v1/admin/config`)
- memory queries (memory browsing)

Appsmith reads from these and provides the UI. No custom frontend framework needed.

For the dogfooding scenario (admin console powered by konflux itself), this is a v2 feature — get the basic admin working first with Appsmith, then build the AI-powered admin later.

### API endpoints for admin

```
# Admin (infra admin only)
GET    /v1/admin/products              ← list products
GET    /v1/admin/users                 ← list all users
GET    /v1/admin/audit                 ← query audit log
GET    /v1/admin/metrics               ← platform metrics

# Product admin
GET    /v1/admin/product/{id}/users    ← product users
GET    /v1/admin/product/{id}/config   ← product config
PUT    /v1/admin/product/{id}/config   ← update config
GET    /v1/admin/product/{id}/runs     ← active workflows
GET    /v1/admin/product/{id}/audit    ← product audit log

# Monitoring (scoped by role)
GET    /v1/monitor/runs                ← list runs (scoped)
GET    /v1/monitor/runs/{id}           ← run detail
GET    /v1/monitor/runs/{id}/tree      ← process tree
GET    /v1/monitor/metrics             ← runtime metrics

# Memory browsing (audited)
GET    /v1/memory/browse/{namespace}   ← browse nodes (audited)
GET    /v1/memory/node/{id}            ← node detail (audited)
```

---

## 8. Configurability: No Hardcoded Hyperparameters

Every tunable value must be configurable at the appropriate level:

| Value | Where it's set | Default | Override at |
|---|---|---|---|
| Max workflow steps | `project.yaml` per trigger | 1000 | Product level |
| Workflow timeout | `project.yaml` per trigger | 5 min | Product level |
| LLM temperature | `models.yaml` | 0.7 | Product level |
| Search limit | memory backend config | 10 | Infrastructure level |
| Pool size | infrastructure config | 10 | Infrastructure level |
| Rate limits | `project.yaml` | None | Product level |
| Session timeout | `project.yaml` | 30 min | Product level |
| Extraction debounce | `project.yaml` | 180s | Product level |
| Edge decay factor | workflow config | 0.99 | Product level |
| Retry backoff | workflow YAML per node | 250ms | Workflow level |
| Stream buffer | infrastructure config | 256 | Infrastructure level |

**Rule: infrastructure values are set by infra admin. Product values are set by product admin. No value is hardcoded without a configurable override.**

---

## 9. Predictability: No Hidden Behaviors

Document every default and every implicit behavior:

| Behavior | Documentation |
|---|---|
| Entry node = first YAML key | README, llms.txt, workflow schema |
| Empty capabilities = deny all | README, capability docs |
| Repeat `until` = "stop when true" | README, YAML schema |
| Namespace injection overrides LLM params | Security docs, capability docs |
| Admin data access is audited | Audit log docs, admin console |
| Config changes trigger hash update | Config versioning docs |
| Session state has no default TTL | Operational guide |
| Cancelled workflows propagate to children | Runtime API docs |

**No behavior should surprise a developer reading the docs.** If something isn't documented, it's a bug.

---

## 10. What This Changes in Existing Plans

### Memory backends
- Each backend handles namespace isolation internally (WHERE clause, native namespaces, or RLS)
- VirtualizedTool namespace injection ensures the correct namespace reaches the backend
- No backend-specific code changes needed for multi-tenancy — the Tool layer handles it

### konf-runtime
- `ExecutionScope` includes `actor: Actor { id, role }` for identity tracking (see runtime.md)
- Audit log writes on every tool invocation that modifies data — **owned by konf-runtime** (writes to `audit_log` table)
- Runtime event journal writes operational events (writes to `runtime_events` table)
- Process table entries include `actor.role` for monitoring scoping
- Process table is **ephemeral** (in-memory). On restart, active runs are lost — clients must handle reconnection. Completed run history is in `runtime_events` (persistent).

### konf-backend
- Auth middleware resolves role from JWT claims (infra_admin, product_admin, user)
- Admin API endpoints with role-based access
- Audit log middleware for data access endpoints

### Project config
- Add `rate_limits` section to project.yaml
- Add `session_timeout` to project.yaml
- All hyperparameters from Unspool's `hyperparams.yaml` move to project.yaml

---

## 11. Sequencing: What's Needed When

### Before Unspool migration (Phase D):
- Namespace hierarchy convention (`konf:unspool:user_123`)
- Capability routing with namespace injection (already planned)
- Basic audit log table (runtime event journal covers this)
- User role only (single product, single tenant)

### Before first external product:
- Product admin role + config management API
- Product-scoped monitoring
- Audit log for admin data access
- Appsmith admin console

### Before multi-tenant hosting:
- Full RBAC (all 6 roles)
- RLS enforcement in Postgres
- Agent identity and scoping
- Cross-product isolation
- Rate limiting per product/user

This is NOT too ambitious. It's the right order of operations — namespace hierarchy from day one enables everything else to be layered on incrementally.

---

## 12. Decisions That Would Limit Future Capability

Avoid these:

| Bad decision | Why it limits you | Do this instead |
|---|---|---|
| Flat namespaces (`user_123`) | Can't scope product-level or infra-level access | Hierarchical (`konf:unspool:user_123`) |
| Hardcoded admin in code | Can't add roles later | Role column in users table from day one |
| Application-only data isolation | Bugs bypass access control | RLS at database level |
| No audit log | Can't prove compliance | Audit table from day one |
| Admin sees everything silently | GDPR violation, trust issue | Log every cross-scope access |
| Config in database only | Hard to version, hard to deploy | Config on disk + optional DB overlay |
| Single API key per product | Can't scope to read/write/admin | Scoped API keys with role attachment |
