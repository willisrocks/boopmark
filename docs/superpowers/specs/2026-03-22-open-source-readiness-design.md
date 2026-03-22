# Open Source Readiness Design

**Date:** 2026-03-22
**Goal:** Prepare Boopmark for open-sourcing with clean abstractions, great self-hosting experience, and comprehensive documentation.

## Priorities

1. Self-hosters (Docker Compose on a VPS)
2. Contributors (developers who want to hack on it)
3. One-click cloud deploy (Railway + Neon as secondary)

## 1. Screenshot Port Abstraction

### Problem

`ScreenshotClient` is a hardcoded HTTP client with no port trait — the only service in the codebase not following the hexagonal pattern.

### Solution

New port trait in `domain/ports/screenshot.rs`:

```rust
#[trait_variant::make(Send)]
pub trait ScreenshotProvider: Send + Sync {
    async fn capture(&self, url: &str) -> Result<Vec<u8>, DomainError>;
}
```

Three adapters:

| Adapter | Backend | Config |
|---------|---------|--------|
| `PlaywrightScreenshot` | Current sidecar HTTP call | `SCREENSHOT_SERVICE_URL` |
| `CloudflareScreenshot` | Cloudflare Browser Rendering API | `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` |
| `NoopScreenshot` | Returns error / no-op | (none) |

Config:
- New env var `SCREENSHOT_BACKEND`: `playwright` | `cloudflare` | `disabled` (default: `disabled`)
- `BookmarkService` receives `Arc<dyn ScreenshotProvider>` instead of `Option<String>`
- Existing `screenshot.rs` adapter renamed/refactored into `adapters/screenshot/playwright.rs`

## 2. Owner Bootstrap & User Management

### Problem

`just add-user` always creates a regular user. No way to bootstrap the first owner without raw SQL.

### Solution

Extend `scripts/add-user.sh` to accept optional flags:

```bash
just add-user email@example.com --role owner                    # Google OAuth (no password)
just add-user email@example.com --role owner --password secret  # Local auth
```

- `--role` accepts `owner`, `admin`, `user` (default: `user`)
- `--password` is optional; required when `LOGIN_ADAPTER=local_password`, error if local auth but no password
- `--role owner` fails if an owner already exists in the database

## 3. Bootstrap Command

### Problem

New self-hosters need to: copy env, generate secrets, start services, wait for DB, run migrations, create owner. Too many manual steps.

### Solution

New `just bootstrap` command that:

1. Copies `.env.example` to `.env`
2. Auto-generates `SESSION_SECRET` and `LLM_SETTINGS_ENCRYPTION_KEY` with cryptographically random values
3. Prompts for owner email/password (or takes args)
4. Runs `docker compose up -d`
5. Waits for Postgres readiness
6. Runs migrations
7. Creates the owner user via `just add-user ... --role owner`
8. Prints "Ready at http://localhost:4000"

New `just dev` command checks `USE_DEVPROXY` env var:
- `USE_DEVPROXY=1`: runs `devproxy up`
- `USE_DEVPROXY=0` (default): runs `docker compose up -d`

Existing `just setup` retired in favor of `just bootstrap`.

## 4. Infrastructure Changes

### Replace MinIO with RustFS

MinIO's licensing (AGPL) is unfriendly for self-hosters. Replace with RustFS:
- Apache 2.0 licensed
- S3-compatible drop-in replacement
- Built in Rust, lighter weight
- Docker image: `rustfs/rustfs`
- Our `S3Storage` adapter works unchanged (generic S3 protocol)

### Dockerfile: Add Tailwind CSS Build Stage

Current Dockerfile copies pre-built CSS from host. Add a Node build stage so self-hosters don't need Node locally:

```dockerfile
FROM node:24-slim AS css
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY static/css/input.css static/css/input.css
COPY templates/ templates/
RUN npx tailwindcss -i static/css/input.css -o static/css/output.css --minify
```

Final stage copies `output.css` from the css builder.

Local dev continues using `just css` (Tailwind watch mode) for fast iteration.

### USE_DEVPROXY Flag

- New env var `USE_DEVPROXY=0` (default: false)
- `just dev` reads this to decide between `devproxy up` and `docker compose up -d`
- Self-hosters never see devproxy; contributors can opt in

## 5. Docker Compose for Self-Hosters

Default services (uncommented):
- `db` — Postgres
- `server` — Boopmark

Optional services (commented out with instructions):
- `rustfs` — RustFS for S3-compatible storage
- `screenshot-svc` — Playwright screenshot sidecar

Sensible defaults so minimal `.env` works out of the box.

## 6. Documentation

### README.md — Full Rewrite

- Project description + feature highlights
- Quick start (Docker Compose, 3 steps including `just bootstrap`)
- Configuration reference (table of all env vars with defaults and descriptions)
- Deployment guides: Docker Compose (primary), Railway + Neon (secondary)
- CLI section (install + usage)
- Architecture overview (hex architecture, link to deeper docs)
- License badge (MIT)

### CONTRIBUTING.md — New

- Prerequisites (Rust, Node, Docker)
- Development setup (`just bootstrap`, `just dev`)
- Project structure overview
- How to add a new adapter (document the existing pattern)
- Testing (`cargo test`, Playwright E2E)
- PR process

### LICENSE — MIT

Standard MIT license file in repository root.

### .env.example — Rewrite

- Grouped: Required vs Optional
- Comments explaining each variable
- Defaults that work for Docker Compose out of the box

## 7. License

MIT. Chosen for:
- Maximum adoption and contributor friendliness
- Rust ecosystem standard
- Compatible with future open-core model if desired
- Owner retains right to relicense future versions

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| License | MIT | Simplest, most contributor-friendly, open-core compatible |
| Screenshot default | Disabled | Minimal dependency footprint for self-hosters |
| S3 provider | RustFS (replaces MinIO) | Apache 2.0 license, lighter, S3-compatible |
| Auth default | Local password | No Google OAuth setup needed for self-hosting |
| Storage default | Local filesystem | No S3/RustFS needed for basic install |
| Registration | Invite-only | Owner invites users via admin panel |
| Tailwind in Docker | Node 24 build stage | Self-hosters don't need Node locally |
