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
RUN apk add --no-cache musl-dev sqlite-static openssl-dev openssl-libs-static pkgconfig g++ gcc make cmake perl clang-dev linux-headers
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Stage 3: Builder
FROM rust:alpine AS builder
WORKDIR /app
# Install all build-time dependencies
RUN apk add --no-cache musl-dev sqlite-static openssl-dev openssl-libs-static pkgconfig g++ gcc make cmake perl clang-dev linux-headers

# Create a non-privileged user for the final image
RUN addgroup -S fitnab && adduser -S fitnab -G fitnab

COPY . .
# Build a fully static binary
# We link sqlite and openssl statically so we can run in a 'scratch' or 'static' distroless image
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

# Copy timezone data and CA certificates (already in distroless static, but good to be explicit)
# Distroless static includes these by default.

# Create data directory (Distroless doesn't have mkdir, so we do it in builder or rely on volume)
# We'll use the user we created
USER fitnab

# Torznab API port
EXPOSE 3000

ENV RUST_LOG="info"

ENTRYPOINT ["/usr/local/bin/fitnab"]
