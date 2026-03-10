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

echo "Upserting user $EMAIL..."
psql "$DATABASE_URL" \
  -v "vemail=$EMAIL" \
  -v "vhash=$HASH" \
  -c "INSERT INTO users (email, name, password_hash)
      VALUES (:'vemail', :'vemail', :'vhash')
      ON CONFLICT (email) DO UPDATE SET password_hash = :'vhash';"

echo "Done! User $EMAIL can now log in with local auth."
