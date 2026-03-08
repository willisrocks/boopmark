# Boopmark Rust Rewrite — Design Spec

## Overview

Full rewrite of Boopmark (bookmark management app) from Node/React to Rust/Axum/HTMX with a CLI client. Uses ports-and-adapters (hexagonal) architecture.

## Goals

- Rust/Axum web server serving HTMX-driven UI + REST API
- CLI tool (`boop`) that talks to the server via REST API
- Ports-and-adapters architecture for testability and swappable backends
- Docker Compose for local dev, Fly.io + Neon Free for prod
- Cloudflare R2 for image storage (local filesystem/MinIO for dev)

## Domain Model

### Entities

**User** — id (UUID), email, name, image, created_at

**Bookmark** — id (UUID), user_id (FK), url, title?, description?, image_url?, domain?, tags (Vec<String>), created_at, updated_at

**Session** — id (UUID), user_id (FK), token, expires_at

**ApiKey** — id (UUID), user_id (FK), key_hash (argon2), name, created_at

### Ports (Traits)

- `BookmarkRepository` — CRUD, search (full-text), filter by tags, sort
- `UserRepository` — find by email, create, find by id
- `SessionRepository` — create, find by token, delete
- `ApiKeyRepository` — create, find by key hash
- `MetadataExtractor` — extract title/description/image/domain from URL
- `ObjectStorage` — put, get, delete objects

## Database Schema

PostgreSQL (Neon Free in prod, Docker Postgres locally).

```sql
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT UNIQUE NOT NULL,
    name TEXT,
    image TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    token TEXT UNIQUE NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    key_hash TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE bookmarks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    title TEXT,
    description TEXT,
    image_url TEXT,
    domain TEXT,
    tags TEXT[] DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT now(),
    updated_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_bookmarks_user_id ON bookmarks(user_id);
CREATE INDEX idx_bookmarks_tags ON bookmarks USING GIN(tags);
CREATE INDEX idx_bookmarks_search ON bookmarks USING GIN(
    to_tsvector('english', coalesce(title,'') || ' ' || coalesce(description,'') || ' ' || url)
);
```

## Authentication

### Web — Google OAuth

1. `GET /auth/google` → redirect to Google OAuth consent
2. `GET /auth/google/callback` → exchange code for tokens, fetch user info, upsert user, create session, set `session_token` cookie
3. Protected routes check session cookie via middleware
4. `POST /auth/logout` → delete session, clear cookie

### CLI — API Keys

1. User generates API key in web UI at `/settings/api-keys`
2. Server generates random key, stores argon2 hash, returns plain key once
3. CLI sends `Authorization: Bearer <key>` on every request
4. Middleware hashes received key and looks up `api_keys` table

## Project Structure

