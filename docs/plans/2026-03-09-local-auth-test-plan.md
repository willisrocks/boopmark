# Local Auth Test Plan

## Sources of Truth

- **Implementation plan:** `docs/superpowers/plans/2026-03-09-local-auth.md`
- **Existing `ENABLE_E2E_AUTH` pattern:** `server/src/config.rs`, `server/src/web/pages/auth.rs`, `templates/auth/login.html` — the local auth feature mirrors this pattern exactly
- **Argon2 crate docs:** password hashing and verification behavior
- **Axum/HTTP semantics:** POST form handling, cookie-based sessions, redirects

## Strategy Reconciliation

The agreed strategy is:
- Use Playwright MCP (agent-browser) against `http://localhost:4000` for E2E verification (Task 11)
- Do NOT write committed Playwright specs
- Use `cargo test` for unit/integration tests within the Rust codebase
- Use `cargo check` / `cargo build` as compilation gates

The implementation plan aligns with this. The plan follows the existing `ENABLE_E2E_AUTH` pattern closely, so the testing surface is well-bounded. No paid APIs or external services beyond the local Postgres database are needed.

Adjustments: None required. The strategy holds as stated.

## Harness Requirements

### Harness 1: Local Postgres + Server (for E2E scenarios)

- **What it does:** Runs the full server stack with `ENABLE_LOCAL_AUTH=1` against a local Postgres instance
- **What it exposes:** HTTP endpoints at `http://localhost:4000`, the login page UI, cookie-based session management
- **Estimated complexity:** Zero — uses existing `docker compose up -d db minio` + `cargo run -p boopmark-server` with env vars
- **Which tests depend on it:** Tests 1-4 (all scenario and integration tests)
- **Setup:** `docker compose up -d db minio`, then `ENABLE_LOCAL_AUTH=1 cargo run -p boopmark-server`, then `just add-user test@local.dev testpass123`

### Harness 2: `cargo test` (for unit tests)

- **What it does:** Runs the Rust test suite
- **What it exposes:** Unit test assertions against pure functions and compilation checks
- **Estimated complexity:** Zero — uses existing `cargo test`
- **Which tests depend on it:** Tests 5-9

---

## Test Plan

### Test 1: Full local auth login flow — user signs in with email/password and reaches home page

- **Type:** scenario
- **Harness:** Local Postgres + Server (Playwright MCP)
- **Preconditions:**
  - Server running with `ENABLE_LOCAL_AUTH=1`
  - Test user created via `just add-user test@local.dev testpass123`
- **Actions:**
  1. Navigate to `http://localhost:4000/auth/login`
  2. Verify the local auth form is visible: email input, password input, "Sign in" button
  3. Fill email field with `test@local.dev`
  4. Fill password field with `testpass123`
  5. Click "Sign in" button
  6. Wait for navigation
- **Expected outcome:**
  - Browser redirects to `/` (the home page)
  - A `session` cookie is set
  - The page shows authenticated content (not the login page)
  - Source: implementation plan Task 6 Step 4 — successful login returns `Ok((jar.add(cookie), Redirect::to("/")))`
- **Interactions:** Session creation (SessionRepository), user lookup (UserRepository), argon2 password verification

### Test 2: Invalid credentials show error message on login page

- **Type:** scenario
- **Harness:** Local Postgres + Server (Playwright MCP)
- **Preconditions:**
  - Server running with `ENABLE_LOCAL_AUTH=1`
- **Actions:**
  1. Navigate to `http://localhost:4000/auth/login`
  2. Fill email with `wrong@example.com`
  3. Fill password with `badpassword`
  4. Click "Sign in"
  5. Wait for navigation
- **Expected outcome:**
  - Browser is on `/auth/login` (with `?error=...` query param)
  - An error message is visible on the page containing "Invalid email or password"
  - No `session` cookie is set
  - Source: implementation plan Task 6 Step 4 — failed login redirects to `/auth/login?error=Invalid+email+or+password`
- **Interactions:** User lookup returns None, DomainError::Unauthorized mapped to redirect

### Test 3: Google OAuth button remains visible alongside local auth form

- **Type:** integration
- **Harness:** Local Postgres + Server (Playwright MCP)
- **Preconditions:**
  - Server running with `ENABLE_LOCAL_AUTH=1`
- **Actions:**
  1. Navigate to `http://localhost:4000/auth/login`
  2. Inspect the page for both the local auth form and the Google sign-in button
- **Expected outcome:**
  - The local auth form is present (email input, password input, "Sign in" button)
  - A divider with "or" text is present between the local form and Google button
  - The Google "Sign in with Google" link/button is present and points to `/auth/google`
  - Source: implementation plan Task 7 — template shows both forms with "or" divider when `enable_local_auth` is true
- **Interactions:** Template rendering with both `enable_local_auth` and Google OAuth coexisting

### Test 4: Local auth form is hidden when ENABLE_LOCAL_AUTH is disabled

- **Type:** boundary
- **Harness:** Local Postgres + Server (Playwright MCP)
- **Preconditions:**
  - Server running with `ENABLE_LOCAL_AUTH=0` (or unset)
