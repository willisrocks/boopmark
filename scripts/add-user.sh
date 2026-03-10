#!/usr/bin/env bash
set -euo pipefail

# Load .env if present
if [ -f .env ]; then
  set -a; source .env; set +a
fi

EMAIL="${1:-}"
PASSWORD="${2:-}"

if [ -z "$EMAIL" ]; then
  read -rp "Email: " EMAIL
fi
if [ -z "$PASSWORD" ]; then
  read -rsp "Password: " PASSWORD
  echo
fi

if [ -z "$EMAIL" ] || [ -z "$PASSWORD" ]; then
  echo "Error: email and password are required" >&2
  exit 1
fi

echo "Hashing password..."
HASH=$(cargo run -p boopmark-server --example hash_password -- "$PASSWORD" 2>/dev/null)

# Find the devproxy db container (pattern: *-boopmark-db-1)
DB_CONTAINER=$(docker ps --filter "name=boopmark-db-1" --format "{{.Names}}" | head -1)
if [ -z "$DB_CONTAINER" ]; then
  echo "Error: no running boopmark db container found. Run 'devproxy up' first." >&2
  exit 1
fi

echo "Upserting user $EMAIL..."
docker exec "$DB_CONTAINER" psql -U boopmark -d boopmark \
  -c "INSERT INTO users (email, name, password_hash)
      VALUES ('$EMAIL', '$EMAIL', '$HASH')
      ON CONFLICT (email) DO UPDATE SET password_hash = EXCLUDED.password_hash;"

echo "Done! User $EMAIL can now log in with local auth."
