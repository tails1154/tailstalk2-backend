#!/usr/bin/env bash
set -euo pipefail

api_url="${TAILSTALK_OAUTH_API_URL:-https://tails1154.com:9961}"
: "${TAILSTALK_SESSION_TOKEN:?Set TAILSTALK_SESSION_TOKEN to an existing authenticated Tailstalk session token}"

if [[ "$#" -ne 0 ]]; then
  echo "Usage: TAILSTALK_SESSION_TOKEN=... $0 < registration.json" >&2
  exit 2
fi

curl --fail-with-body --silent --show-error \
  -X POST "$api_url/api/oauth/applications" \
  -H "Content-Type: application/json" \
  -H "X-Session-Token: $TAILSTALK_SESSION_TOKEN" \
  --data-binary @-
