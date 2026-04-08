# Konf Configuration Strategy

**Status:** Authoritative
**Scope:** Platform config (konf.toml), product config (tools.yaml, workflows/, prompts/), hot-reload

> **Note:** Configuration is loaded by konf-init during boot. See [init.md](init.md) for the boot sequence and [overview.md](overview.md) for platform context.

---

## The Two-Level Config Model

Konf has two distinct config domains with different needs:

### Level 1: Platform Config (how the system runs)

**Format:** TOML (`konf.toml`) + env var overrides
**Audience:** Infra admin deploying the platform
**Reload:** Static — read once at startup. Restart required for changes.
**Pattern:** Restate's "zero-config defaults, optional TOML for production"

```toml
# konf.toml — platform infrastructure config
# Every value has a sensible default. This file is OPTIONAL.
# Env vars override: KONF_DATABASE__URL, KONF_ENGINE__MAX_STEPS, etc.

# [database] — OPTIONAL. Omit for edge/phone deployments without a database.
# When omitted: event journal disabled, scheduling unavailable, smrti backend unavailable.
[database]
url = "postgresql://localhost/konf"
pool_min = 5
pool_max = 20

[auth]
supabase_url = "http://localhost:9999"
jwt_audience = "authenticated"

[server]
host = "0.0.0.0"
port = 8000

[engine]
max_steps = 1000
default_timeout_ms = 30000
max_workflow_timeout_ms = 300000
max_concurrent_nodes = 50
stream_buffer = 256
max_yaml_size = 10485760

[runtime]
max_child_depth = 10
max_active_runs_per_namespace = 20

[memory]
search_mode = "hybrid"
search_limit = 10
rrf_k = 60
distance_metric = "cosine"
min_similarity = 0.0
max_traversal_depth = 5
max_traversal_nodes = 100

[scheduler]
enabled = true

[observability]
log_level = "info"
# otel_endpoint = "http://localhost:4317"  # uncomment to enable OTEL export
```

**Override hierarchy (figment):**
1. Compiled defaults (in Rust `Default` impls)
2. `konf.toml` (file)
3. Environment variables (`KONF_DATABASE__URL`, `KONF_ENGINE__MAX_STEPS`)
4. CLI arguments (highest priority)

**Why TOML for platform config:**
- Structured (sections map to Rust structs)
- Comments supported (unlike JSON)
- Less verbose than YAML for flat key-value (unlike Vector, our platform config IS flat per-section)
- figment has first-class TOML support

### Level 2: Product Config (what the system does)

**Format:** YAML directory
**Audience:** Product developer building an AI product on Konf
**Reload:** Dynamic — file watcher detects changes, hot-reloads safe fields
**Pattern:** Grafana's provisioning directory

```
config/
├── project.yaml           # Product metadata, workflow triggers
├── models.yaml            # LLM provider + model settings
├── tools.yaml             # Tool allowlist, MCP servers, custom tools
├── schedules.yaml         # Cron jobs
├── prompts/
│   ├── system.md          # Agent personality
│   └── extraction.md      # Extraction rules
└── workflows/
    ├── context.yaml       # Context assembly
    ├── chat.yaml          # Main chat workflow
    ├── extraction.yaml    # Cold-path extraction
    └── scheduled/
        └── synthesis.yaml # Nightly maintenance
```

**Why YAML for product config:**
- Already used for workflow definitions (konflux YAML)
- More readable than TOML for nested/complex structures (Vector learned this)
- Markdown prompts are separate files (not embedded in config)
- Directory structure carries meaning (workflows/ vs prompts/ vs tools.yaml)

---

## Static vs Dynamic Classification (Temporal Pattern)

Every config field is classified as **static** (read at startup) or **dynamic** (can be hot-reloaded):

### Platform config — ALL STATIC

| Section | Field | Type | Reason |
|---|---|---|---|
| database | url, pool_min, pool_max | Static | Connection pool created once |
| auth | supabase_url, jwt_audience | Static | JWKS client created once |
| server | host, port | Static | Listener bound once |
| engine | all fields | Static | Engine created once at startup |
| runtime | all fields | Static | Default limits applied at start |
| memory | all fields | Static | Memory config applied at connect |
| scheduler | enabled | Static | Worker created once |
| observability | log_level, otel_endpoint | Static | Tracing subscriber set once |

