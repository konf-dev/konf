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
# curl is needed for the compose healthcheck; ca-certificates for TLS egress.
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false konf
# Pre-create the state dir with correct ownership BEFORE switching to konf user,
# so volume mounts at /var/lib/konf inherit konf:konf (not root:root).
RUN mkdir -p /var/lib/konf && chown konf:konf /var/lib/konf
USER konf
COPY --from=builder /app/target/release/konf-backend /usr/local/bin/
COPY products/ /etc/konf/products/
EXPOSE 8000
CMD ["konf-backend"]
