#!/usr/bin/env bash
set -euo pipefail

docker compose up -d db > /tmp/boopmark-e2e-db.log 2>&1 || true

until bash -lc "exec 3<>/dev/tcp/127.0.0.1/5434" > /dev/null 2>&1; do
  sleep 1
done

export DATABASE_URL=postgres://boopmark:devpassword@127.0.0.1:5434/boopmark
export ENABLE_E2E_AUTH=1
export APP_URL=http://127.0.0.1:4010
export PORT=4010
export SESSION_SECRET=e2e-session-secret
export GOOGLE_CLIENT_ID=e2e-google-client-id
export GOOGLE_CLIENT_SECRET=e2e-google-client-secret
export STORAGE_BACKEND=local
export S3_ENDPOINT=http://127.0.0.1:9000
export S3_BUCKET=boopmark
export S3_ACCESS_KEY=minioadmin
export S3_SECRET_KEY=minioadmin
export S3_REGION=us-east-1

exec cargo run -p boopmark-server
