#!/bin/bash
set -e

cd "$(dirname "$0")"

echo "=== Building revolt-delta ==="
cargo build --release -p revolt-delta -vv 2>&1

echo "=== Copying binary ==="
mkdir -p deploy
cp target/release/revolt-delta deploy/revolt-delta

echo "=== Building Docker image ==="
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
docker compose -f ../compose.yml up -d api

echo "=== Done ==="
