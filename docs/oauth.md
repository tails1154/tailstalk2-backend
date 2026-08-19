# TailsTalk 2 OAuth 2.1

TailsTalk 2 exposes an OAuth 2.1 authorization-code flow with PKCE. The
implicit and resource-owner-password grants are not supported.

## URLs

For this deployment:

- Authorization: `https://tails1154.com:9961/api/oauth/authorize`
- Token: `https://tails1154.com:9961/api/oauth/token`
- Revocation: `https://tails1154.com:9961/api/oauth/revoke`
- Userinfo: `https://tails1154.com:9961/api/oauth/userinfo`
- Applications: `https://tails1154.com:9961/api/oauth/applications/@me`

The authorization page requires the existing Tailstalk session (`X-Session-Token`)
and never asks an OAuth client for a Tailstalk password.

## Register an application

An authenticated user creates an application with `POST /api/oauth/applications`:

```json
{
  "name": "Obsidian Dashboard",
  "redirect_uris": ["https://obsidian.tails1154.com/auth/callback"],
  "allowed_scopes": ["identify", "servers", "server_members", "permissions"],
  "public": false
}
```

Send JSON with the existing `X-Session-Token` header. The response contains a
`client_id` and the client secret once. Store the secret in a server-side
secret manager; it is hashed before storage and is never returned by listing.
Use `POST /api/oauth/applications/{client_id}/rotate-secret` to rotate it. Revoke
an application with `POST /api/oauth/applications/{client_id}/revoke`; this also
revokes its access and refresh tokens.

If the dashboard cannot keep a secret confidential, set `public` to `true` and
use PKCE S256. Public clients are required to send a PKCE challenge.

## Authorization request

```text
GET /api/oauth/authorize?
  response_type=code&
  client_id=CLIENT_ID&
  redirect_uri=https%3A%2F%2Fobsidian.tails1154.com%2Fauth%2Fcallback&
  scope=identify%20servers%20server_members%20permissions&
  state=RANDOM_STATE&
  code_challenge=BASE64URL_SHA256_VERIFIER&
  code_challenge_method=S256
```

Redirect URIs are exact string matches against the registered allowlist. HTTPS
is required, except for localhost development URLs. `state` is required and is
returned unchanged. The user must approve the clearly labelled consent page;
its one-time CSRF request is bound to the authenticated session.

PKCE example:

```js
const verifier = crypto.randomUUID() + crypto.randomUUID();
const bytes = new TextEncoder().encode(verifier);
const digest = await crypto.subtle.digest("SHA-256", bytes);
const challenge = btoa(String.fromCharCode(...new Uint8Array(digest)))
  .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
```

## Token exchange

Use `application/x-www-form-urlencoded` at the token endpoint:

```text
grant_type=authorization_code&code=CODE&client_id=CLIENT_ID&client_secret=CLIENT_SECRET&redirect_uri=https%3A%2F%2Fobsidian.tails1154.com%2Fauth%2Fcallback&code_verifier=VERIFIER
```

Access tokens expire after 10 minutes. Refresh tokens expire after 30 days and
are rotated on every use. A reused refresh token revokes its entire token
family. Authorization codes expire after 5 minutes and are single-use.

Refresh with `grant_type=refresh_token`, `refresh_token`, `client_id`, and the
confidential client secret when applicable. Revoke either token type by POSTing
`token` to the revocation endpoint. Tokens and codes are opaque and only their
SHA-256 digests are stored.

## Scopes and userinfo

- `identify`: `sub`, user ID, username, display name, and avatar ID.
- `servers`: servers the user currently belongs to.
- `server_members`: membership roles and join time for those servers.
- `permissions`: effective server and channel permissions calculated by the
  existing Stoat permission engine.

`GET /api/oauth/userinfo` requires `Authorization: Bearer ACCESS_TOKEN`. It returns
only fields covered by the token’s granted scopes. Server data is restricted to
the authenticated user’s current memberships:

```json
{
  "id": "server-id",
  "name": "Example",
  "owner": "user-id",
  "member": {"roles": ["role-id"], "joined_at": "..."},
  "permissions": 123,
  "channels": [{"id": "channel-id", "permissions": 456}]
}
```

Clients must not submit or trust permission values. In particular,
The dashboard can use the returned `permissions` value with the deployed Stoat
permission constants to determine capabilities such as server management, but
the server remains the authority.

## Errors

Token and userinfo failures use OAuth JSON errors:

```json
{"error":"invalid_grant","error_description":"Invalid or expired authorization code"}
```

Authorization failures use the same fields in a redirect only after the
redirect URI has been validated; otherwise they are returned directly to avoid
open redirects. `state` is included in valid error redirects.

## Obsidian Dashboard

Register the exact redirect URI
`https://obsidian.tails1154.com/auth/callback` with the four scopes listed
above. The dashboard should use `public: false`, keep the returned secret only
on its server, generate a fresh state and PKCE verifier per login, and exchange
the code server-to-server. No new environment variable is required by the
server; the issuer is `https://tails1154.com:9961`.

## Deployment and migration

The backend migration creates the five OAuth collections and indexes, including
TTL indexes for consent requests and authorization codes and unique digest
indexes for tokens. Run the workspace deployment script after pushing backend
changes:

```bash
cd /home/tails1154/Documents/vibecoding/tt2
./deploy.sh
```

It builds and restarts the backend before deploying the client. MongoDB TTL
cleanup removes expired one-time values; application revocation immediately
revokes associated access and refresh tokens.
