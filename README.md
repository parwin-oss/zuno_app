# Zuno

Zuno is a Rust workspace containing an Axum/PostgreSQL messaging API and a
Leptos WebAssembly client.

## What is implemented

- Argon2 password hashing, bearer access tokens, rotating refresh tokens, and
  logout revocation.
- Authenticated profile reads/updates and PNG, JPEG, GIF, or WebP avatar uploads
  (maximum 5 MB), with generated filenames and static image serving.
- User search, deduplicated direct conversations, group creation and member
  management, owner-authorized group renaming, and membership checks on
  conversation and message operations.
- Persisted text messaging, newest-first paginated history, chronological client
  rendering, message edits/deletes, and read receipts.
- Authenticated WebSocket notifications for messages, edits, deletes, typing,
  read receipts, and online presence, with heartbeat handling and client
  reconnect/catch-up behavior.
- A browser client for registration/login, session restoration, profile editing
  and avatar uploads, user search, direct/group chat creation and management,
  message history, sending, editing, and deletion.
- PostgreSQL migrations run automatically by the server on startup.

The WebSocket hub is in-memory and fans out only within one API process.
Cross-instance delivery requires shared pub/sub and has not been added.
Production deployment, including the target platform's WebSocket support, has
not been verified.

## Verification

The workspace checks and frontend build are listed below. The profile, search,
direct/group conversation, persistence, authorization, read/typing/presence,
and two-client WebSocket workflows have also been exercised against the local
PostgreSQL-backed API. This is manual integration verification; the repository
does not yet contain a repeatable, automated end-to-end test suite for all of
these workflows.

## Local development

Requirements: Rust (including `wasm32-unknown-unknown`), Trunk, and Docker
Compose.

1. Copy `.env.example` to `.env`. Replace the local database password and
   `JWT_SECRET` with local-only values; keep `.env` out of source control.
2. Start PostgreSQL and the API:

   ```sh
   docker compose up --build --wait
   ```

   PostgreSQL is published only on `127.0.0.1:5434`; the API listens on
   `127.0.0.1:3000`. The API runs migrations before becoming ready.
3. In a separate terminal, serve the frontend:

   ```sh
   cd frontend/web
   trunk serve
   ```

   Open `http://127.0.0.1:8080`. For another API origin, build with
   `ZUNO_API_URL=https://your-api.example cargo ...` or configure the variable
   in the frontend build environment.

Avatar uploads accept PNG, JPEG, GIF, and WebP images up to 5 MB. Files are
stored in `UPLOAD_DIR` (default `/tmp/zuno-uploads`) and served from `/uploads`;
Docker Compose mounts a persistent local volume for them.
4. Stop the local stack with `docker compose down`. The named PostgreSQL volume
   is retained. Use `docker compose down -v` only when intentionally deleting
   local database data.

Run checks from the workspace root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo check --workspace
cargo test --workspace
cd frontend/web && trunk build
```

## API notes

All `/api` endpoints except registration/login require
`Authorization: Bearer <access-token>`. Access tokens are short-lived;
`POST /api/auth/refresh` rotates the refresh token. The frontend stores these
tokens in browser local storage, so production deployments should move refresh
tokens to secure, HttpOnly cookies and assess the XSS implications of browser
storage.

WebSocket clients connect to `/ws?access_token=<access-token>`. The API accepts
`{"type":"typing","conversation_id":"<uuid>","active":true}` events and sends
`message`, `typing`, and `presence` events only to connected members of the
relevant conversation(s). The WebSocket carries notifications, not message
durability; messages are written to PostgreSQL by the REST endpoint.

## Deployment

The Leptos client has a Vercel build configuration in `frontend/web/vercel.json`.
Set `ZUNO_API_URL` in the frontend project's build environment to the public API
origin. The API's root `Dockerfile` is suitable for a Vercel Container project
and reads the platform-provided `PORT`; configure `DATABASE_URL`, `JWT_SECRET`,
and `CORS_ORIGINS` in the deployment environment. PostgreSQL must be an external
managed service, not the local Compose database.

Vercel documents WebSocket support as a public beta. Connections are pinned to
one function instance but can close at the configured maximum duration (300
seconds on Hobby, configurable limits on paid plans), so the client reconnects
with backoff and reloads chat state. In-memory WebSocket fan-out is per instance;
if the API scales across instances, messages between clients on different
instances require shared pub/sub (for example Redis). This workspace intentionally
does not add that infrastructure yet. Axum on Vercel's container runtime and
end-to-end WebSocket behavior have not been deployed or verified, so a persistent
container host remains the recommended production backend until that test is
performed.
