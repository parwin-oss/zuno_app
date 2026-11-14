FROM rust:1.80-bookworm AS frontend-builder
WORKDIR /APP

RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk --locked 
COPY . .
WORKDIR /app/frontend/web
RUN trunk build --release

 FROM rust:1.80-bookworm AS backend-builder
WORKDIR /APP
COPY . .
WORKDIR /app/backend/server
RUN cargo build --relaese --locked -p server

FROM debian:bookworm-slim AS runtime
WORKDIR /APP
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=frontend-builder /app/frontend/web/dist ./frontend/web/dist
COPY --from=backend-builder /app/backend/server/target/release/server ./backend/server/

ENV APP_HOST=0.0.0.0
ENV UPLOAD_DIR=/tmp/zuno-uploads

EXPOSE 80
ENTRYPOINT ["/usr/local/bin/server"]
