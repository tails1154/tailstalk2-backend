# TailsTalk 2 privacy and GDPR operations

This is an operational checklist, not legal advice. Keep it synchronized with
the public policy at `https://tailstalk2.com/privacy`.

## Data inventory

- Accounts: email, password hash, MFA settings, sessions, deletion state, and
  registration IP timestamp.
- Users and communities: usernames, profiles, memberships, messages, reactions,
  uploads, reports, moderation records, audit logs, and notification state.
- Infrastructure records: MongoDB data, MinIO objects, SMTP delivery records,
  LiveKit call infrastructure, and optional Stripe/CAPTCHA/diagnostics data.

Do not put plaintext passwords, payment-card numbers, or service credentials in
the repository. Keep deployment secrets in the private override configuration.
The production compose stack must also replace the default RabbitMQ and MinIO
credentials in its private `Revolt.toml`, `compose.yml`, and initialization
environment before exposure to the internet. Rotate those credentials together
and restart/validate every dependent service; changing only one side can cause
an outage. The checked-in template uses `CHANGE_ME` placeholders to prevent
accidental reuse of public defaults.

## Retention and deletion

- Registration IP data is automatically removed after the configured
  `api.privacy.registration_ip_retention_seconds` period (30 days in the
  shipped configuration).
- Server audit logs expire after the configured `api.audit_logs.expires_after`
  period (30 days in the shipped configuration).
- Account deletion is MFA/password protected, optionally email-confirmed, and
  scheduled with a seven-day grace period. The deletion worker then removes
  the account's messages, owned communities, sessions, settings, unread state,
  XP, MFA tickets, OAuth applications/grants/tokens, rate-limit records, boosts,
  bot/tailslet/video ownership, and other private records.
- Uploaded files are marked for deletion and removed by the file-deletion
  worker. Reported files and moderation records may be retained as safety or
  legal evidence; the deleted account's report author ID is anonymized.
- Operational invite/gift redemption records are retained only as needed to
  prevent reuse, with the deleted account reference removed or anonymized.
- Backups may retain deleted data until their documented rotation period.

## Rights requests

The public policy directs access, correction, restriction, portability, and
objection requests to `tails1154@tails1154.com`. Verify the requester using the
account email or another proportionate method, record the request date and
scope, search both database and object storage, and respond within one month
unless a lawful extension is communicated. Use `/delete-account` for deletion.

For a verified access request, an operator can create a redacted local export:

```sh
./scripts/export-user-data.sh USER_ID /secure/data-exports
```

The export excludes password hashes, MFA secrets, session/reset tokens, and
webhook tokens. Uploaded binary content requires a separate identity-verified
handoff; do not publish the export directory.

Run the read-only deployment gate before describing the installation as
hardened:

    ./scripts/audit-privacy-deployment.sh

It checks the public privacy and deletion pages, API availability, running
services, replacement of default broker/object-storage credentials, and
encrypted-backup configuration. A failing credential or backup check is
intentional and requires operator action; the script never changes data or
service state.

## Security and incident response

Review admin access and secrets regularly. If a breach is suspected, preserve
logs, contain the affected credential or service, identify affected data and
users, and consult the operator's legal/privacy contact promptly about any
required supervisory-authority or user notification.

Before claiming production compliance, the operator must document the actual
SMTP, storage, LiveKit, payment, CAPTCHA, and diagnostics providers, their
locations, and transfer safeguards as part of the operator's internal records.
The public policy identifies Tails1154 and the privacy email address; no postal
address is published. The operator should obtain jurisdiction-specific legal
review of that contact arrangement rather than inventing an address.

The production broker and object storage are not published as host ports, so
their current credentials are internal-only defaults rather than internet
credentials. Replacing them remains recommended defense in depth. Encrypted
MongoDB/MinIO backups are configured under /mnt/backup/tailstalk2-gdpr,
protected by a mode-0600 passphrase file, retained for 30 days, and included
in the existing nightly backup runner. The readiness audit must pass before
claiming that these operational controls are active.
