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

# Find the db container
DB_CONTAINER=$(docker ps --filter "name=boopmark-db-1" --format "{{.Names}}" | head -1)
if [ -z "$DB_CONTAINER" ]; then
  # Fallback: try docker compose service name
  DB_CONTAINER=$(docker ps --filter "name=db-1" --format "{{.Names}}" | head -1)
fi
if [ -z "$DB_CONTAINER" ]; then
  echo "Error: no running boopmark db container found. Start services first." >&2
  exit 1
fi

# Owner guard
if [ "$ROLE" = "owner" ]; then
  EXISTING_OWNER=$(docker exec "$DB_CONTAINER" psql -U boopmark -d boopmark -t -c \
    "SELECT COUNT(*) FROM users WHERE role = 'owner' AND deactivated_at IS NULL;" | tr -d ' ')
  if [ "$EXISTING_OWNER" -gt 0 ]; then
    echo "Error: an owner already exists. Use the admin panel to manage users." >&2
    exit 1
  fi
fi

# Hash password if provided
HASH_CLAUSE="NULL"
if [ -n "$PASSWORD" ]; then
  echo "Hashing password..."
  if command -v cargo > /dev/null 2>&1; then
    HASH=$(cargo run -p boopmark-server --example hash_password -- "$PASSWORD" 2>/dev/null)
  else
    # Docker-only: exec into server container to hash
    SERVER_CONTAINER=$(docker ps --filter "name=server" --format "{{.Names}}" | head -1)
    if [ -z "$SERVER_CONTAINER" ]; then
      echo "Error: no running server container found and cargo not available." >&2
      exit 1
    fi
    HASH=$(docker exec "$SERVER_CONTAINER" ./hash_password "$PASSWORD")
  fi
  HASH_CLAUSE="'$HASH'"
fi

echo "Creating $ROLE user $EMAIL..."
docker exec "$DB_CONTAINER" psql -U boopmark -d boopmark \
  -c "INSERT INTO users (email, name, password_hash, role)
      VALUES ('$EMAIL', '$EMAIL', $HASH_CLAUSE, '$ROLE')
      ON CONFLICT (email) DO UPDATE SET
        password_hash = COALESCE(EXCLUDED.password_hash, users.password_hash),
        role = EXCLUDED.role;"

echo "Done! $ROLE user $EMAIL created."
