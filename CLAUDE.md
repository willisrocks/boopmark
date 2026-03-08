# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- **Dev server:** `npm run dev` (runs on port 4000)
- **Typecheck:** `npm run typecheck` (runs react-router typegen + tsc)
- **Run all tests:** `npx vitest run`
- **Run single test:** `npx vitest run path/to/test.ts`
- **Run tests in watch:** `npx vitest`

## Architecture

Boopmark is a full-stack bookmark management app using React Router 7 with SSR, Hono as the server framework, and Neon (serverless PostgreSQL) as the database.

### Routing (file-based)

Routes are auto-discovered from `src/app/` using a Next.js-like convention:
- **Pages:** `src/app/**/page.jsx` → UI routes
- **API routes:** `src/app/api/**/route.js` → Hono handlers exporting HTTP methods (GET, POST, PUT, DELETE, PATCH)
- **Layouts:** `src/app/**/layout.jsx` → hierarchical layout wrappers (composed by the `plugins/layouts.ts` Vite plugin)
- **Dynamic segments:** `[id]` for params, `[[id]]` for optional, `[...slug]` for catch-all

Route discovery and registration happens in `__create/route-builder.ts`. The `src/app/routes.ts` generates the route config for React Router.

### Server

- `__create/index.ts` — main Hono app setup (entry point)
- `__create/adapter.ts` — Neon database adapter for Auth.js
- API routes use Hono's `c` context object for request/response

### Auth

Auth.js with `@hono/auth-js` integration. Configured in `src/auth.js`.
- Providers: Credentials (email/password with argon2) and Google OAuth
- JWT session strategy
- Auth tables: `auth_users`, `auth_accounts`, `auth_sessions`, `auth_verification_token`

### Client State

- **Server state:** TanStack React Query
- **Client state:** Zustand
- **Forms:** React Hook Form + Yup validation

### Key Conventions

- Path alias: `@/` maps to `src/`
- `src/utils/` — client-side hooks (useAuth, useUser, useUpload)
- `src/app/api/utils/sql.js` — Neon SQL query wrapper
- `src/client-integrations/` — lazy-loaded library wrappers (Chakra UI, Recharts, etc.)
- `plugins/` — custom Vite plugins (layouts, aliases, font loading, env injection)
- `__create/` — server-side infrastructure (not a typo — this is the server entry directory)

### UI Stack

Tailwind CSS 3 + Chakra UI 2. Icons from Lucide React. Toasts via Sonner. Animations via Motion.
