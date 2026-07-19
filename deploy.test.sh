#!/bin/bash
set -eou pipefail

cd "$(dirname "$0")"

echo "=== Building revolt-delta (debug) ==="
cargo build -p revolt-delta 2>&1

echo "=== Copying binary ==="
mkdir -p deploy
cp target/debug/revolt-delta deploy/revolt-delta
scp -P 1699 deploy/revolt-delta tails1154.com:/home/tails1154/stoat/backend/deploy/revolt-delta

echo "=== Building Docker image ==="
ssh -p 1699 tails1154.com <<EOF
cd /home/tails1154/stoat/backend/
echo "======"
docker build --no-cache -t revolt-delta:local -f deploy/Dockerfile deploy/

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
"

echo "=== Restarting API service ==="
cd /home/tails1154/stoat
docker compose down api
docker compose up -d api
echo "=== Done ==="
EOF
