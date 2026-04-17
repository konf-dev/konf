# Stage 1: Chef - dependency caching
#
# trixie (glibc 2.41+) required because ort.pyke.io's prebuilt onnxruntime
# binaries (pulled in by fastembed via konf-tool-embed) are linked against
# glibc 2.38+ symbols (__isoc23_strtol, __isoc23_strtoll, __isoc23_strtoull).
# Bookworm's glibc 2.36 fails at link time.
FROM rust:1-trixie AS chef
# libclang is required by bindgen-backed crates (rocksdb-sys via surrealdb, etc).
# Install into the shared chef stage so planner and builder both have it.
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef
WORKDIR /app

# Stage 2: Planner
FROM chef AS planner
COPY . .
# Stub the private smrti git dep so metadata resolution succeeds without SSH keys.
# See scripts/stub-smrti.sh — creates vendor/smrti/ + .cargo/config.toml patch.
RUN bash scripts/stub-smrti.sh
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Carry the smrti stub + .cargo patch forward so `cargo chef cook` can resolve
# the path-patched dependency before the full source tree is copied.
COPY --from=planner /app/vendor ./vendor
COPY --from=planner /app/.cargo ./.cargo
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# Re-stub after the fresh COPY . . overwrites the planner-side vendor/.cargo.
RUN bash scripts/stub-smrti.sh
RUN cargo build --release --bin konf-backend

# Stage 4: Runtime
# trixie to match the chef stage's glibc (prebuilt ORT binaries link against it).
FROM debian:trixie-slim
# Baseline for the backend + coding-agent toolkit.
#   curl, ca-certificates: healthcheck + HTTPS.
#   git: konf-prime clones its own source tree into /src and works on branches.
#   ripgrep, fd-find, jq: standard agent search/JSON toolkit.
#   nodejs, npm: runs any MCP server published on npm (`npx -y <server>`).
#   python3 + pip + pipx + python3-yaml: python-based MCP servers + agent-side
#     validation of YAML edits before atomic swap.
#   docker.io client CLI: with the socket mounted, the agent can `docker build`
#     + `docker restart` itself. (We install only the client, daemon stays on host.)
#   sudo: konf user is non-root; agent needs NOPASSWD sudo to apt-install
#     anything else it decides it needs post-boot.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    git ripgrep fd-find jq \
    nodejs npm \
    python3 python3-pip python3-yaml pipx \
    docker-cli \
    sudo \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -r -m -s /bin/bash konf \
    && echo 'konf ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/konf \
    && chmod 0440 /etc/sudoers.d/konf
# Pre-create the state dir with correct ownership BEFORE switching to konf user,
# so volume mounts at /var/lib/konf inherit konf:konf (not root:root).
RUN mkdir -p /var/lib/konf && chown konf:konf /var/lib/konf
USER konf
COPY --from=builder /app/target/release/konf-backend /usr/local/bin/
COPY products/ /etc/konf/products/
EXPOSE 8000
CMD ["konf-backend"]
