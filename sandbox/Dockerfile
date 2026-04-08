FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl wget jq \
    build-essential \
    python3 python3-pip python3-venv \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for safety
RUN useradd -m -s /bin/bash konf-agent
USER konf-agent
WORKDIR /workspace

# Config dir will be mounted as a volume
VOLUME /workspace/config
