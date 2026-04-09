# Guide: Managing Secrets in Konf OS

Konf OS enforces a strict separation between application configuration and sensitive secrets (API keys, passwords, private tokens). 

Secrets are never stored in plain text in `tools.yaml` or `project.yaml`. Instead, they are injected into the environment and accessed through a controlled interface.

---

## 1. The Strategy: Injection & Interpolation

### Step A: Injection
Secrets are injected into the process's RAM at boot time. We recommend using **Infisical** for this. 

```bash
# Example: Inject secrets from the local Infisical instance
infisical run --env=dev -- konf --product devkit
```

### Step B: Interpolation (Static Config)
If you need a secret in your `tools.yaml` (e.g., for an MCP server's environment), use the `${VAR:-default}` syntax:

```yaml
# products/devkit/config/tools.yaml
mcp:
  github:
    command: "npx"
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: ${GITHUB_TOKEN}
```

---

## 2. The `secret:*` Tool (Dynamic Access)

For workflows and agents that need to fetch secrets dynamically (e.g., to call an external API that isn't wrapped in a tool), use the standard library `secret` tools.

### Enabling Secrets
You must explicitly allow which environment variables the product is allowed to read in your `tools.yaml`.

```yaml
# products/devkit/config/tools.yaml
tools:
  secret:
    allowed_keys:
      - "STRIPE_SECRET_KEY"
      - "ANTHROPIC_API_KEY"
```

### Using the Tool
An agent can then call the tool via a workflow node:

```yaml
# Node in a workflow
do: "secret:get"
with:
  key: "STRIPE_SECRET_KEY"
```

---

## 3. Why this is secure

1.  **Redaction:** Konf's logging system (via `DatabaseConfig` and `ToolInfo`) automatically redacts common sensitive patterns.
2.  **Attenuation:** A product can only access the keys listed in its `allowed_keys` config. Even if a workflow has `secret:*` capabilities, it cannot read `PATH` or `AWS_SECRET_KEY` unless they are explicitly whitelisted.
3.  **No Persistence:** Secrets only exist in the memory of the running process. If the process stops, the secrets vanish.
