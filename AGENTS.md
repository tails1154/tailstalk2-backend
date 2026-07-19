# AGENTS.md

A Rust monorepo (workspace resolver 3) for the Revolt/Stoat chat backend. MSRV: Rust 1.86+ (toolchain pins 1.92.0).

## Commands

```bash
# Managed via mise (see .mise/config.toml and .mise/tasks/):
mise build           # cargo build
mise check           # cargo clippy
mise test            # cargo nextest run (defaults to TEST_DB=REFERENCE)

# Manual equivalents:
cargo build
cargo clippy
cargo nextest run    # requires cargo-nextest; pass TEST_DB=REFERENCE or TEST_DB=MONGODB

# Run individual services:
cargo run --bin revolt-delta       # Rocket REST API (port 14702)
cargo run --bin revolt-bonfire     # WebSocket events (port 14703)
cargo run --bin revolt-autumn      # Axum file server (port 14704)
cargo run --bin revolt-january     # Axum link proxy/embed scraper (port 14705)
cargo run --bin revolt-gifbox      # Axum Tenor proxy (port 14706)
cargo run --bin revolt-crond       # Cron daemon
cargo run --bin revolt-pushd       # Push notification daemon
cargo run --bin revolt-voice-ingress # Voice ingress webhook (port 8500)
```

## Testing

- **Test runner**: `cargo-nextest`. Config at `.config/nextest.toml` (5s slow timeout).
- **TEST_DB env var**: `REFERENCE` (in-memory, default) or `MONGODB` (requires `docker compose up -d` first).
- **Test fixture pattern**: `database_test!(|db| async move { ... })` at `crates/core/database/src/lib.rs`. Creates an isolated test database, runs the closure, drops the DB.
- **JSON fixtures** at `crates/core/database/fixtures/` use `__ID:N__` placeholders replaced with real ULIDs at load time. Reference them via `fixture!` macro and `FetchFixture` trait.
- `docker compose up -d` is required for MongoDB-backed tests. Required services: MongoDB (27017), Redis (6379), RabbitMQ (5672), MinIO (14009), Maildev (14025/14080).
- Integration tests at `crates/core/files/tests/` test S3 directly.

## Architecture

### Crate tree

```
crates/
  core/
    config/       Config loading (Revolt.toml + overrides + env), Sentry init, logging
    database/     Trait-based repo pattern: AbstractDatabase = 24 entity traits
    models/       API v0 DTOs (serde + JsonSchema + OpenAPI). NOT the DB models.
    result/       Error enum with Rocket/Axum HTTP conversions
    permissions/  Bitfield permission calculations
    presence/     Redis-based user presence tracking
    files/        S3 storage, image processing, encryption
    ratelimits/   Rate limiting (Rocket and Axum integrations)
    parser/       Message content parsing (logos)
    coalesced/    Event coalescing/queue
  delta/          Main REST API — Rocket v0.5, port 14702
  bonfire/        WebSocket events gateway — raw tokio + async-tungstenite, port 14703
  services/
    autumn/       File upload/processing — Axum, port 14704
    january/      Link proxy + embed scraper — Axum, port 14705, NO database dep
    gifbox/       Tenor GIF search proxy — Axum, port 14706
  daemons/
    crond/        Scheduled cleanup tasks (files, members, accounts, acks)
    pushd/        Push notifications via RabbitMQ consumers (APN + FCM + VAPID)
    voice-ingress/ LiveKit voice webhook ingress — Rocket, port 8500
```

### Database abstraction

The `Database` enum (`Reference` | `MongoDb`) dispatches via `Deref<Target = dyn AbstractDatabase>`. Each entity has its own `ops.rs` trait (e.g., `AbstractUsers`) implemented separately for both drivers. See `crates/core/database/src/models/`.

**Two separate model layers**:
- `revolt_models::v0::*` — API-facing DTOs (what clients see)
- `revolt_database::User`, `revolt_database::Server`, etc. — DB models with `_id`, business logic, and `Partial<Foo>` update types
- Conversion via `.into()` / `.into_known(perspective, is_online)`

**Database connection auto-detection** (`DatabaseInfo::Auto`):
1. If `TEST_DB` env is set → test database
2. If `config.database.mongodb` is non-empty → MongoDB
3. Otherwise → `ReferenceDb` (in-memory, for development)

### Error handling (`revolt-result`)

- `create_error!(NotFound)` — simple error
- `create_error!(MissingPermission { permission: "SendMessage" })` — with fields
- `create_database_error!("find", "users")` — DB errors
- Macros auto-capture `file!()`, `line!()`, `column!()`

### Configuration

Layered loading order:
1. Embedded `Revolt.toml` (compiled into binary, at `crates/core/config/Revolt.toml`)
2. `Revolt.toml` + `Revolt.overrides.toml` searched up from CWD
3. If `TEST_DB` set: `Revolt.test.toml` + `Revolt.test-overrides.toml`
4. `REVOLT__`-prefixed env vars (double underscore = nested key)

The default `Revolt.toml` at repo root is development-only. Production overrides go in `Revolt.overrides.toml` (gitignored). Config is cached for 30 seconds via `#[cached(time = 30)]`.

## Conventions

- **Clippy rules** at `clippy.toml`: disallow direct `insert_*` ops (use `Object::create()`), direct `delete_*` (use `Object::delete()`), direct `update_*` (use `Object::update()`). Also disallows some internal DB mutators.
- **Release profile**: `lto = false`, `codegen-units = 16`, `strip = "debuginfo"` — this is intentional for faster CI builds, not production optimization.
- **Patch crate**: `redis` v0.23.3 is patched to a revoltchat fork.
- **AMQP queues** are suffixed `-prd` or `-tst` based on config flag, not env detection.
- **Two HTTP frameworks in use**: Rocket (delta, voice-ingress) and Axum (autumn, january, gifbox). Axum is the newer choice for microservices.
- **Bonfire** stores its `Database` in a static `OnceCell` instead of framework state — this is unique to bonfire.
- **January** is the only service with zero database dependency — completely stateless proxy.
- **Crond** wraps each cron task in `AssertUnwindSafe`; tasks retry after 60s on panic.
- **Release versioning**: `release-please` manages the monorepo version. All crate versions match the workspace version (0.14.1). Use `just patch`/`just minor`/`just major` + `just publish`.

## Dev environment

- Required tools: mise, Docker, Git. Optional: mold (set `BUILDER = "mold --run cargo"` in `.env`).
- `mise start` brings up Docker services, builds, and runs all services.
- `mise docker:stop` tears down Docker services.
- Email verification is disabled by default in dev config. Find emails at http://localhost:14080 (Maildev).
- `livekit.yml` must exist (copy from `livekit.example.yml`) for voice features.
