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

Two adapters:

| Adapter | Backend | Config |
|---------|---------|--------|
| `PlaywrightScreenshot` | Current sidecar HTTP call | `SCREENSHOT_SERVICE_URL` |
| `NoopScreenshot` | Returns error / no-op | (none) |

Cloudflare Browser Rendering adapter deferred — it's a paid service and most self-hosters won't use it. Can be added later following the same port pattern.

Config:
- New env var `SCREENSHOT_BACKEND`: `playwright` | `disabled` (default: `disabled`)
- `SCREENSHOT_SERVICE_URL` remains as sub-config when `SCREENSHOT_BACKEND=playwright`
- `BookmarkService` receives `Arc<dyn ScreenshotProvider>` instead of `Option<String>`
- Note: `BookmarkService` is generic (`BookmarkService<R, M, S>`) — the screenshot provider will be added as a fourth generic parameter or use `Arc<dyn ScreenshotProvider>` as a field. The `Bookmarks` enum in `state.rs` and construction in `main.rs` will need corresponding updates.
- Existing `screenshot.rs` adapter renamed/refactored into `adapters/screenshot/playwright.rs`

## 2. Owner Bootstrap & User Management

### Problem

`just add-user` always creates a regular user. No way to bootstrap the first owner without raw SQL.

### Solution

Rewrite `scripts/add-user.sh` with proper flag parsing:

```bash
just add-user email@example.com --role owner                    # Google OAuth (no password)
just add-user email@example.com --role owner --password secret  # Local auth
```

This is a non-trivial rewrite of the current script, which only takes positional args and has no role support. The new script needs:
- Argument parser (getopt or manual flag loop)
- `--role` flag: accepts `owner`, `admin`, `user` (default: `user`)
- `--password` flag: optional; required when `LOGIN_ADAPTER=local_password`, error if local auth but no password provided
- Owner guard: `--role owner` fails if an owner already exists in the database
- The INSERT statement must include the `role` column

## 3. Bootstrap Command

### Problem

New self-hosters need to: copy env, generate secrets, start services, wait for DB, run migrations, create owner. Too many manual steps.

### Solution

New `just bootstrap` command for self-hosters (Docker-only workflow):

1. Copies `.env.example` to `.env`
2. Auto-generates `SESSION_SECRET` and `LLM_SETTINGS_ENCRYPTION_KEY` with cryptographically random values
3. Prompts for owner email/password (or takes args)
4. Runs `docker compose up -d`
5. Waits for Postgres readiness
6. Runs migrations
7. Creates the owner user via `just add-user ... --role owner`
8. Prints "Ready at http://localhost:4000"

Modify existing `just dev` to check `USE_DEVPROXY` env var:
- `USE_DEVPROXY=1`: runs `devproxy up`
- `USE_DEVPROXY=0` (default): runs `docker compose up -d`

Note: `just dev` currently runs `docker compose up -d db minio` + `cargo run`. This changes to a Docker-first workflow by default, which is a behavior change for existing contributors.

Keep existing `just setup` for contributors who want the hybrid workflow (infra in Docker, server running locally via cargo). Update it to reflect new service names (RustFS instead of MinIO) and ensure it still works alongside `just bootstrap`.

## 4. Infrastructure Changes

### Replace MinIO with RustFS

MinIO's licensing (AGPL) is unfriendly for self-hosters. Replace with RustFS:
- Apache 2.0 licensed
- S3-compatible drop-in replacement
- Built in Rust, lighter weight
- Docker image: `rustfs/rustfs`
- Our `S3Storage` adapter works unchanged (generic S3 protocol)
- Note: RustFS is still alpha (v1.0.0-alpha). Acceptable for our use case (single-node, bookmark images) but worth noting.

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

Changes from current docker-compose.yml:
- Remove `minio` service, replace with commented-out `rustfs` block
- Remove `minio` from `server.depends_on`
- Remove S3 env vars from `server.environment` when `STORAGE_BACKEND=local` (the default)
- Ensure `server` starts cleanly with only `db` as a dependency

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
- Clean up stale references to `ENABLE_LOCAL_AUTH` (replaced by `LOGIN_ADAPTER` in prior work)

### CONTRIBUTING.md — New

- Prerequisites (Rust, Node, Docker)
- Development setup (`just setup` for hybrid, `just bootstrap` for Docker-only)
- Project structure overview
- How to add a new adapter (document the existing pattern)
- Testing (`cargo test`, Playwright E2E)
- PR process

### LICENSE — MIT

Standard MIT license file in repository root.
Add `license = "MIT"` to workspace `Cargo.toml` and per-crate `Cargo.toml` files.

### .env.example — Rewrite

- Grouped: Required vs Optional
- Comments explaining each variable
- Defaults that work for Docker Compose out of the box
- New vars: `SCREENSHOT_BACKEND`, `USE_DEVPROXY`
- Remove/update stale references

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
| Screenshot adapters | Playwright + Noop (Cloudflare deferred) | Ship what's needed now, extend later |
| S3 provider | RustFS (replaces MinIO) | Apache 2.0 license, lighter, S3-compatible |
| Auth default (proposed) | Change to local password | No Google OAuth setup needed for self-hosting. Note: currently defaults to `google` in `config.rs` — this is a proposed change. |
| Storage default | Local filesystem | Already the default. No S3/RustFS needed for basic install. |
| Registration | Invite-only | Owner invites users via admin panel |
| Tailwind in Docker | Node 24 build stage | Self-hosters don't need Node locally |
| Contributor workflow | Keep `just setup` alongside `just bootstrap` | Hybrid dev (cargo run locally) still supported |
