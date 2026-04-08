FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl wget jq \
    build-essential \
    python3 python3-pip python3-venv \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for safety (reuse ubuntu user UID 1000 for volume sharing)
RUN usermod -l konf-agent -d /home/konf-agent -m ubuntu && groupmod -n konf-agent ubuntu
RUN mkdir -p /workspace && chown konf-agent:konf-agent /workspace
USER konf-agent
WORKDIR /workspace
