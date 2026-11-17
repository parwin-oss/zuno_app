FROM rust:1.89-bookworm AS frontend-builder
WORKDIR /app
RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk --version 0.21.14 --locked
RUN cargo install wasm-bindgen-cli --version 0.2.95
COPY . .
WORKDIR /app/frontend/web
RUN trunk build --release

FROM rust:1.89-bookworm AS backend-builder
WORKDIR /app
COPY . .
WORKDIR /app/backend/server
RUN cargo build --release --locked -p server

FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=frontend-builder /app/frontend/web/dist ./frontend/web/dist
COPY --from=backend-builder /app/target/release/server /usr/local/bin/server

ENV APP_HOST=0.0.0.0
ENV UPLOAD_DIR=/tmp/zuno-uploads

EXPOSE 80
ENTRYPOINT ["/usr/local/bin/server"]