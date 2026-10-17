# ---------- estagio 1: build ----------
FROM rust:1.92-slim AS builder

# libsqlite3-sys compila o SQLite junto (feature `bundled`), entao e preciso um
# compilador C — o `cc` ja vem nesta imagem. pkg-config/libssl-dev cobrem
# dependencias nativas de rede/TLS do reqwest.
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY templates ./templates
COPY src ./src

RUN cargo build --release

# ---------- estagio 2: runtime ----------
FROM debian:bookworm-slim

# TLS: sem ca-certificates o reqwest nao valida o certificado do Querido Diario.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/monitor-diarios /usr/local/bin/monitor-diarios

WORKDIR /app

# `templates/` entra por `include_str!` e `migrations/` por `sqlx::migrate!`,
# ou seja, ja estao dentro do binario. As copias servem para inspecao/debug.
COPY templates ./templates
COPY migrations ./migrations
COPY start.sh ./start.sh
RUN chmod +x /app/start.sh

EXPOSE 8080
CMD ["./start.sh"]