```
boopmark/
├── src/
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── bookmark.rs       # Bookmark, CreateBookmark, BookmarkFilter, BookmarkSort
│   │   ├── user.rs           # User, CreateUser
│   │   └── ports/
│   │       ├── mod.rs
│   │       ├── bookmark_repo.rs
│   │       ├── user_repo.rs
│   │       ├── session_repo.rs
│   │       ├── api_key_repo.rs
│   │       ├── metadata.rs
│   │       └── storage.rs
│   │
│   ├── adapters/
│   │   ├── mod.rs
│   │   ├── postgres/
│   │   │   ├── mod.rs
│   │   │   ├── bookmark_repo.rs
│   │   │   ├── user_repo.rs
│   │   │   ├── session_repo.rs
│   │   │   └── api_key_repo.rs
│   │   ├── storage/
│   │   │   ├── mod.rs
│   │   │   ├── s3.rs
│   │   │   └── local.rs
│   │   └── scraper.rs
│   │
│   ├── app/
│   │   ├── mod.rs
│   │   ├── bookmarks.rs      # BookmarkService
│   │   └── auth.rs           # AuthService
│   │
│   ├── web/
│   │   ├── mod.rs
│   │   ├── router.rs
│   │   ├── state.rs          # AppState with Arc'd services
│   │   ├── api/
│   │   │   ├── mod.rs
│   │   │   ├── bookmarks.rs
│   │   │   └── auth.rs
│   │   ├── pages/
│   │   │   ├── mod.rs
│   │   │   ├── home.rs
│   │   │   ├── auth.rs
│   │   │   └── bookmarks.rs
│   │   ├── middleware/
│   │   │   ├── mod.rs
│   │   │   └── auth.rs
│   │   └── extractors.rs
│   │
│   ├── config.rs
│   └── main.rs
│
├── templates/
│   ├── base.html
│   ├── home.html
│   ├── bookmarks/
│   │   ├── grid.html
│   │   ├── card.html
│   │   ├── list.html
│   │   └── add_modal.html
│   ├── auth/
│   │   ├── login.html
│   │   └── callback.html
│   └── components/
│       ├── header.html
│       ├── filters.html
│       └── tag.html
│
├── migrations/
│   ├── 001_create_users.sql
│   ├── 002_create_sessions.sql
│   ├── 003_create_api_keys.sql
│   └── 004_create_bookmarks.sql
│
├── cli/
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
│
├── static/
│   └── css/
│       └── output.css        # Tailwind compiled
│
├── Cargo.toml                # Workspace root
├── Dockerfile
├── docker-compose.yml
├── fly.toml
├── justfile
├── tailwind.config.js
└── .env.example
```

## Web Layer

### HTMX Pages (server-rendered HTML)

All pages return full HTML on normal requests, partials on HTMX requests (`HX-Request` header).

- `GET /` → redirect to `/bookmarks` if authenticated, `/auth/login` if not
- `GET /bookmarks` → bookmark grid (full page or partial)
  - Query params: `search`, `tags` (comma-sep), `sort` (newest|oldest|title|domain)
- `GET /bookmarks/add` → add bookmark modal partial
- `POST /bookmarks` → create bookmark, return new card partial
- `DELETE /bookmarks/:id` → delete, return empty (HTMX removes element)
- `GET /auth/login` → login page
- `GET /auth/google` → redirect to Google
- `GET /auth/google/callback` → handle OAuth callback
- `POST /auth/logout` → clear session, redirect
- `GET /settings/api-keys` → API key management page
- `POST /settings/api-keys` → generate new key

### HTMX Interactions

- **Search:** `hx-get="/bookmarks" hx-trigger="keyup changed delay:300ms" hx-target="#bookmark-grid"`
- **Tag filter:** `hx-get="/bookmarks?tags=..." hx-target="#bookmark-grid"`
- **Sort:** `hx-get="/bookmarks?sort=..." hx-target="#bookmark-grid"`
- **Add bookmark:** `hx-post="/bookmarks" hx-target="#bookmark-grid" hx-swap="afterbegin"`
- **Delete:** `hx-delete="/bookmarks/{id}" hx-swap="outerHTML swap:200ms" hx-confirm="Delete this bookmark?"`

### REST API (for CLI)

All under `/api/v1/`, JSON request/response, API key auth via `Authorization: Bearer <key>`.

- `GET /api/v1/bookmarks` — list with query params (search, tags, sort, limit, offset)
- `POST /api/v1/bookmarks` — create `{ url, title?, description?, tags? }`
- `GET /api/v1/bookmarks/:id` — get single
- `PUT /api/v1/bookmarks/:id` — update
- `DELETE /api/v1/bookmarks/:id` — delete
- `POST /api/v1/bookmarks/metadata` — extract metadata `{ url }` → `{ title, description, image_url, domain }`

## CLI (`boop`)

Separate crate in Cargo workspace. Uses `clap` for args, `reqwest` for HTTP, `tabled` for output formatting.

