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
    echo "==> Building Tailwind CSS"
    ./tailwindcss-macos-arm64 -i static/css/input.css -o static/css/output.css --minify
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
    ./tailwindcss-macos-arm64 -i static/css/input.css -o static/css/output.css --watch

css-build:
    ./tailwindcss-macos-arm64 -i static/css/input.css -o static/css/output.css --minify

typecheck:
    cargo check

docker-up:
    docker compose up -d

docker-down:
    docker compose down

migrate:
    sqlx migrate run --source migrations

deploy:
    just css-build
    fly deploy
