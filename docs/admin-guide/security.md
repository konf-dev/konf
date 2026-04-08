# Security Overview

> Scope: security model for platform operators.

## Namespace Hierarchy

Every resource in Konf belongs to a hierarchical namespace:

```
konf                    # platform root (infra only)
├── assistant           # product namespace
│   ├── user_123        # end-user scope
│   └── user_456
└── support-bot
    └── org_789
```

A scope can only access resources at or below its own level. There is no "reach up" — a user scope cannot read another product's data, and a product scope cannot read platform-level resources.

## Capability Lattice

Capabilities follow a strict **attenuation-only** model:

1. Infra grants capabilities to admin.
2. Admin grants a subset to product (via `project.yaml` triggers).
3. Product grants a subset to end-user sessions.

Capabilities can only be **narrowed**, never amplified. A child scope cannot possess a capability its parent does not have.

```
infra: [*]
  └── admin: [memory:*, ai:*, http:*, mcp:*]
        └── product: [memory:*, ai:complete, http:get]
              └── user: [memory:search, memory:store, ai:complete]
```

## VirtualizedTool Namespace Injection

When a tool is invoked, the runtime wraps it in a `VirtualizedTool` that automatically injects the caller's namespace into the request. The LLM and workflow author never specify a namespace — they cannot access or even see namespace metadata.

This means:
- `memory:store` called by `user_123` writes to `konf:assistant:user_123`
- The same tool called by `user_456` writes to `konf:assistant:user_456`
- No prompt injection can change the namespace

## Audit Logging

Every cross-scope access attempt is logged with:
- Caller namespace
- Target resource
- Capability used
- Timestamp
- Allow/deny result

## Row-Level Security

Postgres tables enforce namespace isolation via RLS policies. Even if application logic has a bug, the database layer prevents cross-tenant reads and writes.

## Credential Handling

- Database URLs are redacted in all log output (the `DatabaseConfig` Debug impl replaces `url` with `[REDACTED]`).
- Secrets are loaded from environment variables, never from checked-in config.
- `tools.yaml` supports `${VAR:-default}` interpolation for secrets.

## Further Reading

- [Architecture: Multi-Tenancy and Permissions](../architecture/multi-tenancy.md)
- [Concepts: Four-Layer Model](../getting-started/concepts.md)
