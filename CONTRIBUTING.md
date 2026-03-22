# Contributing to Boopmark

Thank you for your interest in contributing! This guide covers everything you need to get started.

## Getting Started

**Prerequisites:**
- Rust (stable toolchain)
- Node.js 24+
- Docker
- [just](https://github.com/casey/just) — task runner

**Setup:**

```bash
# Fork and clone the repo
git clone https://github.com/your-fork/boopmark
cd boopmark

# One-time setup: starts db, runs migrations, installs npm deps, builds CSS
just setup

# Start the server
cargo run -p boopmark-server

# Watch Tailwind CSS changes
just css
```

The server is available at `http://localhost:4000`. Create a local user with:

```bash
just add-user you@example.com --password yourpassword
```

## Project Structure

```
boopmark/
├── server/          # Axum web server (boopmark-server crate)
│   └── src/
│       ├── domain/  # Business logic and port traits
│       ├── adapters/# Port implementations (Postgres, S3, Anthropic, ...)
│       ├── app/     # Application services (BookmarkService, AuthService, ...)
│       └── web/     # Axum handlers, templates, routing
├── cli/             # CLI client (boop crate)
├── screenshot-svc/  # Optional Playwright screenshot sidecar (Node.js)
├── templates/       # Askama HTML templates
├── static/          # CSS, JS, images
├── migrations/      # SQLx Postgres migrations
└── tests/           # E2E tests (Playwright)
```

## Architecture

Boopmark uses hexagonal (ports-and-adapters) architecture. The domain layer has no knowledge of infrastructure — only pure Rust traits.

### Ports (`server/src/domain/ports/`)

Traits define the contract for external dependencies:

- `BookmarkRepository` — CRUD and search for bookmarks
- `MetadataExtractor` — extract title/description/og:image from URLs
- `ObjectStorage` — store and retrieve binary blobs (images)
- `LlmEnricher` — AI-powered tag and description enrichment
- `LoginProvider` — authentication (Google OAuth, local password)
- `ScreenshotProvider` — capture screenshots of web pages

### Adapters (`server/src/adapters/`)

Concrete implementations of ports:

- `adapters/postgres/` — SQLx Postgres implementations
- `adapters/scraper/` — HTML metadata extraction via scraper
- `adapters/storage/local/` and `storage/s3/` — local disk and S3 storage
- `adapters/anthropic/` — Anthropic Claude LLM enricher
- `adapters/login/google/` and `login/local_password/` — auth adapters
- `adapters/screenshot/playwright/` and `screenshot/noop/` — screenshot adapters

### Adding a New Adapter

1. Define or reuse a port trait in `server/src/domain/ports/`.
2. Create the adapter file in `server/src/adapters/<name>/`.
3. Implement the port trait for your struct.
4. Add a config enum variant in `server/src/config.rs`.
5. Wire the new adapter in `server/src/main.rs`.
6. Write unit tests in the adapter file (see existing adapters for the pattern).

## Testing

```bash
# Run all unit and integration tests
cargo test

# Run tests for a specific crate
cargo test -p boopmark-server
cargo test -p boop

# Run E2E tests (requires Docker — spins up its own server on port 4010)
npx playwright test tests/e2e/suggest.spec.js

# Lint
cargo clippy -- -D warnings

# Format check
cargo fmt -- --check
```

The E2E test harness manages its own server instance — do not point it at a running dev server.

## Pull Requests

- Keep PRs focused: one feature or fix per PR.
- Include tests for new behavior. New adapters should have unit tests.
- Run `cargo test` and `cargo clippy -- -D warnings` before submitting.
- Follow the existing code style (match indentation, naming conventions, etc.).
- Write a clear PR description explaining what and why.

For large changes, open an issue first to discuss the approach.
