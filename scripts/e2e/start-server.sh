#!/usr/bin/env bash
set -euo pipefail

# devproxy 0.4.x creates a new stack on every `up`. Reuse only a database
# belonging to this checkout and accompanied by its devproxy-labelled server.
repo_dir="$(pwd -P)"
existing_db=""
for container in $(docker ps --filter "label=com.docker.compose.project.working_dir=$repo_dir" --filter label=com.docker.compose.service=db --format '{{.ID}}'); do
  if docker port "$container" 5432/tcp | grep -Eq ':5434$'; then
    project="$(docker inspect "$container" --format '{{index .Config.Labels "com.docker.compose.project"}}')"
    proxy_server="$(docker ps -a --filter "label=com.docker.compose.project=$project" --filter label=devproxy.port=4000 --format '{{.ID}}')"
    if [ -n "$proxy_server" ]; then existing_db="$container"; break; fi
  fi
done
if [ -z "$existing_db" ]; then
  if bash -lc "exec 3<>/dev/tcp/127.0.0.1/5434" > /dev/null 2>&1; then
    echo "Port 5434 is occupied by an unverified database; refusing to use it." >&2
    exit 1
  fi
  devproxy up
fi

ready=0
for attempt in {1..60}; do
  if bash -lc "exec 3<>/dev/tcp/127.0.0.1/5434" > /dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "Postgres did not become ready on port 5434 after devproxy up" >&2
  exit 1
fi

# The devproxy database is intentionally reused, but the browser suite expects
# a clean account. Remove only the dedicated E2E identity and its dependent
# records so repeated runs cannot inherit bookmarks or API keys from an older
# run. Invites do not cascade from users, so remove those references first.
e2e_db=""
for container in $(docker ps --filter "label=com.docker.compose.project.working_dir=$repo_dir" --filter label=com.docker.compose.service=db --format '{{.ID}}'); do
  if docker port "$container" 5432/tcp | grep -Eq ':5434$'; then
    e2e_db="$container"
    break
  fi
done
if [ -z "$e2e_db" ]; then
  echo "Could not resolve the verified E2E database container" >&2
  exit 1
fi

reset_ready=0
for attempt in {1..60}; do
  if docker exec "$e2e_db" psql -U boopmark -d boopmark -v ON_ERROR_STOP=1 -c \
    "DELETE FROM invites WHERE created_by IN (SELECT id FROM users WHERE email = 'e2e@boopmark.local') OR claimed_by IN (SELECT id FROM users WHERE email = 'e2e@boopmark.local'); DELETE FROM users WHERE email = 'e2e@boopmark.local';" \
    > /dev/null 2>&1; then
    reset_ready=1
    break
  fi
  sleep 1
done
if [ "$reset_ready" -ne 1 ]; then
  echo "Could not reset the dedicated E2E account" >&2
  exit 1
fi

export DATABASE_URL=postgres://boopmark:devpassword@127.0.0.1:5434/boopmark
export ENABLE_E2E_AUTH=1
export LOGIN_ADAPTER=local_password
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

if [ -f .env ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  export ANTHROPIC_API_KEY="$(awk -F= '/^ANTHROPIC_API_KEY=/{print substr($0, index($0,$2))}' .env)"
fi

if [ -f .env ] && [ -z "${LLM_SETTINGS_ENCRYPTION_KEY:-}" ]; then
  export LLM_SETTINGS_ENCRYPTION_KEY="$(awk -F= '/^LLM_SETTINGS_ENCRYPTION_KEY=/{print substr($0, index($0,$2))}' .env)"
fi

if [ -z "${LLM_SETTINGS_ENCRYPTION_KEY:-}" ]; then
  echo "LLM_SETTINGS_ENCRYPTION_KEY must exist in the copied worktree .env or environment" >&2
  exit 1
fi

exec cargo run -p boopmark-server
