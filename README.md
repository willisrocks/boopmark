# Boopmark

A full-stack bookmark management app built with Rust (Axum), HTMX, and Tailwind CSS.

## Getting Started

1. Copy `.env.example` to `.env` and fill in values
2. Run `docker compose up -d` to start all services (Postgres, MinIO, and the app server)
3. Open `http://localhost:4000`

### Hybrid development (faster iteration)

For faster rebuilds without Docker, run only the infrastructure services and start the server directly:

1. Run `docker compose up -d db minio` to start only Postgres and MinIO
2. Run `cargo run -p boopmark-server` to start the dev server
3. Open `http://localhost:4000`

> **Note:** Do not run both workflows at the same time — they both bind port 4000.

### Local Auth (Development)

When Google OAuth isn't available (e.g. behind a reverse proxy), you can use local username/password login:

1. Set `ENABLE_LOCAL_AUTH=1` in your `.env` file (already enabled in `docker-compose.yml`)
2. Create a user: `just add-user you@example.com yourpassword`
3. Sign in with email and password on the login page

### Local HTTPS Dev URLs

This project supports local HTTPS subdomains via [devproxy](https://github.com/foundra-build/devproxy):

1. Install devproxy: `curl -fsSL https://raw.githubusercontent.com/foundra-build/devproxy/main/install.sh | sh`
2. Run `devproxy init` (one-time setup — see devproxy docs for DNS configuration)
3. Start the project: `devproxy up`
4. Open the printed HTTPS URL

Run `devproxy ls` to see all running projects (`*` marks the current directory).

### CLI (`boop`)

Manage your bookmarks from the terminal:

1. Install: `curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | sh`
2. Configure:
   ```bash
   boop config set-server https://your-boopmark-instance.example.com
   boop config set-key YOUR_API_KEY
   ```
3. Use:
   ```bash
   boop add https://example.com --title "Example" --tags "ref"
   boop list
   boop search "query"
   ```

See `boop --help` for all commands.
