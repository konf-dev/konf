# Konf Sandbox

A Docker container providing isolated shell access for the Konf Architect agent.

## Quick Start

```bash
# Build and start the sandbox
docker compose up -d

# Verify it's running
docker exec konf-sandbox echo "sandbox ready"
```

## Security

The sandbox runs with strict resource limits:

| Limit | Value | Why |
|-------|-------|-----|
| Network | `none` | No network access by default |
| CPU | 2 cores | Prevent compute exhaustion |
| Memory | 1GB (no swap) | Prevent OOM on host |
| PIDs | 100 | Prevent fork bombs |
| Capabilities | All dropped | No privilege escalation |
| User | `konf-agent` (non-root) | Least privilege |

## Shared Volume

The `/workspace` directory is a shared volume between the container and the host. The Konf engine reads config from this directory — when the agent writes workflow files inside the container, Konf sees them on the host.

## Opening Network Access

To allow the agent to make HTTP requests (e.g., for testing workflows that use `http:get`), change `network_mode` in `docker-compose.yml`:

```yaml
# Allow specific egress (create a custom network with iptables rules)
network_mode: "bridge"
```

Start with `none` and open selectively as needed.
