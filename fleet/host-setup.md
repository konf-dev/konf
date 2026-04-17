# Host-side one-time setup

The konf-prime fleet depends on two things outside docker:
dual-GPU ollama endpoints, and (optionally) an API-key env set for
the paid-tier variants.

## Dual-GPU ollama

You have two GPUs. The fleet uses each for different model sizes:

| Endpoint | GPU | Models to keep warm |
|---|---|---|
| `host.docker.internal:11434` | RTX 3090 Ti (GPU 1, 24 GB) | `gemma4:31b`, `qwen3-coder:30b`, `deepseek-r1:32b` |
| `host.docker.internal:11435` | RTX 2070 Super (GPU 0, 8 GB) | `gemma4:e4b`, `qwen3:8b` |

The default systemd ollama on port 11434 stays as-is (auto-placement
will drift onto GPU 1 because it has more free memory, but for
determinism you can pin it too).

### Ad-hoc: just this session

```bash
# Start the small-GPU instance in the background
setsid env CUDA_VISIBLE_DEVICES=0 OLLAMA_HOST=0.0.0.0:11435 \
  /usr/bin/ollama serve </dev/null >/tmp/ollama-small.log 2>&1 &
disown
```

Check:

```bash
ss -tlnp | grep -E ':11434|:11435'
curl http://localhost:11435/api/tags | jq '.models[].name'
```

### Persistent: systemd unit

Save as `/etc/systemd/system/ollama-small.service`:

```ini
[Unit]
Description=Ollama local AI — small-GPU instance
After=network-online.target
Wants=network-online.target

[Service]
Environment="OLLAMA_HOST=0.0.0.0:11435"
Environment="CUDA_VISIBLE_DEVICES=0"
ExecStart=/usr/bin/ollama serve
Restart=on-failure
RestartSec=5
User=ollama
Group=ollama

[Install]
WantedBy=multi-user.target
```

Then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now ollama-small
```

Pin the existing `ollama.service` to GPU 1 by adding
`Environment="CUDA_VISIBLE_DEVICES=1"` to its `override.conf`.

## API keys

Two env vars the `konf-prime-gemini` and `konf-prime-hybrid` variants
need to start:

- `GEMINI_API_KEY` — required by both paid variants.
- `KONF_GITHUB_TOKEN` — required by the `github` MCP server in all
  three variants.

Before `docker compose up` for any paid-tier variant:

```bash
export GEMINI_API_KEY=...
export KONF_GITHUB_TOKEN=...

docker compose -f fleet/fleet-compose.yml up -d prime-gemini prime-hybrid
```

The ollama-only `prime` variant works without either env var (the
github MCP server will just fail to connect, which is a soft
failure — other MCP servers still register).

## Populate the per-variant `/src` volumes

Each variant has its own writable git clone of the konf source at
`/src`. Run once before first boot of a new variant:

```bash
for v in fleet_prime_src fleet_prime_gemini_src fleet_prime_hybrid_src; do
  docker volume create "$v"
  docker run --rm \
    -v "$(pwd):/host:ro" -v "$v:/dest" \
    alpine sh -c '
      apk add -q rsync &&
      rsync -a --exclude=target/ --exclude=node_modules/ --exclude=".direnv/" \
        /host/ /dest/ &&
      chown -R 999:999 /dest
    '
done
```

The agent can later `git remote set-url origin ...` inside the volume
if it wants to push branches somewhere reviewable.
