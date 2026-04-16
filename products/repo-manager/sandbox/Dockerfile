# syntax=docker/dockerfile:1.7
#
# repo-manager sandbox image.
#
# Rust + Node + git + tmux + github-mcp-server native binary + claude-cli.
# konf sources are bind-mounted at runtime; konf-mcp is built on first
# start via the entrypoint and then cached in the mounted target/ dir.
#
# Build:
#   podman build -t konf-repo-manager:latest --build-arg UID=$(id -u) \
#                --build-arg GID=$(id -g) sandbox/
#
# Runs rootless under Podman. Under Docker Engine it runs as the
# non-root konf-agent user (UID matched to host) so bind-mounted files
# don't end up root-owned.

ARG RUST_VERSION=1.85
FROM rust:${RUST_VERSION}-slim-bookworm

# ------------------------------------------------------------------
# OS layer
# ------------------------------------------------------------------
ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      curl \
      git \
      gnupg \
      jq \
      libssl-dev \
      pkg-config \
      python3 \
      tmux \
      tini \
    && rm -rf /var/lib/apt/lists/*

# Node 20 via NodeSource — Debian's default is too old for claude-cli.
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# ------------------------------------------------------------------
# github-mcp-server — native binary, no docker-in-docker.
# ------------------------------------------------------------------
ARG GITHUB_MCP_VERSION=v0.10.0
ARG TARGETARCH=amd64
RUN set -eux; \
    curl -fsSL -o /tmp/gh-mcp.tar.gz \
      "https://github.com/github/github-mcp-server/releases/download/${GITHUB_MCP_VERSION}/github-mcp-server_Linux_${TARGETARCH}.tar.gz"; \
    tar -xzf /tmp/gh-mcp.tar.gz -C /usr/local/bin github-mcp-server; \
    chmod +x /usr/local/bin/github-mcp-server; \
    rm /tmp/gh-mcp.tar.gz; \
    github-mcp-server --version

# ------------------------------------------------------------------
# claude-cli
# ------------------------------------------------------------------
RUN npm install -g @anthropic-ai/claude-code \
    && npm cache clean --force

# ------------------------------------------------------------------
# Non-root user, UID-aligned with the host for clean bind-mount perms.
# ------------------------------------------------------------------
ARG UID=1000
ARG GID=1000
ARG USERNAME=konf-agent
RUN groupadd --gid "${GID}" "${USERNAME}" \
    && useradd --uid "${UID}" --gid "${GID}" --shell /bin/bash \
               --create-home "${USERNAME}"

# tmux config — sane defaults, 256-color, big history.
COPY --chown=${UID}:${GID} tmux.conf /home/${USERNAME}/.tmux.conf

# Entrypoint handles konf-mcp build + tmux session startup.
COPY --chown=${UID}:${GID} entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

USER ${USERNAME}
WORKDIR /home/${USERNAME}

# Cargo cache lives under the user so named volumes can persist it.
ENV CARGO_HOME=/home/${USERNAME}/.cargo
ENV PATH=/home/${USERNAME}/.cargo/bin:${PATH}
ENV KONF_CONFIG_DIR=/home/${USERNAME}/konf-dev-stack/konf/products/repo-manager/config

# tini as PID 1 — reaps zombies so tmux sessions behave.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
CMD ["sleep", "infinity"]
