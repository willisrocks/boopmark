# Admin Controls Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add invite-only access, user roles (owner/admin/user), an admin page for managing invites and users, and a login provider abstraction for swappable auth backends.

**Architecture:** Extends the existing hexagonal architecture. New domain models (UserRole, Invite), new ports (InviteRepository, LoginProvider), new postgres adapter (invite_repo), new service (InviteService). The existing auth flow is refactored behind a LoginProvider trait with Google and LocalPassword adapters. Shared post-auth logic handles invite validation and user creation. Admin page is a new route tree at `/admin`.

**Tech Stack:** Rust, Axum 0.8, SQLx 0.8 (Postgres), Askama 0.12, HTMX 2, Tailwind CSS 4

**Spec:** `docs/superpowers/specs/2026-03-21-admin-controls-design.md`

---

## Chunk 1: Database & Domain Foundation

### Task 1: Database Migrations

**Files:**
- Create: `migrations/007_add_user_role_and_deactivated_at.sql`
- Create: `migrations/008_create_invites.sql`

- [ ] **Step 1: Write the role + deactivation migration**

```sql
-- migrations/007_add_user_role_and_deactivated_at.sql
CREATE TYPE user_role AS ENUM ('owner', 'admin', 'user');
ALTER TABLE users ADD COLUMN role user_role NOT NULL DEFAULT 'user';
ALTER TABLE users ADD COLUMN deactivated_at TIMESTAMPTZ;
```

- [ ] **Step 2: Write the invites table migration**

```sql
-- migrations/008_create_invites.sql
CREATE TABLE invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token TEXT UNIQUE NOT NULL,
    email TEXT,
    created_by UUID NOT NULL REFERENCES users(id),
    claimed_by UUID REFERENCES users(id),
    revoked_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

- [ ] **Step 3: Verify migrations compile**

Run: `cargo build -p boopmark-server`
Expected: Compiles (migrations are run at startup, not compile time, but sqlx may check them)

- [ ] **Step 4: Commit**

```bash
git add migrations/007_add_user_role_and_deactivated_at.sql migrations/008_create_invites.sql
git commit -m "feat: add migrations for user roles and invites table"
```

---

### Task 2: Domain Models, Ports, and Adapters — UserRole, Invite, and Query Updates

This task combines domain models, ports, and adapter updates into a single task so the codebase compiles at each commit. Adding fields to `User` requires updating all queries and test fakes simultaneously.

**Files:**
- Modify: `server/src/domain/user.rs` — add UserRole enum, deactivated_at field
- Create: `server/src/domain/invite.rs` — Invite and CreateInvite structs
- Modify: `server/src/domain/mod.rs` — add invite module
- Create: `server/src/domain/ports/invite_repo.rs` — InviteRepository trait
- Modify: `server/src/domain/ports/user_repo.rs` — add new methods
- Modify: `server/src/domain/ports/mod.rs` — add invite_repo module
- Create: `server/src/adapters/postgres/invite_repo.rs` — Postgres InviteRepository
- Modify: `server/src/adapters/postgres/user_repo.rs` — update queries for role/deactivated_at
- Modify: `server/src/adapters/postgres/mod.rs` — add invite_repo module
- Modify: `server/src/app/auth.rs` — update FakeUserRepo in tests to include new fields

- [ ] **Step 1: Add UserRole enum and update User struct**

In `server/src/domain/user.rs`:

```rust
use sqlx::Type;

#[derive(Debug, Clone, PartialEq, Eq, Type)]
#[sqlx(type_name = "user_role", rename_all = "lowercase")]
pub enum UserRole {
    Owner,
    Admin,
    User,
}

// Update User struct to add:
pub role: UserRole,
pub deactivated_at: Option<DateTime<Utc>>,
```

Add helper methods:

```rust
impl User {
    pub fn is_admin_or_owner(&self) -> bool {
        matches!(self.role, UserRole::Owner | UserRole::Admin)
    }

    pub fn is_owner(&self) -> bool {
        matches!(self.role, UserRole::Owner)
    }

    pub fn is_active(&self) -> bool {
        self.deactivated_at.is_none()
    }
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::Owner => "owner",
            UserRole::Admin => "admin",
            UserRole::User => "user",
        }
    }
}
```

- [ ] **Step 2: Create Invite domain model**

Create `server/src/domain/invite.rs`:

```rust
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Invite {
    pub id: Uuid,
    pub token: String,
    pub email: Option<String>,
    pub created_by: Uuid,
    pub claimed_by: Option<Uuid>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

pub struct CreateInvite {
    pub email: Option<String>,
    pub created_by: Uuid,
}

impl Invite {
    pub fn is_pending(&self) -> bool {
        self.claimed_by.is_none()
            && self.revoked_at.is_none()
            && self.expires_at > Utc::now()
    }

    pub fn status(&self) -> &'static str {
        if self.claimed_by.is_some() {
            "claimed"
        } else if self.revoked_at.is_some() {
            "revoked"
        } else if self.expires_at <= Utc::now() {
            "expired"
        } else {
            "pending"
        }
    }
}
```

- [ ] **Step 3: Wire up domain module**

Add `pub mod invite;` to `server/src/domain/mod.rs`.

- [ ] **Step 4: Create InviteRepository port**

Create `server/src/domain/ports/invite_repo.rs`. Use `#[trait_variant::make(Send)]` (not `async_trait` — the project uses `trait_variant` for all async traits):

