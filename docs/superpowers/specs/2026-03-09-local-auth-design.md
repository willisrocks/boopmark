# Local User Login for Development

## Problem

Google OAuth breaks behind devproxy because devproxy generates random slugs and Google doesn't support wildcard redirect URIs. Need a local login option for development.

## Design

### Database

New migration adds `password_hash TEXT` nullable column to `users` table.

### Config

`ENABLE_LOCAL_AUTH` env var, parsed identically to `ENABLE_E2E_AUTH` (accepts `1`, `true`, `TRUE`). Off by default.

### Domain

Add `password_hash: Option<String>` to `User` struct.

### User Repository

Update queries to include `password_hash` column. Add method to upsert user with password hash.

### Auth Service

New `local_login(email, password) -> Result<Session>` method:
1. Find user by email
2. Verify password against stored argon2 hash
3. Create session (same as Google OAuth flow)

### Web Handler

`POST /auth/local-login` — accepts form with email/password, calls auth service, sets session cookie on success, redirects to login with error on failure.

### Login Template

When `enable_local_auth` is true, show email/password form alongside Google button. Both visible if both configured.

### `just add-user`

Shell script (`scripts/add-user.sh`):
- Accepts email and password as args or prompts interactively
- Hashes password via small Rust helper (cargo run --example)
- Inserts/updates user via psql
- Justfile recipe wraps the script

### Docker Compose

Add `ENABLE_LOCAL_AUTH=1` to server service environment.

### Documentation

Update CLAUDE.md and README.md with local auth instructions.

## Unchanged

- Google OAuth flow
- E2E auth mechanism
- API key auth
- Session management

## Testing

Use agent-browser (Playwright MCP) for E2E verification against dev server.
