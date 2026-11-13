# TailsTalk 2 Backend

The backend for TailsTalk 2, a modified fork of the [Stoat](https://github.com/stoatchat/stoatchat) backend (originally Revolt). It is run as a single instance at `https://tails1154.com`.

Changes from upstream Stoat include:

- **AutoMod:** per-server rules that block or moderate messages, enforced on message send and edit.
- **AISD student IDs:** accounts on `cats.angletonisd.net` must use a student ID that starts with 4 and has six digits.
- **Marketing list sync:** users who do not opt out at signup are added to a Brevo contact list. AISD addresses go to a separate list.
- **Email verification:** verification mail is sent through Brevo SMTP, and admins can verify an address manually.
- **Suspensions:** suspended and banned accounts are rejected on every request, and suspensions end automatically at their expiry time.
- **Signed server backups:** server exports are signed with Ed25519, and restores reject any file whose signature does not verify.
- **Backup permissions:** `BackupServer` and `RestoreServer` control who can export and restore backups.
- **GitHub webhooks:** `/webhooks/<id>/<token>/github` accepts GitHub events, and `ping` is acknowledged.
- **Admin panel API:** user search (by username, ID, or email), warnings, suspensions, bans, badges, reports, and a pending-verification list.

## Crates

| Crate | Path | Description |
| --- | --- | --- |
| `core/config` | [crates/core/config](crates/core/config) | Configuration loading |
| `core/database` | [crates/core/database](crates/core/database) | Database models and MongoDB implementation |
| `core/models` | [crates/core/models](crates/core/models) | API request and response models |
| `core/permissions` | [crates/core/permissions](crates/core/permissions) | Permission bits and calculation |
| `core/result` | [crates/core/result](crates/core/result) | Error types |
| `delta` | [crates/delta](crates/delta) | REST API server |
| `bonfire` | [crates/bonfire](crates/bonfire) | WebSocket events server |
| `services/january` | [crates/services/january](crates/services/january) | Proxy |
| `services/gifbox` | [crates/services/gifbox](crates/services/gifbox) | GIF proxy |
| `services/autumn` | [crates/services/autumn](crates/services/autumn) | File server |
| `daemons/crond` | [crates/daemons/crond](crates/daemons/crond) | Scheduled clean-up |
| `daemons/pushd` | [crates/daemons/pushd](crates/daemons/pushd) | Push notifications |

## Requirements

- Rust 1.92 (the toolchain this project builds with)
- MongoDB running as a replica set. Account and user updates use transactions, so a standalone server will fail some requests.
- Redis, RabbitMQ, and an S3-compatible store for files, as in upstream Stoat

## Configuration

Settings live in `Revolt.toml`. Local overrides go in `Revolt.overrides.toml`. Never commit real credentials.

The sections this fork relies on:

```toml
[api.registration]
invite_only = false
email_verification = true   # reported to clients as `features.email`

[api.smtp]
host = "smtp-relay.brevo.com"
port = 587
use_starttls = true
use_tls = false
username = "<brevo smtp login>"
password = "<brevo smtp key>"
from_address = "verify@tails1154.com"
reply_to = "support@tails1154.com"

[api.marketing]
brevo_api_key = "<brevo api key>"
brevo_list_id = 5           # all other signups
brevo_aisd_list_id = 6      # cats.angletonisd.net addresses; 0 uses brevo_list_id

[api.backup]
signing_key = "<base64 32-byte Ed25519 seed>"   # empty disables backup export and restore
```

Generate the backup key with `head -c 32 /dev/urandom | base64 -w0`. Changing it invalidates every backup signed with the old key.

Leaving `[api.marketing].brevo_api_key` empty turns marketing sync off. Leaving `[api.smtp].host` empty turns off email verification.

## Server backups

`GET /servers/<id>/backup` exports the server's name, description, roles, categories, and default permissions as JSON, signed with the instance key. `POST /servers/<id>/backup` restores such a file.

- Export requires `BackupServer` in the server.
- Restore requires `RestoreServer` in the server.
- Backups do not need to come from the server they are restored into, so they can be used as templates.
- Restore replaces the server's roles with the ones in the file. Channels are not included in backups yet.

## Development

Upstream Stoat's development setup (mise, Docker services, and the `local` port layout) still applies. See the [Stoat developer docs](https://developers.stoat.chat/developing/contrib/).

Run the test suite with:

```sh
cargo test
```

Some database tests need a running MongoDB and are marked `#[ignore]`. Run them with `cargo test -- --ignored` once MongoDB is available.

## Deployment

Production deployment is done by `./deploy.sh`. It builds the release API and events binaries, builds the container images, and restarts the services on the server over SSH. Use the script rather than running the steps by hand.

The `deploy/` directory holds Dockerfiles and the binaries copied in during a build. The binaries are ignored by git.

## License

The backend is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE). Individual crates may carry their own licenses.
