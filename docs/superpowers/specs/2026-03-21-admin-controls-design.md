# Admin Controls Design

Invite-only access, user roles, admin settings page, and login provider abstraction.

## Goals

- Limit access to invited users only
- Role-based permissions: owner, admin, user
- Admin page for managing invites and users
- Login provider abstraction (port/adapter) for swappable auth backends
- Google-only social login in prod, local password in dev, WorkOS in the future

## Login Provider Architecture

### Port

`LoginProvider` trait selected via `LOGIN_ADAPTER` env var. Replaces the current `ENABLE_LOCAL_AUTH` flag. Only one adapter is active at a time — this is intentional (prod = Google only, dev = local password only).

`ENABLE_E2E_AUTH` remains a separate flag — the E2E test login bypasses the adapter pattern and invite checks entirely, creating/logging in a test user directly.

```rust
trait LoginProvider: Send + Sync {
    /// Return the router fragment with this adapter's auth routes
    fn routes(&self) -> Router<AppState>;

    /// Return the template context for rendering the login page
    fn login_page_context(&self) -> LoginPageContext;
}

struct AuthenticatedIdentity {
    email: String,
    name: Option<String>,
    image: Option<String>,
}
```

Each adapter owns its routes (initiate, callback) and renders the appropriate login UI. The callback handler resolves an `AuthenticatedIdentity`, then shared logic handles the invite check and user creation.

### Shared callback logic (above the adapter)

1. Adapter resolves `AuthenticatedIdentity` from OAuth callback / form post
2. Check if user exists by email (`find_by_email`, not `upsert`)
3. If user exists → create session, login (no invite needed)
4. If user does not exist → check session for invite token
5. If valid invite token → create user with `role = 'user'`, consume invite, create session
6. If no invite token → show "BoopMark is invite-only" page (no account created)

### Adapters

| Adapter | Env value | Description |
|---|---|---|
| `GoogleOAuthAdapter` | `google` | Current Google OAuth flow, extracted as-is |
| `LocalPasswordAdapter` | `local_password` | Current Argon2 flow, extracted as-is |
| `WorkOSAdapter` | `workos` | Future — drop-in replacement for Google |

### Config

```
LOGIN_ADAPTER=google|local_password    # which adapter to use (one at a time)
# Google: GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET (unchanged)
# Future WorkOS: WORKOS_CLIENT_ID, WORKOS_API_KEY
```

## Roles

Postgres enum `user_role` with values: `owner`, `admin`, `user`.

Added as a column on the `users` table with `DEFAULT 'user'`. Owner is set manually in the database — no automatic promotion, no CLI command, no env var. On a fresh deployment, the first user signs up as `user` and must be promoted via SQL.

The `just add-user` command continues to create users with the default `user` role. Set role manually after if needed.

### Permissions

| Capability | Owner | Admin | User |
|---|---|---|---|
| Manage own bookmarks/settings | yes | yes | yes |
| Create/view/revoke invite links | yes | yes | no |
| Manage users (view, deactivate) | yes | yes | no |
| Change user roles | yes | admin→user only | no |
| Promote user to admin | yes | no | no |
| Access /admin page | yes | yes | no |

"admin→user only" means an admin can demote another admin to user, but cannot promote a user to admin. Only the owner can promote.

## Invite System

### Flow

1. Admin creates invite on `/admin` page (optional email for tracking)
2. System generates single-use token (32-char alphanumeric, same as session tokens), creates invite record, returns link
3. Link URL: `/invite/{token}`
4. Invitee clicks link → landing page: "You're invited to BoopMark" with sign-in button
5. Token stored in session cookie
6. Invitee completes login adapter flow (Google OAuth / password)
7. Shared callback logic checks: user exists? invite token in session? (see above)

### Invite properties

- Single-use: one token = one signup
- 7-day expiry from creation
- Revocable by admin/owner while pending (sets `revoked_at` timestamp)
- Optional email field for tracking (not enforced — any person with the link can use it)
- Statuses (derived, not stored): pending (`claimed_by IS NULL AND revoked_at IS NULL AND expires_at > now()`), claimed (`claimed_by IS NOT NULL`), revoked (`revoked_at IS NOT NULL`), expired (`expires_at <= now()`)

## Database Changes

### Migration: Create user_role enum, add role and deactivated_at to users

```sql
CREATE TYPE user_role AS ENUM ('owner', 'admin', 'user');
ALTER TABLE users ADD COLUMN role user_role NOT NULL DEFAULT 'user';
ALTER TABLE users ADD COLUMN deactivated_at TIMESTAMPTZ;
```

### Migration: Create invites table

```sql
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

No separate index on `token` — the UNIQUE constraint already creates one.

### Domain port

```rust
trait InviteRepository: Send + Sync {
    async fn create(&self, invite: &CreateInvite) -> Result<Invite>;
    async fn find_by_token(&self, token: &str) -> Result<Option<Invite>>;
    async fn claim(&self, token: &str, user_id: Uuid) -> Result<()>;
    async fn revoke(&self, invite_id: Uuid) -> Result<()>;
    async fn list_all(&self) -> Result<Vec<Invite>>;
}
```

## Pages

### Admin page (`/admin`)

- Accessible to owner and admin roles only (return 403 for others)
- Two sections: Invites and Users
- HTMX for all interactions (create invite, revoke, change role, deactivate)
- Invite list shows: link/email, created by, status (pending/claimed/revoked/expired), revoke action on pending
- User list shows: avatar + name/email, role dropdown, join date, deactivate action
- Owner row has no controls — role managed in DB only
- Copy-to-clipboard on invite links

### Invite landing page (`/invite/{token}`)

- Valid token: "You're invited to BoopMark" + sign-in button (styled with primary blue)
- Invalid/expired/revoked token: "Invite not valid" + guidance to ask for a new link

### "You need an invite" page

- Shown post-login when user doesn't exist and no invite token in session
- "BoopMark is invite-only" message

### Login page changes

- Renders based on `LOGIN_ADAPTER`: Google-only button or email/password form
- Sign in with Google button uses primary blue background (not white)

### Nav changes

- Profile dropdown gets "Admin" link, visible only to owner/admin roles

## Deactivation

`deactivated_at TIMESTAMPTZ` column on users table — NULL means active, non-NULL means deactivated. Deactivated users cannot create sessions (checked at login and session validation). Existing sessions for deactivated users are invalidated.
