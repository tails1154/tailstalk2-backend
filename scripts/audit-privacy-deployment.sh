#!/usr/bin/env bash
set -euo pipefail

# Read-only production privacy/security readiness audit.
# Override these when auditing another deployment:
#   REMOTE_HOST, REMOTE_PORT, REMOTE_USER, PUBLIC_ORIGIN, API_ORIGIN

REMOTE_HOST="${REMOTE_HOST:-tails1154.com}"
REMOTE_PORT="${REMOTE_PORT:-1699}"
REMOTE_USER="${REMOTE_USER:-tails1154}"
PUBLIC_ORIGIN="${PUBLIC_ORIGIN:-https://tailstalk2.com}"
API_ORIGIN="${API_ORIGIN:-https://tails1154.com:9961}"

failed=0

check_http() {
  local name="$1" url="$2" expected="$3" status
  status="$(curl --silent --show-error --location --max-time 20 \
    --output /dev/null --write-out '%{http_code}' "$url")" || status="000"
  if [[ "$status" == "$expected" ]]; then
    printf 'PASS %-24s %s\n' "$name" "$status"
  else
    printf 'FAIL %-24s expected %s, got %s\n' "$name" "$expected" "$status"
    failed=1
  fi
}

check_http "privacy page" "$PUBLIC_ORIGIN/privacy" "200"
check_http "deletion page" "$PUBLIC_ORIGIN/delete-account" "200"
check_http "API health" "$API_ORIGIN/api" "200"

remote_result="$(ssh -p "$REMOTE_PORT" -o BatchMode=yes -o ConnectTimeout=10 \
  "$REMOTE_USER@${REMOTE_HOST}" 'python3 - <<"PY"
from pathlib import Path

compose = Path("/home/tails1154/stoat/compose.yml")
config = Path("/home/tails1154/stoat/Revolt.toml")
text = "\n".join(
    path.read_text(errors="ignore")
    for path in (compose, config)
    if path.exists()
)

print("default_credentials=" + str(
    any(value in text for value in ("rabbitpass", "rabbituser", "minioautumn"))
).lower())
print("backup_configured=" + str(
    bool(__import__("os").environ.get("BACKUP_ROOT"))
    and bool(__import__("os").environ.get("BACKUP_PASSPHRASE_FILE"))
).lower())
PY
docker compose -f /home/tails1154/stoat/compose.yml ps --status running -q | wc -l' 2>/dev/null)" || {
  printf 'FAIL %-24s unavailable\n' "remote audit"
  failed=1
  remote_result=""
}

if [[ -n "$remote_result" ]]; then
  default_credentials="$(printf '%s\n' "$remote_result" | sed -n '1p')"
  backup_configured="$(printf '%s\n' "$remote_result" | sed -n '2p')"
  running_count="$(printf '%s\n' "$remote_result" | sed -n '3p')"

  if [[ "$default_credentials" == "default_credentials=False" ]]; then
    printf 'PASS %-24s replaced\n' "service credentials"
  else
    printf 'FAIL %-24s production defaults detected\n' "service credentials"
    failed=1
  fi

  if [[ "$backup_configured" == "backup_configured=True" ]]; then
    printf 'PASS %-24s configured\n' "encrypted backups"
  else
    printf 'FAIL %-24s destination/passphrase missing\n' "encrypted backups"
    failed=1
  fi

  if [[ "$running_count" =~ ^[0-9]+$ ]] && (( running_count > 0 )); then
    printf 'PASS %-24s %s containers running\n' "compose services" "$running_count"
  else
    printf 'FAIL %-24s no running containers reported\n' "compose services"
    failed=1
  fi
fi

exit "$failed"
