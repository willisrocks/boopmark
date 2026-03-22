#!/usr/bin/env bash
set -euo pipefail

# Load .env if present
if [ -f .env ]; then
  set -a; source .env; set +a
fi

# Defaults
ROLE="user"
PASSWORD=""
EMAIL=""

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --role)
      ROLE="$2"
      shift 2
      ;;
    --password)
      PASSWORD="$2"
      shift 2
      ;;
    *)
      if [ -z "$EMAIL" ]; then
        EMAIL="$1"
      fi
      shift
      ;;
  esac
done

# Interactive fallback
if [ -z "$EMAIL" ]; then
  read -rp "Email: " EMAIL
fi

# Validate role
case "$ROLE" in
  owner|admin|user) ;;
  *) echo "Error: --role must be owner, admin, or user" >&2; exit 1 ;;
esac

# Validate email
if [ -z "$EMAIL" ]; then
  echo "Error: email is required" >&2
  exit 1
fi

# Determine login adapter
LOGIN_ADAPTER="${LOGIN_ADAPTER:-local_password}"

# Password handling
if [ "$LOGIN_ADAPTER" = "local_password" ] && [ -z "$PASSWORD" ]; then
  read -rsp "Password: " PASSWORD
  echo
fi

if [ "$LOGIN_ADAPTER" = "local_password" ] && [ -z "$PASSWORD" ]; then
  echo "Error: password is required when LOGIN_ADAPTER=local_password" >&2
  exit 1
fi

# Owner guard: use docker compose exec to scope to this project's db
if [ "$ROLE" = "owner" ]; then
  EXISTING_OWNER=$(docker compose exec -T db psql -U boopmark -d boopmark -t -c \
    "SELECT COUNT(*) FROM users WHERE role = 'owner' AND deactivated_at IS NULL;" | tr -d ' ')
  if [ "$EXISTING_OWNER" -gt 0 ]; then
    echo "Error: an owner already exists. Use the admin panel to manage users." >&2
    exit 1
  fi
fi

# Hash password if provided — pass via stdin to avoid exposing in process listing
HASH=""
if [ -n "$PASSWORD" ]; then
  echo "Hashing password..."
  if command -v cargo > /dev/null 2>&1; then
    HASH=$(echo "$PASSWORD" | cargo run -p boopmark-server --example hash_password 2>/dev/null)
  else
    # Docker-only: exec into server container to hash via stdin
    HASH=$(echo "$PASSWORD" | docker compose exec -T server ./hash_password)
  fi
fi

echo "Creating $ROLE user $EMAIL..."
# Use psql -v variables to avoid SQL injection from user-supplied email/hash/role
docker compose exec -T db psql -U boopmark -d boopmark \
  -v email="$EMAIL" \
  -v hash="${HASH:-}" \
  -v role="$ROLE" \
  -c "INSERT INTO users (email, name, password_hash, role)
      VALUES (:'email', :'email', NULLIF(:'hash', ''), :'role')
      ON CONFLICT (email) DO UPDATE SET
        password_hash = COALESCE(EXCLUDED.password_hash, users.password_hash),
        role = EXCLUDED.role;"

echo "Done! $ROLE user $EMAIL created."