**Restart required for any platform config change.** This is intentional — platform config affects connection pools, listeners, and internal structures that can't be safely changed at runtime.

### Product config — MOSTLY DYNAMIC

| File | Reload? | What changes | What doesn't |
|---|---|---|---|
| prompts/*.md | ✅ Dynamic | Prompt text reloaded on next request | N/A |
| models.yaml | ✅ Dynamic | Model/temperature for next LLM call | Provider (requires new client) |
| tools.yaml (allowed) | ⚠️ Partial | Tool allowlist updated | MCP servers (require spawn/kill) |
| tools.yaml (mcp_servers) | ❌ Static | N/A | Server processes managed at startup |
| tools.yaml (custom) | ❌ Static | N/A | Python modules loaded at startup |
| workflows/*.yaml | ✅ Dynamic | Workflow YAML re-parsed on next trigger | N/A |
| project.yaml | ⚠️ Partial | Trigger config, capabilities | Namespace template |
| schedules.yaml | ❌ Static | N/A | Cron jobs registered at startup |

**Hot-reload mechanism:** File system watcher (`notify` crate) on the config directory. On change:
1. Re-parse the changed file
2. Validate against schema
3. If valid, swap the in-memory config atomically (`Arc<ArcSwap<ProductConfig>>`)
4. Compute new config version hash
5. Log the change to audit log

**If validation fails:** Keep the old config, log a warning. Never apply invalid config.

---

## Config Validation

### At startup (fail-fast)

```rust
// Platform config validation
let config: PlatformConfig = figment.extract()?;
config.database.validate()?;  // DSN format, pool bounds
config.engine.validate()?;    // All values > 0
config.runtime.validate()?;   // All values > 0
config.memory.validate()?;    // memory backend config validation

// Product config validation
let product = ProductConfig::load(&config.config_dir)?;
product.validate_tools_exist(&engine)?;        // All referenced tools registered
product.validate_capabilities_satisfiable()?;  // All trigger capabilities have matching tools
product.validate_workflows_parse(&engine)?;    // All YAMLs parse successfully
```

**If ANY validation fails, the server refuses to start with a clear error message listing every problem.**

### On hot-reload (warn, don't crash)

```rust
match ProductConfig::load(&config.config_dir) {
    Ok(new_config) => {
        match new_config.validate_all(&engine) {
            Ok(()) => {
                product_config.swap(Arc::new(new_config));
                info!(config_version = %new_hash, "product config reloaded");
                audit_log.write("config_change", ...);
            }
            Err(errors) => {
                warn!(errors = ?errors, "product config invalid, keeping previous version");
            }
        }
    }
    Err(e) => {
        warn!(error = %e, "failed to parse product config, keeping previous version");
    }
}
```

---

## Config Versioning

Every config state gets a SHA-256 hash. This hash is attached to:
- Every persisted message
- Every runtime event journal entry
- Every audit log entry

```rust
fn compute_config_hash(config_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    for entry in WalkDir::new(config_dir).sort_by_file_name() {
        if let Ok(entry) = entry {
            if entry.file_type().is_file() {
                hasher.update(entry.path().to_string_lossy().as_bytes());
                hasher.update(&std::fs::read(entry.path()).unwrap_or_default());
            }
        }
    }
    format!("{:x}", hasher.finalize())[..16].to_string()
}
```

When debugging "why did the agent do X?", the config_version in the event tells you exactly which config was active.

---

## Unified Config Struct (Rust)

```rust
/// Platform-level configuration. Loaded once at startup.
/// All fields have serde defaults. Loaded via figment.
#[derive(Debug, Clone, Deserialize)]
pub struct PlatformConfig {
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub engine: EngineSettings,
    #[serde(default)]
    pub runtime: RuntimeSettings,
    #[serde(default)]
    pub memory: MemorySettings,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
    #[serde(default = "default_config_dir")]
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_url")]
    pub url: String,
    #[serde(default = "default_pool_min")]
    pub pool_min: u32,
    #[serde(default = "default_pool_max")]
    pub pool_max: u32,
}

// EngineSettings maps to EngineConfig
#[derive(Debug, Clone, Deserialize)]
pub struct EngineSettings {
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
    // ... all EngineConfig fields
}

impl From<EngineSettings> for konflux::EngineConfig {
    fn from(s: EngineSettings) -> Self {
        konflux::EngineConfig {
            max_steps: s.max_steps,
            default_timeout_ms: s.default_timeout_ms,
            // ...
        }
    }
}
```

**Every field has `#[serde(default = "...")]` — zero-config works out of the box.**

---

## Env Var Override Convention

figment maps nested TOML keys to env vars with double-underscore separators:

| TOML key | Env var |
|---|---|
| `database.url` | `KONF_DATABASE__URL` |
| `engine.max_steps` | `KONF_ENGINE__MAX_STEPS` |
| `memory.search_mode` | `KONF_MEMORY__SEARCH_MODE` |
| `server.port` | `KONF_SERVER__PORT` |
| `auth.supabase_url` | `KONF_AUTH__SUPABASE_URL` |

Prefix is `KONF_` to avoid collisions with other software.

---

## Documentation: Config Reference

The konf-backend should generate a config reference document listing every field:

```markdown
## Platform Configuration Reference

### [database] (optional)

Omit this section entirely for edge/phone deployments. When absent, the event journal, scheduling, and Postgres-based memory backends are disabled.

| Field | Type | Default | Env Override | Description |
|---|---|---|---|---|
| `url` | string | `postgresql://localhost/konf` | `KONF_DATABASE__URL` | PostgreSQL connection string |
| `pool_min` | integer | `5` | `KONF_DATABASE__POOL_MIN` | Minimum connection pool size |
| `pool_max` | integer | `20` | `KONF_DATABASE__POOL_MAX` | Maximum connection pool size |

### [engine]

| Field | Type | Default | Env Override | Description |
|---|---|---|---|---|
| `max_steps` | integer | `1000` | `KONF_ENGINE__MAX_STEPS` | Max workflow steps before abort |
| `default_timeout_ms` | integer | `30000` | `KONF_ENGINE__DEFAULT_TIMEOUT_MS` | Per-tool timeout |
...
```

This reference is generated from the Rust struct annotations — not hand-maintained.

---

## What This Means for Existing Code

### konflux-core: Add serde to EngineConfig

```rust
#[derive(Debug, Clone, Deserialize)]  // add Deserialize
pub struct EngineConfig {
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    // ...
}
```

### konf-runtime: Add serde to ResourceLimits (already done)

Already has `Deserialize`. ✓

### Memory backend: Config validation via figment + serde

SmrtiConfig already follows this pattern. ✓

### konf-backend: Use figment as the single config entry point

```rust
let config: PlatformConfig = Figment::new()
    .merge(Serialized::defaults(PlatformConfig::default()))
    .merge(Toml::file("konf.toml"))
    .merge(Env::prefixed("KONF_").split("__"))
    .extract()?;
```

---

## Key Design Decisions

| Decision | Rationale |
|---|---|
| TOML for platform config | Structured, commented, less verbose than YAML for flat config |
| YAML for product config | Already used for workflows, better for nested structures |
| All fields have defaults | Zero-config works (Restate pattern) |
| Env vars override files | Standard for containers (12-factor) |
| Static platform, dynamic product | Platform config affects connections/pools (unsafe to reload). Product config is just data. |
| Fail-fast on startup | Invalid config = don't start. No silent degradation. |
| Warn on hot-reload failure | Keep old config, log warning. Never crash a running server for a config typo. |
| Config hash in events | Audit trail: which config produced this behavior? |

---

## What We DON'T Do (Learned from Others)

| Anti-pattern | Who got burned | Our approach |
|---|---|---|
| 80+ flat env vars | Supabase | Structured TOML sections |
| Config embedded in database | Various | Files on disk (git-friendly, reviewable) |
| No defaults (everything required) | Early Temporal | Every field has a default |
| Hot-reload of connection pools | — | Classify as static, require restart |
| Config migration between versions | GitLab | Keep field names stable, add new fields with defaults |
| Auto-generated config files on first run | — | Ship an example `konf.toml.example`, don't auto-generate |