```
boop add <url> [--title "..."] [--tags "rust,web"]
boop list [--search "..."] [--tags "..."] [--sort newest|oldest|title]
boop search <query>
boop get <id>
boop delete <id>
boop config set-server <url>
boop config set-key <api-key>
boop config show
```

Config at `~/.config/boop/config.toml`:
```toml
server_url = "https://boopmark.fly.dev"
api_key = "boop_xxxxxxxxxxxx"
```

## Infrastructure

### Docker Compose (Local Dev)

```yaml
services:
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: boopmark
      POSTGRES_USER: boopmark
      POSTGRES_PASSWORD: devpassword
    ports: ["5432:5432"]
    volumes: ["pgdata:/var/lib/postgresql/data"]

  minio:
    image: minio/minio
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin
    ports: ["9000:9000", "9001:9001"]

  app:
    build: .
    depends_on: [db, minio]
    ports: ["4000:4000"]
    environment:
      DATABASE_URL: postgres://boopmark:devpassword@db/boopmark
      STORAGE_BACKEND: s3
      S3_ENDPOINT: http://minio:9000
      S3_BUCKET: boopmark
      S3_ACCESS_KEY: minioadmin
      S3_SECRET_KEY: minioadmin
      S3_REGION: us-east-1
      GOOGLE_CLIENT_ID: ${GOOGLE_CLIENT_ID}
      GOOGLE_CLIENT_SECRET: ${GOOGLE_CLIENT_SECRET}
      SESSION_SECRET: dev-secret-change-me
      APP_URL: http://localhost:4000

volumes:
  pgdata:
```

### Fly.io (Production)

`fly.toml`:
- App name: `boopmark`
- Machine: `shared-cpu-1x`, 256MB RAM
- Internal port: 4000
- Health check: `GET /health`
- Secrets: DATABASE_URL (Neon pooled), S3_* (R2 creds), GOOGLE_CLIENT_ID/SECRET, SESSION_SECRET

### Dockerfile

Multi-stage build:
1. `rust:1.85-slim` builder stage — `cargo build --release`
2. `debian:bookworm-slim` runtime — copy binary + migrations + templates + static assets
3. Entry: run migrations then start server

## Dependencies

```toml
[workspace.dependencies]
axum = "0.8"
axum-extra = { version = "0.10", features = ["cookie"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "chrono"] }
askama = "0.12"
askama_axum = "0.4"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
scraper = "0.22"
aws-sdk-s3 = "1"
aws-config = "1"
tower = "0.5"
tower-http = { version = "0.6", features = ["fs", "cors", "trace"] }
argon2 = "0.5"
rand = "0.9"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
dotenvy = "0.15"
thiserror = "2"
url = "2"
clap = { version = "4", features = ["derive"] }
tabled = "0.17"
toml = "0.8"
dirs = "6"
```

## UI Design

Dark theme matching current Boopmark design:
- **Background:** Dark navy (#0f1117 / #1a1d2e)
- **Cards:** Slightly lighter dark (#1e2235) with subtle border, rounded corners
- **Header:** Logo left, search bar center, "Add Bookmark" button right, avatar far right
- **Tag chips:** Colored pills (varied colors per tag)
- **Card layout:** Image preview top, title, description snippet, tags, date
- **Responsive:** Grid adapts from 1 to 4 columns
- **Tailwind CSS 4** via standalone CLI (no Node.js dependency)

## Testing Strategy

- **Domain + app layer:** Unit tests with mock trait implementations
- **Adapters:** Integration tests against Docker Postgres (via sqlx test fixtures)
- **Web layer:** Integration tests using axum::test helpers
- **CLI:** Unit tests for argument parsing, integration tests against running server

## Error Handling

- Domain errors as enums implementing `thiserror::Error`
- App layer maps adapter errors to domain errors
- Web layer maps domain errors to HTTP responses (JSON for API, HTML for pages)
- Consistent error pages for HTMX responses