- **Actions:**
  1. Navigate to `http://localhost:4000/auth/login`
  2. Inspect the page
- **Expected outcome:**
  - No email input field is present
  - No password input field is present
  - No local "Sign in" button is present
  - The Google "Sign in with Google" button IS still present
  - Source: implementation plan Task 7 — template wraps local form in `{% if enable_local_auth %}` conditional
- **Interactions:** Config parsing (ENABLE_LOCAL_AUTH=0 means form is absent)
- **Note:** This test requires restarting the server without `ENABLE_LOCAL_AUTH=1`. Run after Tests 1-3 or in a separate server session.

### Test 5: Config parses ENABLE_LOCAL_AUTH correctly

- **Type:** unit
- **Harness:** `cargo test`
- **Preconditions:** None (pure env var parsing)
- **Actions:** Verify at compile time that the `enable_local_auth` field exists on `Config` and is populated from `from_env()`. This is implicitly tested by a successful `cargo check -p boopmark-server`.
- **Expected outcome:**
  - `cargo check` passes with the new `enable_local_auth: bool` field
  - Source: implementation plan Task 4 — field added to Config struct and parsed in `from_env()`
- **Interactions:** None

### Test 6: User struct includes password_hash field — compilation gate

- **Type:** unit
- **Harness:** `cargo check`
- **Preconditions:** Migration 006 applied, User struct updated
- **Actions:** Run `cargo check -p boopmark-server`
- **Expected outcome:**
  - Compilation succeeds with `password_hash: Option<String>` on User
  - All existing queries updated to include `password_hash` in SELECT lists
  - Source: implementation plan Task 2
- **Interactions:** SQLx compile-time query checking (if enabled), sqlx::FromRow derive

### Test 7: hash_password example binary compiles and produces valid output

- **Type:** integration
- **Harness:** `cargo build` + shell
- **Preconditions:** argon2 workspace dep has `std` feature
- **Actions:**
  1. Run `cargo build -p boopmark-server --example hash_password`
  2. Run `cargo run -p boopmark-server --example hash_password -- testpass123`
- **Expected outcome:**
  - Build succeeds
  - Output is a non-empty string starting with `$argon2` (standard argon2 PHC string format)
  - Source: implementation plan Task 8 Step 1 — the example uses `Argon2::default().hash_password()` which produces PHC-format strings
- **Interactions:** argon2 crate hashing, OsRng salt generation

### Test 8: add-user script creates a user with hashed password in database

- **Type:** integration
- **Harness:** Local Postgres + shell
- **Preconditions:**
  - Postgres running (`docker compose up -d db`)
  - Migrations applied
  - `.env` sourced
- **Actions:**
  1. Run `./scripts/add-user.sh scripttest@local.dev mypass456`
  2. Query database: `psql "$DATABASE_URL" -c "SELECT email, password_hash IS NOT NULL as has_hash FROM users WHERE email = 'scripttest@local.dev'"`
- **Expected outcome:**
  - Script exits 0 with "Done!" message
  - Database query returns one row with `has_hash = true`
  - Source: implementation plan Task 8 Step 2 — script inserts user with hashed password via psql
- **Interactions:** hash_password example binary, psql, database

### Test 9: Existing tests still pass after all changes

- **Type:** regression
- **Harness:** `cargo test`
- **Preconditions:** All code changes applied
- **Actions:** Run `cargo test`
- **Expected outcome:**
  - All existing tests pass (no regressions)
  - Source: implementation plan Tasks 2, 4 — each task specifies "run existing tests, all pass"
- **Interactions:** All existing test suites

---

## Coverage Summary

### Covered

- **Login with valid credentials** (Test 1): The primary happy path — user enters email/password, gets authenticated and redirected to home
- **Login with invalid credentials** (Test 2): Error handling — wrong credentials show user-friendly error message
- **UI coexistence** (Test 3): Both local auth and Google OAuth appear together when local auth is enabled
- **Feature flag gating** (Test 4): Local auth form hidden when `ENABLE_LOCAL_AUTH` is off
- **Compilation correctness** (Tests 5, 6): New fields, traits, and methods compile without errors
- **Password hashing tool** (Test 7): The hash_password example produces valid argon2 hashes
- **User provisioning script** (Test 8): The add-user script correctly creates users in the database
- **Regression safety** (Test 9): Existing tests still pass

### Explicitly Excluded (per agreed strategy)

- **Committed Playwright specs:** User explicitly said not to write these. E2E verification via Playwright MCP only. Risk: no automated regression for local auth in CI. Mitigated by the feature being dev-only and behind a feature flag.
- **Google OAuth flow testing:** Out of scope for this task. Existing functionality, not changed.
- **Production deployment testing:** Local auth is explicitly development-only per the plan.
- **Concurrent login race conditions:** Unlikely for a dev-only feature; same accepted risk as noted in existing Google OAuth code.
- **Password strength validation:** Not in scope — the plan does not include password strength rules.
- **POST to `/auth/local-login` when `ENABLE_LOCAL_AUTH=0`:** The handler returns `Redirect::to("/auth/login")` — could be tested but is a minor boundary case covered by the feature flag gating test (Test 4) which confirms the form is not rendered.
