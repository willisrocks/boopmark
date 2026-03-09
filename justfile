# Local development commands

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

deploy:
    just css-build
    fly deploy
