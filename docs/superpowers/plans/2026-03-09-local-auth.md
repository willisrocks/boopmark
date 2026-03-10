# Local Auth Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local username/password login option for development, controlled by `ENABLE_LOCAL_AUTH` env var, so developers can authenticate without Google OAuth when running behind devproxy.

**Architecture:** Follow existing `ENABLE_E2E_AUTH` pattern — env var in Config, conditional UI in template, new POST handler. Password hashing via argon2 (already a workspace dep). A shell script + justfile recipe for creating users.

**Tech Stack:** Rust, Axum, SQLx, Askama, argon2, psql, just

---

## Chunk 1: Database and Domain Layer

### Task 1: Migration — add password_hash column

**Files:**
- Create: `migrations/006_add_password_hash.sql`

- [ ] **Step 1: Write the migration**

```sql
ALTER TABLE users ADD COLUMN password_hash TEXT;
```

- [ ] **Step 2: Run the migration**

Run: `source .env && sqlx migrate run --source migrations`
Expected: Migration 006 applied successfully

- [ ] **Step 3: Commit**

```bash
git add migrations/006_add_password_hash.sql
git commit -m "feat: add password_hash column to users table"
```

---

### Task 2: Add password_hash to User struct and queries

**Files:**
- Modify: `server/src/domain/user.rs:5-12`
- Modify: `server/src/adapters/postgres/user_repo.rs:9-41`

- [ ] **Step 1: Add password_hash field to User struct**

In `server/src/domain/user.rs`, add `password_hash` to the `User` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
    pub password_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}
```

- [ ] **Step 2: Update all SQL queries in user_repo.rs to include password_hash**

In `server/src/adapters/postgres/user_repo.rs`, update each query's SELECT list:

`find_by_id`:
```rust
"SELECT id, email, name, image, password_hash, created_at FROM users WHERE id = $1"
```

`find_by_email`:
```rust
"SELECT id, email, name, image, password_hash, created_at FROM users WHERE email = $1"
```

`upsert`:
```rust
"INSERT INTO users (email, name, image) VALUES ($1, $2, $3)
 ON CONFLICT (email) DO UPDATE SET name = COALESCE($2, users.name), image = COALESCE($3, users.image)
 RETURNING id, email, name, image, password_hash, created_at"
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: Compiles with no errors

