# Stage 1: Recipe Plan
FROM rust:alpine AS planner
WORKDIR /app
RUN apk add --no-cache musl-dev
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Caching Dependencies
FROM rust:alpine AS cacher
WORKDIR /app
# Only essential build tools needed now that we are using pure-Rust TLS
RUN apk add --no-cache musl-dev sqlite-static pkgconfig gcc make
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Stage 3: Builder
FROM rust:alpine AS builder
WORKDIR /app
RUN apk add --no-cache musl-dev sqlite-static pkgconfig gcc make

# Create non-privileged user
RUN addgroup -S fitnab && adduser -S fitnab -G fitnab

COPY . .
# Build a fully static binary using musl and rustls
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --target x86_64-unknown-linux-musl

# Stage 4: Final Lean Image
FROM gcr.io/distroless/static-debian12:latest

WORKDIR /app

# Copy user information
COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group

# Copy the static binary
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/fitnab /usr/local/bin/fitnab

# Torznab API port
EXPOSE 3000

ENV RUST_LOG="info"

ENTRYPOINT ["/usr/local/bin/fitnab"]
