# Deployment Guide

> Scope: running Konf in production.

## Docker Compose (Recommended)

The repository includes a `docker-compose.yml` at the project root:

```bash
# Set a secure password
export POSTGRES_PASSWORD=change-me-in-production

# Start Konf + Postgres with pgvector
docker compose up -d
```

The default compose file mounts `products/devkit/config` as the product config. To use a different product, edit the volume mount:

```yaml
volumes:
  - ./products/my-product/config:/config:ro
```

Health check: `curl http://localhost:8000/v1/health`

## From Source

```bash
# Build release binary
cargo build --release --bin konf-backend

# Run with a product config directory
KONF_CONFIG_DIR=products/devkit/config \
KONF__DATABASE__URL=postgresql://postgres:konf@localhost/konf \
  ./target/release/konf-backend
```

## systemd Service Unit

```ini
[Unit]
Description=Konf Agent OS Backend
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=konf
Group=konf
WorkingDirectory=/opt/konf
ExecStart=/opt/konf/konf-backend
Restart=on-failure
RestartSec=5

# Configuration
Environment=KONF_CONFIG_DIR=/opt/konf/config
Environment=KONF__DATABASE__URL=postgresql://konf:secret@localhost/konf
Environment=KONF__SERVER__HOST=127.0.0.1
Environment=KONF__SERVER__PORT=8000

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadOnlyPaths=/opt/konf/config
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

Install:

```bash
sudo cp konf.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now konf
```

## Environment Variables

All platform configuration can be set via environment variables with the `KONF_` prefix. Nested fields use double underscores (`__`):

| Variable | Default | Description |
|----------|---------|-------------|
| `KONF_CONFIG_DIR` | `./config` | Path to product config directory |
| `KONF__DATABASE__URL` | — | Postgres connection string |
| `KONF__DATABASE__POOL_MIN` | `5` | Minimum connection pool size |
| `KONF__DATABASE__POOL_MAX` | `20` | Maximum connection pool size |
| `KONF__SERVER__HOST` | `0.0.0.0` | Bind address |
| `KONF__SERVER__PORT` | `8000` | Bind port |
| `KONF__AUTH__SUPABASE_URL` | `http://localhost:9999` | Supabase auth endpoint |
| `KONF__AUTH__JWT_AUDIENCE` | `authenticated` | Expected JWT audience claim |
| `KONF__MCP_ENABLED` | `false` | Enable MCP server support |

See [platform-config.md](platform-config.md) for the full `konf.toml` reference.