```rust
use uuid::Uuid;
use crate::domain::error::DomainError;
use crate::domain::invite::{CreateInvite, Invite};

#[trait_variant::make(Send)]
pub trait InviteRepository: Sync {
    async fn create(&self, invite: &CreateInvite, token: &str, expires_at: chrono::DateTime<chrono::Utc>) -> Result<Invite, DomainError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<Invite>, DomainError>;
    async fn claim(&self, token: &str, user_id: Uuid) -> Result<(), DomainError>;
    async fn revoke(&self, invite_id: Uuid) -> Result<(), DomainError>;
    async fn list_all(&self) -> Result<Vec<Invite>, DomainError>;
}
```

- [ ] **Step 5: Update UserRepository port**

Add to the existing `UserRepository` trait in `server/src/domain/ports/user_repo.rs`:

```rust
async fn list_all(&self) -> Result<Vec<User>, DomainError>;
async fn update_role(&self, user_id: Uuid, role: UserRole) -> Result<(), DomainError>;
async fn deactivate(&self, user_id: Uuid) -> Result<(), DomainError>;
```

- [ ] **Step 6: Wire up ports module**

Add `pub mod invite_repo;` to `server/src/domain/ports/mod.rs`.

- [ ] **Step 7: Update user_repo.rs adapter queries**

In `server/src/adapters/postgres/user_repo.rs`, all existing queries that SELECT from users need `role` and `deactivated_at` columns. The adapter uses `self.pool` (not `self.0`). Update:

- `find_by_id`: add `role, deactivated_at` to SELECT
- `find_by_email`: add `role, deactivated_at` to SELECT
- `upsert`: add `role, deactivated_at` to the RETURNING clause

Add new method implementations:

```rust
async fn list_all(&self) -> Result<Vec<User>, DomainError> {
    let users = sqlx::query_as::<_, User>(
        "SELECT id, email, name, image, password_hash, role, deactivated_at, created_at FROM users ORDER BY created_at"
    )
    .fetch_all(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))?;
    Ok(users)
}

async fn update_role(&self, user_id: Uuid, role: UserRole) -> Result<(), DomainError> {
    sqlx::query("UPDATE users SET role = $1 WHERE id = $2")
        .bind(&role)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
    Ok(())
}

async fn deactivate(&self, user_id: Uuid) -> Result<(), DomainError> {
    sqlx::query("UPDATE users SET deactivated_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 8: Create invite_repo.rs adapter**

Create `server/src/adapters/postgres/invite_repo.rs`:

```rust
use uuid::Uuid;
use crate::adapters::postgres::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::invite::{CreateInvite, Invite};
use crate::domain::ports::invite_repo::InviteRepository;

