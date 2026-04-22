# Build stage
FROM rust:alpine AS builder

# Install build dependencies for Rust on Alpine, SQLite, and BoringSSL (requires C++ toolchain and Go)
RUN apk add --no-cache musl-dev sqlite-dev openssl-dev pkgconfig g++ gcc make cmake go perl clang clang-dev linux-headers git
RUN rustup component add rustfmt

WORKDIR /app

# Using cargo-chef for dependency caching
RUN cargo install cargo-chef

# Prepare step
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Cook step (Build dependencies)
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Build the actual application
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

# Final stage - Minimal footprint
FROM alpine:latest

# Install runtime dependencies (ca-certificates for HTTPS, sqlite-libs for DB)
RUN apk add --no-cache ca-certificates sqlite-libs

WORKDIR /app

# Copy the stripped binary from builder
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/fitnab /usr/local/bin/fitnab

# Create directory for SQLite DB
RUN mkdir -p /app/data

# Expose Torznab API port
EXPOSE 3000

ENV DATABASE_URL="sqlite:/app/data/fitnab.db"

ENTRYPOINT ["fitnab"]
