#!/bin/bash
set -eou pipefail

cd "$(dirname "$0")"

echo "=== Building revolt-delta ==="
cargo build --release -p revolt-delta -vv 2>&1

BONFIRE_CHANGED=0
if [[ ! -x target/release/revolt-bonfire ]] \
    || ! git diff --quiet HEAD -- crates/bonfire Cargo.toml Cargo.lock \
    || ! git diff --quiet HEAD^ HEAD -- crates/bonfire Cargo.toml Cargo.lock; then
    BONFIRE_CHANGED=1
    echo "=== Building revolt-bonfire ==="
    cargo build --release -p revolt-bonfire -vv 2>&1
else
    echo "=== Skipping revolt-bonfire (no Bonfire changes detected) ==="
fi

echo "=== Copying binary ==="
mkdir -p deploy
cp target/release/revolt-delta deploy/revolt-delta
scp -P 1699 deploy/revolt-delta tails1154.com:/home/tails1154/stoat/backend/deploy/revolt-delta
scp -P 1699 deploy/Dockerfile tails1154.com:/home/tails1154/stoat/backend/deploy/Dockerfile
if [[ "$BONFIRE_CHANGED" -eq 1 ]]; then
    cp target/release/revolt-bonfire deploy/revolt-bonfire
    scp -P 1699 deploy/revolt-bonfire tails1154.com:/home/tails1154/stoat/backend/deploy/revolt-bonfire
    scp -P 1699 deploy/bonfire.Dockerfile tails1154.com:/home/tails1154/stoat/backend/deploy/bonfire.Dockerfile
fi

echo "=== Building Docker image ==="
ssh -p 1699 tails1154.com <<EOF
cd /home/tails1154/stoat/backend/
echo "======"
docker build --no-cache -t revolt-delta:local -f deploy/Dockerfile deploy/
if [[ "$BONFIRE_CHANGED" -eq 1 ]]; then
    docker build --no-cache -t revolt-bonfire:local -f deploy/bonfire.Dockerfile deploy/
fi

echo "=== Updating compose to use local image ==="
python3 -c "
import yaml, sys

with open('../compose.yml') as f:
    data = yaml.safe_load(f)

if 'api' in data.get('services', {}):
    api = data['services']['api']
    if 'build' in api:
        del api['build']
    api['image'] = 'revolt-delta:local'
    with open('../compose.yml', 'w') as f:
        yaml.dump(data, f, default_flow_style=False)
    print('Updated compose.yml')
else:
    print('No api service found')
    sys.exit(1)

if "${BONFIRE_CHANGED}" == "1":
    if 'events' in data.get('services', {}):
        events = data['services']['events']
        events.pop('build', None)
        events['image'] = 'revolt-bonfire:local'
        with open('../compose.yml', 'w') as f:
            yaml.dump(data, f, default_flow_style=False)
        print('Updated events service')
    else:
        print('No events service found')
        sys.exit(1)
else:
    print('Kept existing events service image')
"

echo "=== Restarting API service ==="
cd /home/tails1154/stoat
docker compose down api
docker compose up -d api
if [[ "$BONFIRE_CHANGED" -eq 1 ]]; then
    echo "=== Restarting events service ==="
    docker compose down events
    docker compose up -d events
else
    echo "=== Keeping events service running ==="
fi
echo "=== Done ==="
EOF
