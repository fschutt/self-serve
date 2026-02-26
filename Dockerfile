# ── Stage 1: build ────────────────────────────────────────────────────────────
FROM rust:1.82 AS builder

WORKDIR /app
COPY . .

# Build release binary.
# strip = false is set in Cargo.toml so that function symbols survive into
# the binary — the transpiler reads them at runtime to locate x86-64 machine
# code via the `object` crate.
RUN cargo build --release --bin x64_to_wasm_server

# ── Stage 2: runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/x64_to_wasm_server /usr/local/bin/server

# Fly.io sets $PORT; our server reads it and defaults to 8080.
ENV PORT=8080
EXPOSE 8080

CMD ["server"]
