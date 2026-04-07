# Stage 1: Chef - dependency caching
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

# Stage 2: Planner
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin konf-backend

# Stage 4: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false konf
USER konf
COPY --from=builder /app/target/release/konf-backend /usr/local/bin/
EXPOSE 8000
CMD ["konf-backend"]
