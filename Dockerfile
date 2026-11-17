
# ==========================================
# FRONTEND BUILDER
# ==========================================
FROM rust:1.89-bookworm AS frontend-builder

WORKDIR /app

# Install WebAssembly target
RUN rustup target add wasm32-unknown-unknown

# Install Trunk
RUN cargo install trunk --version 0.21.14 --locked

# Install matching wasm-bindgen CLI
RUN cargo install wasm-bindgen-cli --version 0.2.130 --locked

# Verify wasm-bindgen installation
RUN wasm-bindgen --version

# Copy project source
COPY . .

# Build Leptos frontend
WORKDIR /app/frontend/web

RUN trunk build --release


# ==========================================
# BACKEND BUILDER
# ==========================================
FROM rust:1.89-bookworm AS backend-builder

WORKDIR /app

# Copy project source
COPY . .

# Build Axum backend
RUN cargo build --release --locked -p server


# ==========================================
# PRODUCTION RUNTIME
# ==========================================
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install CA certificates
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy frontend build
COPY --from=frontend-builder /app/frontend/web/dist ./frontend/web/dist

# Copy backend executable
COPY --from=backend-builder /app/target/release/server /usr/local/bin/server

# Runtime environment
ENV APP_HOST=0.0.0.0
ENV UPLOAD_DIR=/tmp/zuno-uploads

EXPOSE 80

ENTRYPOINT ["/usr/local/bin/server"]