impl InviteRepository for PostgresPool {
    async fn create(&self, invite: &CreateInvite, token: &str, expires_at: chrono::DateTime<chrono::Utc>) -> Result<Invite, DomainError> {
        let invite = sqlx::query_as::<_, Invite>(
            "INSERT INTO invites (token, email, created_by, expires_at) VALUES ($1, $2, $3, $4) RETURNING *"
        )
        .bind(token)
        .bind(&invite.email)
        .bind(invite.created_by)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(invite)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<Invite>, DomainError> {
        let invite = sqlx::query_as::<_, Invite>(
            "SELECT * FROM invites WHERE token = $1"
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(invite)
    }

    async fn claim(&self, token: &str, user_id: Uuid) -> Result<(), DomainError> {
        sqlx::query("UPDATE invites SET claimed_by = $1 WHERE token = $2")
            .bind(user_id)
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn revoke(&self, invite_id: Uuid) -> Result<(), DomainError> {
        sqlx::query("UPDATE invites SET revoked_at = now() WHERE id = $1")
            .bind(invite_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<Invite>, DomainError> {
        let invites = sqlx::query_as::<_, Invite>(
            "SELECT * FROM invites ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(invites)
    }
}
```

- [ ] **Step 9: Wire up adapter module**

Add `pub mod invite_repo;` to `server/src/adapters/postgres/mod.rs`.

- [ ] **Step 10: Update FakeUserRepo in auth.rs tests**

In `server/src/app/auth.rs`, the test module has a `FakeUserRepo` that constructs `User` structs. Update all `User` construction sites to include the new fields:

```rust
User {
    // ... existing fields ...
    role: UserRole::User,
    deactivated_at: None,
}
```

Also update any other test fakes across the codebase that construct `User` directly.

- [ ] **Step 11: Verify everything compiles**

Run: `cargo build -p boopmark-server && cargo test -p boopmark-server`
Expected: Compiles and all existing tests pass

- [ ] **Step 12: Commit**

```bash
git add server/src/domain/ server/src/adapters/postgres/ server/src/app/auth.rs
git commit -m "feat: add UserRole, Invite model, ports, and postgres adapters"
```

---

## Chunk 2: Services & Auth Updates

### Task 3: InviteService

**Files:**
- Create: `server/src/app/invite.rs`
- Modify: `server/src/app/mod.rs` — add invite module

- [ ] **Step 1: Write unit tests for InviteService**

At the bottom of `server/src/app/invite.rs`, add a test module with a `FakeInviteRepository`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // FakeInviteRepository with Arc<Mutex<Vec<Invite>>> storage
    // Tests:
    // - create_invite: generates token, sets 7-day expiry, returns invite
    // - create_invite: stores email if provided
    // - validate_token: returns invite for valid pending token
    // - validate_token: returns None for expired token
    // - validate_token: returns None for claimed token
    // - validate_token: returns None for revoked token
    // - validate_token: returns None for nonexistent token
    // - claim_invite: marks invite as claimed
    // - revoke_invite: marks invite as revoked
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p boopmark-server invite`
Expected: FAIL — InviteService not yet implemented

- [ ] **Step 3: Implement InviteService**

```rust
use chrono::{Duration, Utc};
use rand::Rng;
use std::sync::Arc;
use uuid::Uuid;
use crate::domain::error::DomainError;
use crate::domain::invite::{CreateInvite, Invite};
use crate::domain::ports::invite_repo::InviteRepository;

pub struct InviteService<R: InviteRepository> {
    repo: Arc<R>,
}

impl<R: InviteRepository> InviteService<R> {
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    pub async fn create_invite(&self, created_by: Uuid, email: Option<String>) -> Result<Invite, DomainError> {
        let token = generate_token();
        let expires_at = Utc::now() + Duration::days(7);
        let input = CreateInvite { email, created_by };
        self.repo.create(&input, &token, expires_at).await
    }

    /// Returns the invite if the token is valid and pending (not claimed, not revoked, not expired).
    pub async fn validate_token(&self, token: &str) -> Result<Option<Invite>, DomainError> {
        let invite = self.repo.find_by_token(token).await?;
        Ok(invite.filter(|i| i.is_pending()))
    }

    pub async fn claim_invite(&self, token: &str, user_id: Uuid) -> Result<(), DomainError> {
        self.repo.claim(token, user_id).await
    }

    pub async fn revoke_invite(&self, invite_id: Uuid) -> Result<(), DomainError> {
        self.repo.revoke(invite_id).await
    }

    pub async fn list_invites(&self) -> Result<Vec<Invite>, DomainError> {
        self.repo.list_all().await
    }
}

fn generate_token() -> String {
    use rand::distr::Alphanumeric;
    rand::rng().sample_iter(Alphanumeric).take(32).map(char::from).collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p boopmark-server invite`
Expected: All tests PASS

- [ ] **Step 5: Wire up app module**

Add `pub mod invite;` to `server/src/app/mod.rs`.

- [ ] **Step 6: Commit**

```bash
git add server/src/app/invite.rs server/src/app/mod.rs
git commit -m "feat: add InviteService with token generation and validation"
```

---

### Task 4: AuthService Updates — Deactivation Checks

**Files:**
- Modify: `server/src/app/auth.rs` — add deactivation check to session validation and login

- [ ] **Step 1: Write tests for deactivation**

Add to existing test module in `server/src/app/auth.rs`:

```rust
// - validate_session: returns error for deactivated user
// - local_login: returns error for deactivated user
// - create_session: works normally (no deactivation check at creation time)
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p boopmark-server auth`
Expected: FAIL — deactivation not checked yet

- [ ] **Step 3: Add deactivation checks**

In `validate_session`: after finding the user, check `user.is_active()`. If not, return `DomainError::Unauthorized`.

In `local_login`: after verifying password, check `user.is_active()`. If not, return `DomainError::Unauthorized`.

In `validate_api_key`: after finding the user, check `user.is_active()`. If not, return `DomainError::Unauthorized`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p boopmark-server auth`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add server/src/app/auth.rs
git commit -m "feat: block deactivated users from login and session validation"
```

---

### Task 5: Add Role-Based Extractors

**Files:**
- Modify: `server/src/web/extractors.rs` — add `AdminUser` extractor

- [ ] **Step 1: Add AdminUser extractor**

Follow the existing pattern in `extractors.rs` — `AuthUser` implements `FromRequestParts<AppState>` directly (not with a generic `S`):

```rust
/// Requires authenticated user with owner or admin role.
/// Returns 403 Forbidden if the user doesn't have the required role.
pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        if user.is_admin_or_owner() {
            Ok(AdminUser(user))
        } else {
            Err(StatusCode::FORBIDDEN)
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 3: Commit**

```bash
git add server/src/web/extractors.rs
git commit -m "feat: add AdminUser extractor for role-based access control"
```

---

## Chunk 3: Login Provider Refactor

### Task 6: Config Changes

**Files:**
- Modify: `server/src/config.rs` — add `LoginAdapter` enum, replace `enable_local_auth`, make Google creds optional

- [ ] **Step 1: Add LoginAdapter enum and update Config**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum LoginAdapter {
    Google,
    LocalPassword,
}

// In Config struct, replace:
//   enable_local_auth: bool
// With:
//   login_adapter: LoginAdapter

// Make google_client_id and google_client_secret Option<String>
// They're required only when login_adapter == Google
```

- [ ] **Step 2: Update from_env()**

```rust
let login_adapter = match std::env::var("LOGIN_ADAPTER").unwrap_or_else(|_| "google".to_string()).as_str() {
    "local_password" => LoginAdapter::LocalPassword,
    "google" | _ => LoginAdapter::Google,
};

// Validate: if login_adapter == Google, require GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET
let google_client_id = std::env::var("GOOGLE_CLIENT_ID").ok();
let google_client_secret = std::env::var("GOOGLE_CLIENT_SECRET").ok();

if login_adapter == LoginAdapter::Google {
    if google_client_id.is_none() || google_client_secret.is_none() {
        panic!("GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET required when LOGIN_ADAPTER=google");
    }
}
```

- [ ] **Step 3: Update docker-compose.yml**

Replace `ENABLE_LOCAL_AUTH: "1"` with `LOGIN_ADAPTER: "local_password"`.

- [ ] **Step 4: Update E2E start script**

In `scripts/e2e/start-server.sh`, replace `ENABLE_LOCAL_AUTH=1` with `LOGIN_ADAPTER=local_password` (if present).

- [ ] **Step 5: Update .env.example**

Replace `ENABLE_LOCAL_AUTH=0` with `LOGIN_ADAPTER=google` in `.env.example`.

- [ ] **Step 6: Fix all compile errors from Config changes**

Any code referencing `config.enable_local_auth` or `config.google_client_id` (non-optional) needs updating.

- [ ] **Step 7: Run tests**

Run: `cargo test -p boopmark-server`
Expected: All existing tests pass

- [ ] **Step 8: Commit**

```bash
git add server/src/config.rs docker-compose.yml scripts/ .env.example
git commit -m "feat: replace ENABLE_LOCAL_AUTH with LOGIN_ADAPTER config"
```

---

### Task 7: LoginProvider Trait and Shared Post-Auth Logic

**Files:**
- Create: `server/src/domain/ports/login_provider.rs` — LoginProvider trait
- Create: `server/src/web/pages/auth_shared.rs` — shared post-auth function
- Modify: `server/src/domain/ports/mod.rs` — add login_provider module

- [ ] **Step 1: Create LoginProvider trait**

Create `server/src/domain/ports/login_provider.rs`. Use `Router<AppState>` directly — this matches the existing pattern in `pages/auth.rs::routes()`:

```rust
use axum::Router;
use crate::web::state::AppState;

/// Identity returned by a login adapter after successful authentication.
pub struct AuthenticatedIdentity {
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
}

/// Context for rendering the login page.
pub struct LoginPageContext {
    pub provider_name: String, // "google" or "local_password"
}

/// Port for login provider adapters.
pub trait LoginProvider: Send + Sync + 'static {
    fn routes(&self) -> Router<AppState>;
    fn login_page_context(&self) -> LoginPageContext;
}
```

- [ ] **Step 2: Create shared post-auth logic**

Create `server/src/web/pages/auth_shared.rs`. This function is called by login adapters after successful authentication. Follow the cookie-setting pattern from `google_callback` in `pages/auth.rs` — use `axum_extra::extract::cookie::{CookieJar, Cookie}`:

```rust
use askama::Template;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use crate::domain::ports::login_provider::AuthenticatedIdentity;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/invite_only.html")]
struct InviteOnlyPage;

/// Called by login adapters after successful authentication.
/// Handles invite validation, user creation, and session management.
/// Returns (Response, CookieJar) — the adapter should return both.
pub async fn handle_authenticated_identity(
    state: &AppState,
    identity: AuthenticatedIdentity,
    jar: CookieJar,
) -> (CookieJar, Response) {
    // Read invite token from cookie (set by /invite/{token} handler)
    let invite_token = jar.get("invite_token").map(|c| c.value().to_string());

    // 1. Check if user exists by email
    let existing_user = state.auth.find_user_by_email(&identity.email).await.ok().flatten();

    match existing_user {
        Some(user) if !user.is_active() => {
            // Deactivated user
            (jar, Redirect::to("/auth/login?error=deactivated").into_response())
        }
        Some(user) => {
            // Existing active user — create session, no invite needed
            let token = state.auth.create_session(user.id).await.unwrap();
            let jar = jar.add(Cookie::build(("session", token)).path("/"));
            (jar, Redirect::to("/bookmarks").into_response())
        }
        None => {
            // New user — need an invite
            match invite_token {
                Some(ref token) => {
                    match state.invites.validate_token(token).await.ok().flatten() {
                        Some(_invite) => {
                            // Create user via upsert, claim invite, create session
                            let user = state.auth.upsert_user(
                                &identity.email,
                                identity.name.as_deref(),
                                identity.image.as_deref(),
                            ).await.unwrap();
                            state.invites.claim_invite(token, user.id).await.ok();
                            let session_token = state.auth.create_session(user.id).await.unwrap();
                            let jar = jar
                                .add(Cookie::build(("session", session_token)).path("/"))
                                .remove(Cookie::build("invite_token").path("/"));
                            (jar, Redirect::to("/bookmarks").into_response())
                        }
                        None => {
                            // Invalid/expired invite token
                            let jar = jar.remove(Cookie::build("invite_token").path("/"));
                            (jar, Redirect::to("/auth/login?error=invite_invalid").into_response())
                        }
                    }
                }
                None => {
                    // No invite — show "invite only" page
                    (jar, InviteOnlyPage.into_response())
                }
            }
        }
    }
}
```

- [ ] **Step 3: Wire up modules**

Add `pub mod login_provider;` to `server/src/domain/ports/mod.rs`.
Add `pub mod auth_shared;` to `server/src/web/pages/mod.rs`.

- [ ] **Step 4: Commit**

```bash
git add server/src/domain/ports/login_provider.rs server/src/web/pages/auth_shared.rs server/src/domain/ports/mod.rs
git commit -m "feat: add LoginProvider trait and shared post-auth logic"
```

---

### Task 8: Google OAuth Adapter

**Files:**
- Create: `server/src/adapters/login/mod.rs`
- Create: `server/src/adapters/login/google.rs` — extract Google OAuth from `pages/auth.rs`
- Modify: `server/src/adapters/mod.rs` — add login module

- [ ] **Step 1: Create Google OAuth adapter**

Extract the Google OAuth logic from `server/src/web/pages/auth.rs` into `server/src/adapters/login/google.rs`:

```rust
pub struct GoogleOAuthProvider {
    pub client_id: String,
    pub client_secret: String,
}

impl GoogleOAuthProvider {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self { client_id, client_secret }
    }
}

impl LoginProvider for GoogleOAuthProvider {
    fn routes(&self) -> Router<AppState> {
        let client_id = self.client_id.clone();
        let client_secret = self.client_secret.clone();

        Router::new()
            .route("/auth/google", get({
                let cid = client_id.clone();
                move |State(state): State<AppState>, headers: HeaderMap| {
                    google_redirect(state, headers, cid.clone())
                }
            }))
            .route("/auth/google/callback", get({
                let cid = client_id.clone();
                let cs = client_secret.clone();
                move |State(state): State<AppState>, Query(params): Query<CallbackParams>, jar: CookieJar| {
                    google_callback(state, params, jar, cid.clone(), cs.clone())
                }
            }))
    }

    fn login_page_context(&self) -> LoginPageContext {
        LoginPageContext { provider_name: "google".to_string() }
    }
}
```

Move `google_redirect`, `google_callback`, `download_and_store_avatar` functions from `pages/auth.rs` into this module. Update `google_callback` to call `handle_authenticated_identity` instead of directly doing upsert + session creation.

- [ ] **Step 2: Create adapter module**

`server/src/adapters/login/mod.rs`:

```rust
pub mod google;
pub mod local_password;
```

Add `pub mod login;` to `server/src/adapters/mod.rs`.

- [ ] **Step 3: Commit**

```bash
git add server/src/adapters/login/
git commit -m "feat: extract Google OAuth into LoginProvider adapter"
```

---

### Task 9: Local Password Adapter

**Files:**
- Create: `server/src/adapters/login/local_password.rs` — extract local auth from `pages/auth.rs`

- [ ] **Step 1: Create Local Password adapter**

Extract local login from `server/src/web/pages/auth.rs`:

```rust
pub struct LocalPasswordProvider;

impl LoginProvider for LocalPasswordProvider {
    fn routes(&self) -> Router<AppState> {
        Router::new()
            .route("/auth/local-login", post(local_login))
    }

    fn login_page_context(&self) -> LoginPageContext {
        LoginPageContext { provider_name: "local_password".to_string() }
    }
}
```

Move the `local_login` handler from `pages/auth.rs`. Update it to call `handle_authenticated_identity` after password verification — or, since local login already has the user from the DB, it can directly create a session (the user already exists by definition for local password login, so invite checks don't apply).

Actually, for local password login: the user must already exist in the DB (they were created via invite + another adapter, or via `just add-user`). So the handler just verifies the password and creates a session. No invite check needed since the user already exists. The deactivation check is already in `AuthService::local_login`.

- [ ] **Step 2: Commit**

```bash
git add server/src/adapters/login/local_password.rs
git commit -m "feat: extract local password into LoginProvider adapter"
```

---

### Task 10: Wire Up LoginProvider in Main and Router

**Files:**
- Modify: `server/src/main.rs` — create the selected LoginProvider, add to AppState
- Modify: `server/src/web/state.rs` — add `login_provider` and `invites` to AppState
- Modify: `server/src/web/pages/auth.rs` — remove extracted handlers, keep login_page + logout + E2E, use LoginProvider for routes
- Modify: `server/src/web/router.rs` — merge LoginProvider routes here (has access to state)

- [ ] **Step 1: Update AppState**

Add to `AppState` struct in `server/src/web/state.rs`:

```rust
pub login_provider: Arc<dyn LoginProvider>,
pub invites: Arc<InviteService<PostgresPool>>,
```

- [ ] **Step 2: Update main.rs**

In `server/src/main.rs`, after creating the pool and config:

```rust
// Create InviteService
let invite_service = Arc::new(InviteService::new(pool.clone()));

// Create LoginProvider based on config
let login_provider: Arc<dyn LoginProvider> = match config.login_adapter {
    LoginAdapter::Google => {
        let client_id = config.google_client_id.clone().unwrap();
        let client_secret = config.google_client_secret.clone().unwrap();
        Arc::new(GoogleOAuthProvider::new(client_id, client_secret))
    }
    LoginAdapter::LocalPassword => Arc::new(LocalPasswordProvider),
};
```

Add both to AppState construction.

- [ ] **Step 3: Update auth routes**

In `server/src/web/pages/auth.rs`, the `routes()` function should:
- Keep `/auth/login` (login page) — now uses `state.login_provider.login_page_context()` to decide what to render
- Keep `/auth/logout`
- Keep `/auth/test-login` (E2E, guarded by config flag)
- Remove Google and local password routes (now owned by adapters)

- [ ] **Step 4: Merge LoginProvider routes in router.rs**

The LoginProvider routes must be merged in `web/router.rs::create_router()` because it already has access to the `AppState`. The `pages::routes()` free function does NOT have access to state, so it cannot call `state.login_provider.routes()`.

In `server/src/web/router.rs`:

```rust
pub fn create_router(state: AppState) -> Router {
    let login_routes = state.login_provider.routes();
    Router::new()
        .merge(pages::routes())
        .merge(login_routes)  // LoginProvider's auth routes
        .nest("/api/v1", api::routes())
        // ... rest unchanged
        .with_state(state)
}
```

- [ ] **Step 5: Update login page handler**

The login page handler should get the provider context from `state.login_provider.login_page_context()` and pass `provider_name` to the template.

- [ ] **Step 6: Verify everything compiles and existing auth still works**

Run: `cargo build -p boopmark-server`
Run: `cargo test -p boopmark-server`
Expected: Compiles and tests pass

- [ ] **Step 7: Commit**

```bash
git add server/src/main.rs server/src/web/
git commit -m "feat: wire up LoginProvider in AppState and router"
```

---

## Chunk 4: Invite Flow & Pages

### Task 11: Invite Landing Page

**Files:**
- Create: `templates/auth/invite.html` — "You're invited" landing page
- Create: `templates/auth/invite_invalid.html` — invalid/expired invite page
- Create: `templates/auth/invite_only.html` — "You need an invite" page
- Create: `server/src/web/pages/invite.rs` — invite page handlers
- Modify: `server/src/web/pages/mod.rs` — add invite routes

- [ ] **Step 1: Create invite landing page template**

`templates/auth/invite.html`:

```html
{% extends "base.html" %}
{% block title %}You're Invited - BoopMark{% endblock %}
{% block content %}
<main class="min-h-screen flex items-center justify-center px-6">
    <div class="text-center max-w-sm">
        <img src="/static/boopmark-logo.svg" alt="" class="w-12 h-12 mx-auto mb-4">
        <h1 class="text-2xl font-bold mb-2">You're invited to BoopMark</h1>
        <p class="text-sm text-gray-400 mb-6">Sign in to create your account and start bookmarking.</p>
        {% if provider_name == "google" %}
        <a href="/auth/google"
           class="inline-flex items-center gap-2 px-6 py-3 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">
            <!-- Google icon SVG -->
            Sign in with Google
        </a>
        {% else %}
        <form method="post" action="/auth/local-login" class="space-y-3 text-left max-w-xs mx-auto">
            <input type="email" name="email" placeholder="Email" required
                class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200">
            <input type="password" name="password" placeholder="Password" required
                class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200">
            <button type="submit" class="w-full px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">
                Sign in
            </button>
        </form>
        {% endif %}
    </div>
</main>
{% endblock %}
```

- [ ] **Step 2: Create invalid invite and invite-only templates**

`templates/auth/invite_invalid.html`: Shows "Invite not valid" with guidance.

`templates/auth/invite_only.html`: Shows "BoopMark is invite-only" message.

Both follow the same centered layout pattern as the invite landing page.

- [ ] **Step 3: Create invite page handlers**

`server/src/web/pages/invite.rs`:

```rust
use axum::{extract::{Path, State}, response::IntoResponse};
use askama::Template;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/invite.html")]
struct InvitePage {
    provider_name: String,
}

#[derive(Template)]
#[template(path = "auth/invite_invalid.html")]
struct InviteInvalidPage;

pub async fn invite_landing(
    State(state): State<AppState>,
    Path(token): Path<String>,
    jar: CookieJar,
) -> impl IntoResponse {
    // Validate the invite token
    match state.invites.validate_token(&token).await {
        Ok(Some(_invite)) => {
            // Store token in a cookie so the OAuth callback can find it
            let jar = jar.add(Cookie::build(("invite_token", token)).path("/"));
            let context = state.login_provider.login_page_context();
            (jar, InvitePage { provider_name: context.provider_name }).into_response()
        }
        _ => {
            InviteInvalidPage.into_response()
        }
    }
}
```

- [ ] **Step 4: Add invite routes**

In `server/src/web/pages/mod.rs`, add:

```rust
.route("/invite/{token}", get(invite::invite_landing))
```

- [ ] **Step 5: Update shared post-auth to read invite_token from cookie**

In `auth_shared.rs`, read the `invite_token` cookie from the request and pass it to the handler. After claiming, remove the cookie.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 7: Commit**

```bash
git add templates/auth/ server/src/web/pages/invite.rs server/src/web/pages/mod.rs server/src/web/pages/auth_shared.rs
git commit -m "feat: add invite landing page and token validation flow"
```

---

### Task 12: Update Login Page Template

**Files:**
- Modify: `templates/auth/login.html` — render based on provider, use blue Google button
- Modify: `server/src/web/pages/auth.rs` — pass provider context to login template

- [ ] **Step 1: Update login page handler**

The `login_page` handler in `auth.rs` should get the provider context from `state.login_provider.login_page_context()` and pass `provider_name` to the template.

- [ ] **Step 2: Update login template**

```html
{% if provider_name == "google" %}
<a href="/auth/google"
   class="inline-flex items-center gap-2 px-6 py-3 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium w-full justify-center">
    <!-- Google SVG icon with white fill -->
    Sign in with Google
</a>
{% else %}
<!-- Existing email/password form -->
{% endif %}
```

Remove the old conditional `enable_local_auth` logic. The E2E test login button remains guarded by `enable_e2e_auth`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p boopmark-server`

- [ ] **Step 4: Commit**

```bash
git add templates/auth/login.html server/src/web/pages/auth.rs
git commit -m "feat: update login page to render based on login adapter"
```

---

## Chunk 5: Admin UI

### Task 13: Admin Page — Routes and Handlers

**Files:**
- Create: `server/src/web/pages/admin.rs` — admin page handlers
- Modify: `server/src/web/pages/mod.rs` — add admin routes

- [ ] **Step 1: Create admin page handler**

`server/src/web/pages/admin.rs`:

```rust
use axum::{extract::State, response::IntoResponse, Form};
use askama::Template;
use crate::web::extractors::AdminUser;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "admin/index.html")]
struct AdminPage {
    header_shows_bookmark_actions: bool, // false — required by header.html
    user: Option<UserView>, // required by header.html
    invites: Vec<InviteView>,
    users: Vec<AdminUserView>,
}

struct InviteView {
    id: String,
    token: String,
    email: Option<String>,
    created_by_name: String,
    status: String,
    is_pending: bool,
    app_url: String,
}

struct AdminUserView {
    id: String,
    email: String,
    name: Option<String>,
    image: Option<String>,
    role: String,
    is_owner: bool,
    is_self: bool,
    created_at: String,
    is_active: bool,
}

pub async fn admin_page(
    AdminUser(user): AdminUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let invites = state.invites.list_invites().await.unwrap_or_default();
    let users = state.auth.list_users().await.unwrap_or_default();
    // Map to view models and render
    AdminPage { /* ... */ }
}

// HTMX handlers for:
pub async fn create_invite(AdminUser(user): AdminUser, State(state): State<AppState>, Form(form): Form<CreateInviteForm>) -> impl IntoResponse { /* ... */ }
pub async fn revoke_invite(AdminUser(user): AdminUser, State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse { /* ... */ }
pub async fn update_user_role(AdminUser(user): AdminUser, State(state): State<AppState>, Path(id): Path<Uuid>, Form(form): Form<UpdateRoleForm>) -> impl IntoResponse { /* ... */ }
pub async fn deactivate_user(AdminUser(user): AdminUser, State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse { /* ... */ }
```

- [ ] **Step 2: Add admin routes**

In `server/src/web/pages/mod.rs`, add admin routes that use the `AdminUser` extractor:

```rust
.route("/admin", get(admin::admin_page))
.route("/admin/invites", post(admin::create_invite))
.route("/admin/invites/{id}/revoke", post(admin::revoke_invite))
.route("/admin/users/{id}/role", post(admin::update_user_role))
.route("/admin/users/{id}/deactivate", post(admin::deactivate_user))
```

- [ ] **Step 3: Add list_users to AuthService**

Add `pub async fn list_users(&self) -> Result<Vec<User>, DomainError>` to `AuthService` that delegates to `self.users.list_all()`.

- [ ] **Step 4: Commit**

```bash
git add server/src/web/pages/admin.rs server/src/web/pages/mod.rs server/src/app/auth.rs
git commit -m "feat: add admin page handlers and routes"
```

---

### Task 14: Admin Page Template

**Files:**
- Create: `templates/admin/index.html` — main admin page
- Create: `templates/admin/invite_list.html` — invite list HTMX fragment
- Create: `templates/admin/user_list.html` — user list HTMX fragment

- [ ] **Step 1: Create admin page template**

`templates/admin/index.html` — follows the existing settings page pattern (`templates/settings/index.html`):

```html
{% extends "base.html" %}
{% block title %}Admin - BoopMark{% endblock %}
{% block content %}
{% include "components/header.html" %}
<main class="max-w-3xl mx-auto px-6 py-8">
    <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-8 space-y-8">
        <h1 class="text-2xl font-bold">Admin</h1>

        <!-- Invites Section -->
        <section class="space-y-5">
            <div>
                <h2 class="text-lg font-semibold">Invites</h2>
                <p class="text-sm text-gray-400">Create single-use invite links. Links expire after 7 days.</p>
            </div>
            <form hx-post="/admin/invites" hx-target="#invite-list" hx-swap="innerHTML" class="flex gap-3">
                <input type="text" name="email" placeholder="Email (optional)"
                    class="flex-1 px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 text-sm">
                <button type="submit" class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">
                    Create Invite
                </button>
            </form>
            <div id="invite-list">
                {% include "admin/invite_list.html" %}
            </div>
        </section>

        <!-- Users Section -->
        <section class="space-y-5">
            <div>
                <h2 class="text-lg font-semibold">Users</h2>
                <p class="text-sm text-gray-400">Manage users and their roles.</p>
            </div>
            <div id="user-list">
                {% include "admin/user_list.html" %}
            </div>
        </section>
    </div>
</main>
{% endblock %}
```

- [ ] **Step 2: Create invite list fragment**

`templates/admin/invite_list.html` — table with columns: Link/Email, Created by, Status, Actions (Revoke for pending). Include copy-to-clipboard JS for invite links.

- [ ] **Step 3: Create user list fragment**

`templates/admin/user_list.html` — table with columns: User (avatar + name + email), Role (dropdown for non-owner), Joined, Actions (Deactivate). Role dropdown uses HTMX to POST role changes. Owner row has no controls.

Role change permission logic:
- Owner sees admin/user dropdown for everyone except themselves
- Admin sees admin/user dropdown only for non-admins (can't touch other admins or owner)

- [ ] **Step 4: Verify templates compile with Askama**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add templates/admin/
git commit -m "feat: add admin page templates with invite and user management"
```

---

### Task 15: Nav Changes — Admin Link in Header

**Files:**
- Modify: `templates/components/header.html` — add Admin link to profile dropdown
- Modify: `server/src/web/pages/shared.rs` — add role to UserView

- [ ] **Step 1: Add role to UserView**

In `server/src/web/pages/shared.rs`, add `is_admin_or_owner: bool` to `UserView` and update the `From<User>` impl to populate it from `user.is_admin_or_owner()`.

- [ ] **Step 2: Update header template**

In `templates/components/header.html`, add the Admin link to the profile dropdown menu, after the Settings link:

```html
{% if user.is_admin_or_owner %}
<a href="/admin" class="block text-sm text-gray-300 hover:text-white py-1">Admin</a>
{% endif %}
```

Add this in both instances of the dropdown (the template has two: one for bookmark pages, one for other pages).

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p boopmark-server`

- [ ] **Step 4: Commit**

```bash
git add templates/components/header.html server/src/web/pages/shared.rs
git commit -m "feat: add Admin link to header dropdown for owner/admin roles"
```

---

### Task 16: Final Integration and Tailwind

**Files:**
- Modify: `static/css/output.css` — rebuild Tailwind (if using Tailwind build step)
- Various — fix any remaining compile errors

- [ ] **Step 1: Rebuild Tailwind CSS**

Run the Tailwind CSS build to pick up any new classes used in admin templates:

```bash
npx @tailwindcss/cli -i static/css/input.css -o static/css/output.css
```

(Check the project's existing Tailwind build command — it may differ.)

- [ ] **Step 2: Full build and test**

Run: `cargo build -p boopmark-server && cargo test -p boopmark-server`
Expected: All tests pass, no compile errors

- [ ] **Step 3: Manual smoke test**

Start the dev server with `devproxy up` and verify:
1. Login page shows correct form based on `LOGIN_ADAPTER`
2. `/admin` page loads for admin/owner users
3. `/admin` returns 403 for regular users
4. Creating an invite generates a link
5. Visiting `/invite/{token}` shows the landing page
6. Using an invite link allows signup
7. Uninvited users see "invite only" page after OAuth
8. Role changes work via dropdown
9. Deactivation blocks user login

- [ ] **Step 4: Run E2E tests**

Run: `npx playwright test tests/e2e/suggest.spec.js`
Expected: E2E tests pass (E2E auth bypasses invite checks)

- [ ] **Step 5: Commit**

```bash
git add static/css/output.css
git commit -m "chore: rebuild Tailwind CSS for admin templates"
```

---

## File Map Summary

### New Files
| File | Purpose |
|---|---|
| `migrations/007_add_user_role_and_deactivated_at.sql` | Add role enum and deactivated_at to users |
| `migrations/008_create_invites.sql` | Create invites table |
| `server/src/domain/invite.rs` | Invite domain model |
| `server/src/domain/ports/invite_repo.rs` | InviteRepository port |
| `server/src/domain/ports/login_provider.rs` | LoginProvider port |
| `server/src/adapters/postgres/invite_repo.rs` | Postgres InviteRepository |
| `server/src/adapters/login/mod.rs` | Login adapter module |
| `server/src/adapters/login/google.rs` | Google OAuth LoginProvider |
| `server/src/adapters/login/local_password.rs` | Local password LoginProvider |
| `server/src/app/invite.rs` | InviteService |
| `server/src/web/pages/admin.rs` | Admin page handlers |
| `server/src/web/pages/invite.rs` | Invite landing page handler |
| `server/src/web/pages/auth_shared.rs` | Shared post-auth logic |
| `templates/admin/index.html` | Admin page template |
| `templates/admin/invite_list.html` | Invite list HTMX fragment |
| `templates/admin/user_list.html` | User list HTMX fragment |
| `templates/auth/invite.html` | Invite landing page |
| `templates/auth/invite_invalid.html` | Invalid invite page |
| `templates/auth/invite_only.html` | "Invite only" page |

### Modified Files
| File | Change |
|---|---|
| `server/src/domain/user.rs` | Add UserRole, deactivated_at, helper methods |
| `server/src/domain/mod.rs` | Add invite module |
| `server/src/domain/ports/mod.rs` | Add invite_repo, login_provider modules |
| `server/src/domain/ports/user_repo.rs` | Add list_all, update_role, deactivate |
| `server/src/adapters/mod.rs` | Add login module |
| `server/src/adapters/postgres/mod.rs` | Add invite_repo module |
| `server/src/adapters/postgres/user_repo.rs` | Update queries for role/deactivated_at, add new methods |
| `server/src/app/mod.rs` | Add invite module |
| `server/src/app/auth.rs` | Add deactivation checks, list_users method |
| `server/src/config.rs` | Add LoginAdapter enum, replace enable_local_auth |
| `server/src/main.rs` | Wire up InviteService, LoginProvider |
| `server/src/web/state.rs` | Add login_provider and invites to AppState |
| `server/src/web/extractors.rs` | Add AdminUser extractor |
| `server/src/web/pages/mod.rs` | Add admin and invite routes, merge provider routes |
| `server/src/web/pages/auth.rs` | Remove extracted handlers, use LoginProvider context |
| `server/src/web/pages/shared.rs` | Add role to UserView |
| `templates/auth/login.html` | Render based on provider, blue Google button |
| `templates/components/header.html` | Add Admin link to dropdown |
| `docker-compose.yml` | Replace ENABLE_LOCAL_AUTH with LOGIN_ADAPTER |
| `scripts/e2e/start-server.sh` | Update env var |
| `.env.example` | Replace ENABLE_LOCAL_AUTH with LOGIN_ADAPTER |
