# Local development commands

setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "==> Copying .env.example to .env (if needed)"
    [ -f .env ] || cp .env.example .env
    echo "==> Starting Docker services"
    docker compose up -d db minio
    echo "==> Waiting for Postgres..."
    until docker compose exec db pg_isready -U boopmark > /dev/null 2>&1; do sleep 1; done
    echo "==> Installing sqlx-cli (if needed)"
    command -v sqlx > /dev/null 2>&1 || cargo install sqlx-cli --no-default-features --features postgres,rustls
    echo "==> Running migrations"
    source .env
    sqlx migrate run --source migrations
    echo "==> Installing npm dependencies"
    npm install
    echo "==> Building Tailwind CSS"
    npx tailwindcss -i static/css/input.css -o static/css/output.css --minify
    echo "==> Building project"
    cargo build
    echo "==> Setup complete! Run 'just dev' to start the server."

dev:
    docker compose up -d db minio
    cargo run -p boopmark-server

build:
    cargo build --release

test:
    cargo test

css:
    npx tailwindcss -i static/css/input.css -o static/css/output.css --watch

css-build:
    npx tailwindcss -i static/css/input.css -o static/css/output.css --minify

typecheck:
    cargo check

docker-up:
    docker compose up -d

docker-down:
    docker compose down

migrate:
    sqlx migrate run --source migrations

add-user *ARGS:
    ./scripts/add-user.sh {{ARGS}}

deploy:
    just css-build
    railway up

# Run install script tests
test-install:
    sh tests/test_install.sh

# Format check (CI)
fmt-check:
    cargo fmt -- --check

# Run clippy
check:
    cargo clippy -p boop --all-targets -- -D warnings
    cargo test -p boop

# Create a release (--major, --minor, or --patch)
release bump="patch":
    #!/usr/bin/env bash
    set -euo pipefail
    latest=$(git tag --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)
    if [ -z "$latest" ]; then
      echo "Error: no existing vX.Y.Z tag found" >&2; exit 1
    fi
    IFS='.' read -r major minor patch <<< "${latest#v}"
    case "{{bump}}" in
      major) major=$((major + 1)); minor=0; patch=0 ;;
      minor) minor=$((minor + 1)); patch=0 ;;
      patch) patch=$((patch + 1)) ;;
      *) echo "Error: bump must be major, minor, or patch" >&2; exit 1 ;;
    esac
    next="${major}.${minor}.${patch}"
    echo "Releasing v${next} (current: ${latest})"
    gh workflow run release.yml -f version="${next}"
    echo "Release workflow triggered. Watch: gh run list --workflow=release.yml"
