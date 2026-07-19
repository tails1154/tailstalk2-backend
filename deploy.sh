#!/bin/bash

cd "$(dirname "$0")"

echo "=== Building revolt-delta ==="
cargo build --release -p revolt-delta -vv 2>&1

echo "=== Copying binary ==="
mkdir -p deploy
scp -P 1699 target/release/revolt-delta deploy/revolt-delta tails1154.com:/home/tails1154/stoat/backend/target/release/revolt-delta

echo "=== Building Docker image ==="
ssh -p 1699 tails1154.com <<EOF
cd /home/tails1154/stoat/backend
echo "======"
docker build -t revolt-delta:local -f deploy/Dockerfile deploy/

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
docker compose down
docker compose up -d
EOF
echo "=== Done ==="
