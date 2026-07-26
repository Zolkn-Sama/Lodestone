# syntax=docker/dockerfile:1

# =============================================================================
# 1. Base commune + cargo-chef (cache des dépendances entre builds)
# =============================================================================
FROM rust:1.95-slim-bookworm AS chef
# Outils requis pour compiler le backend TLS (rustls/aws-lc-rs) : C compiler + cmake + perl.
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config build-essential cmake perl \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# =============================================================================
# 2. Plan des dépendances (recipe.json) — change rarement
# =============================================================================
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# =============================================================================
# 3. Build : on cuit d'abord SEULEMENT les dépendances (couche mise en cache
#    tant que recipe.json ne bouge pas), puis on compile le code.
# =============================================================================
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin lodestone

# =============================================================================
# 4. Image finale légère (sans la toolchain Rust)
# =============================================================================
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 app
COPY --from=builder /app/target/release/lodestone /usr/local/bin/lodestone
USER app
EXPOSE 8090
ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/lodestone"]