#!/usr/bin/env bash
set -euo pipefail

# Read-only production privacy/security readiness audit.
# Override these when auditing another deployment:
#   PUBLIC_ORIGIN, API_ORIGIN

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

local_result="$(python3 - <<"PY"
from pathlib import Path

compose = Path("/home/tails1154/stoat/compose.yml")
config = Path("/home/tails1154/stoat/Revolt.toml")
text = "\n".join(
    path.read_text(errors="ignore")
    for path in (compose, config)
    if path.exists()
)

default_credentials = any(
    value in text for value in ("rabbitpass", "rabbituser", "minioautumn")
)
import subprocess

published = any(
    subprocess.run(
        ["docker", "port", container],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()
    for container in ("stoat-rabbit-1", "stoat-minio-1")
)
if default_credentials and published:
    print("service_credentials=exposed-defaults")
elif default_credentials:
    print("service_credentials=internal-only-defaults")
else:
    print("service_credentials=replaced")
print("backup_configured=" + str(
    Path("/home/tails1154/backup/tailstalk2-backup.env").is_file()
    and Path("/home/tails1154/backup/tailstalk2.passphrase").is_file()
    and (Path("/home/tails1154/backup/tailstalk2.passphrase").stat().st_mode & 0o777) == 0o600
    and any(Path("/mnt/backup/tailstalk2-gdpr").glob("tailstalk2-*.tar.gpg"))
).lower())
PY
)" || {
  printf 'FAIL %-24s unavailable\n' "local audit"
  failed=1
  local_result=""
}

if [[ -n "$local_result" ]]; then
  service_credentials="$(printf '%s\n' "$local_result" | sed -n '1p')"
  backup_configured="$(printf '%s\n' "$local_result" | sed -n '2p')"
  running_count="$(docker compose -f /home/tails1154/stoat/compose.yml ps --status running -q | wc -l)"

  if [[ "$service_credentials" == "service_credentials=exposed-defaults" ]]; then
    printf 'FAIL %-24s publicly exposed defaults detected\n' "service credentials"
    failed=1
  elif [[ "$service_credentials" == "service_credentials=internal-only-defaults" ]]; then
    printf 'PASS %-24s internal-only defaults (not published)\n' "service credentials"
  else
    printf 'PASS %-24s replaced\n' "service credentials"
  fi

  if [[ "$backup_configured" == "backup_configured=true" ]]; then
    printf 'PASS %-24s configured\n' "encrypted backups"
  else
    printf 'FAIL %-24s destination/passphrase missing\n' "encrypted backups"
    failed=1
  fi

  plaintext_archives="$(find /mnt/backup/tailstalk -maxdepth 1 -type f -name '*.tar.gz' 2>/dev/null | wc -l)"
  if [[ "$plaintext_archives" == "0" ]]; then
    printf 'PASS %-24s no plaintext archives\n' "backup privacy"
  else
    printf 'FAIL %-24s %s plaintext archives found\n' "backup privacy" "$plaintext_archives"
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
