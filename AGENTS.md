# AGENTS.md

## Toolchain & Task Runner

- **Rust 1.92.0** (`rust-toolchain.toml`). Managed via mise.
- **mise** is the task runner. Use `mise <task>` — not raw `cargo` directly:
  - `mise build` — compile everything
  - `mise check` — `cargo clippy` (also the only lint step; no separate formatter)
  - `mise test` — `cargo nextest run` (defaults `TEST_DB=REFERENCE`)
  - `mise start` — build + start docker + run all services in parallel
  - `mise docker:start` / `mise docker:stop` — Docker compose lifecycle
- Tasks are defined as shell scripts under `.mise/tasks/`.

## Configuration & Environment

- **`Revolt.toml`** is the default dev config. To override, create `Revolt.overrides.toml` (gitignored by `Revolt.*.toml` pattern).
- `.env` is gitignored — set env vars there (e.g. `BUILDER=mold --run cargo` for faster builds with mold).
- Rust server processes use **Rocket** (`revolt-delta`) for the REST API and **Axum** (`revolt-bonfire`) for WebSocket events. Do not assume a single HTTP framework.
- OpenTelemetry and Sentry integration exist but are optional; don't assume they are enabled.

## Architecture

| Directory | Binary | Purpose |
|---|---|---|
| `crates/delta` | `revolt-delta` | REST API server (port 14702) — **the main deployable** |
| `crates/bonfire` | `revolt-bonfire` | WebSocket events server (port 14703) |
| `crates/services/autumn` | `revolt-autumn` | File server via S3/MinIO (port 14704) |
| `crates/services/january` | `revolt-january` | Proxy server (port 14705) |
| `crates/services/gifbox` | `revolt-gifbox` | Tenor GIF proxy (port 14706) |
| `crates/daemons/crond` | `revolt-crond` | Timed data cleanup daemon |
| `crates/daemons/pushd` | `revolt-pushd` | Push notification daemon |
| `crates/daemons/voice-ingress` | — | LiveKit voice ingress daemon |
| `crates/core/*` | — | Shared libraries (config, database, models, permissions, presence, result, etc.) |

- **Only `revolt-delta` is deployed** to production (via `deploy.sh`).
- Delta uses **Rocket** (0.5), not Axum. Bonfire uses **Axum**.
- `crates/core/database` has two backends: **MongoDB** (production) and **Reference** (in-memory mock for tests). The `TEST_DB` env var selects one.
- Voice features are gated behind the `voice` feature flag on `revolt-database`.

## Testing

- Tests use `cargo nextest` (installed via mise as `cargo-nextest`).
- `docker compose up -d` must be running for MongoDB-backed tests.
- Two test modes:
  - `TEST_DB=REFERENCE cargo nextest run` — fast, no external DB needed
  - `TEST_DB=MONGODB cargo nextest run` — requires Docker services
- `mise test` defaults to `TEST_DB=REFERENCE`.
- Slow test timeout: 5s period / 10s termination (`.config/nextest.toml`).
- Database tests use the `database_test!` macro (defined in `crates/core/database/src/lib.rs:170`).

## Local Infrastructure (`docker compose`)

| Service | Port |
|---|---|
| KeyDB (Redis) | 6379 |
| MongoDB (replica set `rs0`) | 27017 |
| MinIO (S3) | 14009 (API), 14010 (console) |
| RabbitMQ | 5672 (AMQP), 15672 (management UI) |
| Maildev | 14025 (SMTP), 14080 (web UI for emails) |
| LiveKit | host networking |

- Copy `livekit.example.yml` to `livekit.yml` before starting.
- Registration email verification is disabled in `Revolt.toml`. Find test emails at `http://localhost:14080`.

## Deploy

- `deploy.sh` builds `revolt-delta` in release mode, copies the binary to `tails1154.com:1699`, then rebuilds the Docker image and restarts the API service remotely.
- Docker multi-stage build script: `scripts/build-image-layer.sh`.
- Release profile: LTO off, 16 codegen-units, strip debuginfo.

## Conventions & Gotchas

- Clippy has disallowed-methods rules (`clippy.toml`): prefer `Object::create()`, `Object::update()`, `Object::delete()` over raw `insert_*`/`update_*`/`delete_*` methods.
- A patched fork of `redis` (redis-rs) is used via `[patch.crates-io]`; do not bump the redis crate version blindly.
- The `mongodb` feature flag is required for production builds but optional for tests.
- There are no CI workflows in this repo. All checks are local (`mise check`, `mise test`).
- `git-town` is configured for branching (`main` branch, GitHub forge).
- The `docs/` directory is an unrelated Docusaurus site (Node.js/pnpm); ignore it for backend work.
- Use `schemars` for JSON schema generation in models, not `utoipa`.