- [ ] **Step 4: Run existing tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add server/src/domain/user.rs server/src/adapters/postgres/user_repo.rs
git commit -m "feat: add password_hash field to User struct and queries"
```

---

### Task 3: Add upsert_with_password to UserRepository

**Files:**
- Modify: `server/src/domain/ports/user_repo.rs:1-10`
- Modify: `server/src/adapters/postgres/user_repo.rs`

- [ ] **Step 1: Add trait method**

In `server/src/domain/ports/user_repo.rs`, add:

```rust
async fn upsert_with_password(
    &self,
    email: &str,
    name: Option<&str>,
    password_hash: &str,
) -> Result<User, DomainError>;
```

- [ ] **Step 2: Implement in PostgresPool**

In `server/src/adapters/postgres/user_repo.rs`, add after the existing `upsert` method:

```rust
async fn upsert_with_password(
    &self,
    email: &str,
    name: Option<&str>,
    password_hash: &str,
) -> Result<User, DomainError> {
    sqlx::query_as::<_, User>(
        "INSERT INTO users (email, name, password_hash) VALUES ($1, $2, $3)
         ON CONFLICT (email) DO UPDATE SET
           name = COALESCE($2, users.name),
           password_hash = $3
         RETURNING id, email, name, image, password_hash, created_at",
    )
    .bind(email)
    .bind(name)
    .bind(password_hash)
    .fetch_one(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add server/src/domain/ports/user_repo.rs server/src/adapters/postgres/user_repo.rs
git commit -m "feat: add upsert_with_password to UserRepository"
```

---

## Chunk 2: Config and Auth Service

### Task 4: Add enable_local_auth to Config

**Files:**
- Modify: `server/src/config.rs:5-23` (struct) and `server/src/config.rs:32-66` (from_env)

- [ ] **Step 1: Add field to Config struct**

Add after `enable_e2e_auth`:

```rust
pub enable_local_auth: bool,
```

- [ ] **Step 2: Parse from env in from_env()**

Add after the `enable_e2e_auth` parsing block:

```rust
enable_local_auth: env::var("ENABLE_LOCAL_AUTH")
    .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
    .unwrap_or(false),
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add server/src/config.rs
git commit -m "feat: add ENABLE_LOCAL_AUTH config option"
```

---

### Task 5: Add local_login method to AuthService

**Files:**
- Modify: `server/src/app/auth.rs:1-10` (imports) and add method after `find_user_by_email`

- [ ] **Step 1: Add argon2 imports**

At the top of `server/src/app/auth.rs`, add:

```rust
use argon2::{Argon2, PasswordHash, PasswordVerifier};
```

- [ ] **Step 2: Add local_login method**

Add this method to the `impl` block, after `find_user_by_email`:

```rust
pub async fn local_login(
    &self,
    email: &str,
    password: &str,
) -> Result<(User, String), DomainError> {
    let user = self
        .users
        .find_by_email(email)
        .await?
        .ok_or(DomainError::Unauthorized)?;

    let hash = user
        .password_hash
        .as_deref()
        .ok_or(DomainError::Unauthorized)?;

    let parsed_hash =
        PasswordHash::new(hash).map_err(|_| DomainError::Internal("invalid hash".into()))?;

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .map_err(|_| DomainError::Unauthorized)?;

    let token = self.create_session(user.id).await?;
    Ok((user, token))
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add server/src/app/auth.rs
git commit -m "feat: add local_login method to AuthService"
```

---

## Chunk 3: Web Layer — Handler and Template

### Task 6: Add local login handler and route

**Files:**
- Modify: `server/src/web/pages/auth.rs:12-16` (LoginPage struct), `server/src/web/pages/auth.rs:18-25` (routes), `server/src/web/pages/auth.rs:27-36` (login_page handler)

- [ ] **Step 1: Add enable_local_auth to LoginPage struct**

```rust
#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage {
    enable_e2e_auth: bool,
    enable_local_auth: bool,
    login_error: Option<String>,
}
```

- [ ] **Step 2: Add query params struct for error display**

Add near the other Deserialize structs:

```rust
#[derive(Deserialize)]
struct LoginQueryParams {
    #[serde(default)]
    error: Option<String>,
}
```

- [ ] **Step 3: Update login_page handler to pass new fields**

```rust
async fn login_page(
    State(state): State<AppState>,
    Query(params): Query<LoginQueryParams>,
) -> impl IntoResponse {
    let page = LoginPage {
        enable_e2e_auth: state.config.enable_e2e_auth,
        enable_local_auth: state.config.enable_local_auth,
        login_error: params.error,
    };

    match page.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

- [ ] **Step 4: Add LocalLoginForm struct and handler**

```rust
#[derive(Deserialize)]
struct LocalLoginForm {
    email: String,
    password: String,
}

async fn local_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<LocalLoginForm>,
) -> Result<(CookieJar, Redirect), Redirect> {
    if !state.config.enable_local_auth {
        return Err(Redirect::to("/auth/login"));
    }

    let (_, token) = state
        .auth
        .local_login(&form.email, &form.password)
        .await
        .map_err(|_| Redirect::to("/auth/login?error=Invalid+email+or+password"))?;

    let origin = origin_from_headers(&headers, &state.config);
    let cookie = build_session_cookie(&origin, token);

    Ok((jar.add(cookie), Redirect::to("/")))
}
```

- [ ] **Step 5: Register the route**

In the `routes()` function, add:

```rust
.route("/auth/local-login", axum::routing::post(local_login))
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: Compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add server/src/web/pages/auth.rs
git commit -m "feat: add local login handler and route"
```

---

### Task 7: Update login template

**Files:**
- Modify: `templates/auth/login.html`

- [ ] **Step 1: Add local auth form to template**

Replace the content of `templates/auth/login.html` with:

```html
{% extends "base.html" %}
{% block title %}Sign In — BoopMark{% endblock %}
{% block content %}
<div class="flex items-center justify-center min-h-screen">
    <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-8 w-full max-w-sm text-center">
        <span class="text-4xl">🔖</span>
        <h1 class="text-xl font-bold mt-2 mb-6">BoopMark</h1>
        {% if let Some(ref err) = login_error %}
        <div class="mb-4 p-3 rounded-lg bg-red-900/50 border border-red-700 text-red-200 text-sm">
            {{ err }}
        </div>
        {% endif %}
        {% if enable_local_auth %}
        <form method="post" action="/auth/local-login" class="mb-4 space-y-3">
            <input type="email" name="email" placeholder="Email" required
                   class="w-full px-4 py-3 rounded-lg bg-[#161829] border border-gray-700 text-white placeholder-gray-500 focus:outline-none focus:border-blue-500" />
            <input type="password" name="password" placeholder="Password" required
                   class="w-full px-4 py-3 rounded-lg bg-[#161829] border border-gray-700 text-white placeholder-gray-500 focus:outline-none focus:border-blue-500" />
            <button type="submit"
                    class="w-full px-4 py-3 rounded-lg bg-blue-600 text-white font-medium hover:bg-blue-700 transition-colors">
                Sign in
            </button>
        </form>
        <div class="relative my-4">
            <div class="absolute inset-0 flex items-center"><div class="w-full border-t border-gray-700"></div></div>
            <div class="relative flex justify-center text-sm"><span class="bg-[#1e2235] px-2 text-gray-500">or</span></div>
        </div>
        {% endif %}
        <a href="/auth/google"
           class="flex items-center justify-center gap-2 px-4 py-3 rounded-lg bg-white text-gray-800 font-medium hover:bg-gray-100 transition-colors">
            <svg class="w-5 h-5" viewBox="0 0 24 24"><path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 01-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z"/><path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/><path fill="#FBBC05" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/><path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/></svg>
            Sign in with Google
        </a>
        {% if enable_e2e_auth %}
        <form method="post" action="/auth/test-login" class="mt-3">
            <button id="e2e-login-button"
                    type="submit"
                    class="w-full px-4 py-3 rounded-lg border border-gray-700 text-gray-200 hover:text-white">
                Sign in for E2E
            </button>
        </form>
        {% endif %}
    </div>
</div>
{% endblock %}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: Compiles (Askama templates are checked at compile time)

- [ ] **Step 3: Commit**

```bash
git add templates/auth/login.html
git commit -m "feat: add local auth form to login template"
```

---

## Chunk 4: Add-user Script and Infrastructure

### Task 8: Create add-user script and justfile recipe

**Files:**
- Modify: `Cargo.toml` (workspace deps — add `password-hash` feature to argon2)
- Create: `server/examples/hash_password.rs`
- Create: `scripts/add-user.sh`
- Modify: `justfile`

- [ ] **Step 0: Add password-hash feature to argon2 workspace dep**

In `Cargo.toml` (workspace root), change:

```toml
argon2 = "0.5"
```

to:

```toml
argon2 = { version = "0.5", features = ["std"] }
```

Note: argon2 0.5 re-exports `password_hash` and `rand_core` by default with the `std` feature. The `SaltString::generate` method uses `OsRng` from `password_hash::rand_core`, which is available with `std`.

- [ ] **Step 1: Create the hash-password example binary**

Create `server/examples/hash_password.rs`:

```rust
//! Tiny helper: reads a password from argv and prints its argon2 hash.
use argon2::{Argon2, PasswordHasher, password_hash::{SaltString, rand_core::OsRng}};

fn main() {
    let password = std::env::args().nth(1).expect("usage: hash_password <password>");
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("failed to hash password")
        .to_string();
    print!("{hash}");
}
```

- [ ] **Step 2: Create scripts/add-user.sh**

Note: argon2 hashes contain `$` characters so we must avoid shell interpolation.
Use psql `-v` variables for safe parameterisation (no SQL injection).

```bash
#!/usr/bin/env bash
set -euo pipefail

# Load .env if present
if [ -f .env ]; then
  set -a; source .env; set +a
fi

EMAIL="${1:-}"
PASSWORD="${2:-}"

if [ -z "$EMAIL" ]; then
  read -rp "Email: " EMAIL
fi
if [ -z "$PASSWORD" ]; then
  read -rsp "Password: " PASSWORD
  echo
fi

if [ -z "$EMAIL" ] || [ -z "$PASSWORD" ]; then
  echo "Error: email and password are required" >&2
  exit 1
fi

echo "Hashing password..."
HASH=$(cargo run -p boopmark-server --example hash_password -- "$PASSWORD" 2>/dev/null)

echo "Upserting user $EMAIL..."
psql "$DATABASE_URL" \
  -v "vemail=$EMAIL" \
  -v "vhash=$HASH" \
  -c "INSERT INTO users (email, name, password_hash)
      VALUES (:'vemail', :'vemail', :'vhash')
      ON CONFLICT (email) DO UPDATE SET password_hash = :'vhash';"

echo "Done! User $EMAIL can now log in with local auth."
```

- [ ] **Step 3: Make script executable**

Run: `chmod +x scripts/add-user.sh`

- [ ] **Step 4: Add justfile recipe**

Add to `justfile`:

```
add-user *ARGS:
    ./scripts/add-user.sh {{ARGS}}
```

- [ ] **Step 5: Verify the example compiles**

Run: `cargo build -p boopmark-server --example hash_password`
Expected: Compiles successfully

- [ ] **Step 6: Commit**

```bash
git add server/examples/hash_password.rs scripts/add-user.sh justfile
git commit -m "feat: add just add-user command for creating local auth users"
```

---

### Task 9: Docker Compose and env config

**Files:**
- Modify: `docker-compose.yml:33-36` (server environment block)
- Modify: `.env.example`

- [ ] **Step 1: Add ENABLE_LOCAL_AUTH to docker-compose.yml**

In the `server` service's `environment` block, add:

```yaml
      ENABLE_LOCAL_AUTH: "1"
```

- [ ] **Step 2: Add ENABLE_LOCAL_AUTH to .env.example**

Add after the `ENABLE_E2E_AUTH=0` line:

```
ENABLE_LOCAL_AUTH=0
```

- [ ] **Step 3: Commit**

```bash
git add docker-compose.yml .env.example
git commit -m "feat: enable local auth in Docker Compose by default"
```

---

### Task 10: Documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

- [ ] **Step 1: Update CLAUDE.md**

Add a section after "## Testing Notes":

```markdown
## Local Auth (Development)

When Google OAuth isn't available (e.g. behind devproxy), enable local username/password login:

1. Set `ENABLE_LOCAL_AUTH=1` in `.env` (already set in `docker-compose.yml`)
2. Create a user: `just add-user email@example.com mypassword`
3. Sign in with the local form on the login page

This is for development only — do not use in production.
```

- [ ] **Step 2: Update README.md Getting Started section**

Add a note about local auth after the existing setup instructions. Mention `ENABLE_LOCAL_AUTH=1` and `just add-user`.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: document local auth for development"
```

---

## Chunk 5: E2E Verification

### Task 11: End-to-end verification with agent-browser

- [ ] **Step 1: Start the dev stack**

Run: `docker compose up -d db minio` and then `ENABLE_LOCAL_AUTH=1 cargo run -p boopmark-server`

- [ ] **Step 2: Create a test user**

Run: `just add-user test@local.dev testpass123`

- [ ] **Step 3: Verify with agent-browser (Playwright MCP)**

Using the browser:
1. Navigate to `http://localhost:4000/auth/login`
2. Verify the local auth form is visible (email input, password input, sign-in button)
3. Verify the Google OAuth button is still present
4. Fill in `test@local.dev` / `testpass123` and submit
5. Verify redirect to `/` (home page, authenticated)
6. Test invalid credentials — verify error message appears
7. Test logout — verify redirect to login page

- [ ] **Step 4: Verify Google OAuth button still works** (links to `/auth/google`)

- [ ] **Step 5: Verify with ENABLE_LOCAL_AUTH=0 — local form should not appear**
