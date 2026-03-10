# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- **Build all:** `cargo build`
- **Run all tests:** `cargo test`
- **Run server:** `cargo run -p boopmark-server`
- **Run CLI:** `cargo run -p boop`
- **Local dev stack:** `devproxy up` (NOT `docker compose up` — always use devproxy)
- **Run suggest E2E:** `npx playwright test tests/e2e/suggest.spec.js`

## Testing Notes

- The committed Playwright harness starts its own dedicated E2E server via `scripts/e2e/start-server.sh` on `http://127.0.0.1:4010`; do not point it at an already-running dev server on port `4000`.
- The E2E bootstrap script sets its own env inline, including `ENABLE_E2E_AUTH=1` and `STORAGE_BACKEND=local`, then waits for Postgres readiness before starting the server.
- Use Playwright MCP or agent-browser for ad-hoc verification against the same local server, but keep the committed regression in `tests/e2e/suggest.spec.js` as the source of truth.

## Local Auth (Development)

When Google OAuth isn't available (e.g. behind devproxy), enable local username/password login:

1. Set `ENABLE_LOCAL_AUTH=1` in `.env` (already set in `docker-compose.yml`)
2. Create a user: `just add-user email@example.com mypassword`
3. Sign in with the local form on the login page

This is for development only — do not use in production.

## Local HTTPS (devproxy)

**IMPORTANT:** Always use `devproxy up` instead of `docker compose up`. devproxy manages the Docker Compose stack and adds HTTPS proxying. Running `docker compose up` directly will conflict with devproxy-managed containers. Use `docker exec` on devproxy-managed containers (e.g. `docker exec <container> psql ...`) instead of expecting tools like `psql` to be installed locally.

This project uses [devproxy](https://github.com/foundra-build/devproxy) for local HTTPS dev subdomains.

```bash
devproxy up      # start and get an HTTPS URL
devproxy ls      # list running projects (* = current dir)
devproxy get-url # get current project's proxy URL
devproxy down    # stop this project
devproxy status  # check daemon health
```

URLs follow the format `https://{slug}-{app-name}.mysite.dev`.

Use the `/devproxy:url` skill to get the current proxy URL on demand.

## Worktree Setup

When using trycycle or git worktrees, copy the root `.env` file to the worktree before running:

```bash
cp /path/to/main/repo/.env /path/to/worktree/.env
```

This is required for Docker Compose services that depend on environment variables.

## Architecture

Boopmark is a full-stack bookmark management app using Rust with Axum, SQLx, HTMX, and Askama templates. Hexagonal (ports-and-adapters) architecture.

### Workspace Layout

- `server/` — Axum web server (`boopmark-server` crate)
- `cli/` — CLI client (`boop` crate)
- `docs/` — Documentation and plans

### Tech Stack

Rust, Axum 0.8, SQLx 0.8, Askama 0.12, HTMX 2, Tailwind CSS 4, clap 4, aws-sdk-s3, Docker Compose, Neon (Postgres).

<!-- workgraph-managed -->
# Workgraph

Use workgraph for task management.

**At the start of each session, run `wg quickstart` in your terminal to orient yourself.**
Use `wg service start` to dispatch work — do not manually claim tasks.

## For All Agents (Including the Orchestrating Agent)

CRITICAL: Do NOT use built-in TaskCreate/TaskUpdate/TaskList/TaskGet tools.
These are a separate system that does NOT interact with workgraph.
Always use `wg` CLI commands for all task management.

CRITICAL: Do NOT use the built-in **Task tool** (subagents). NEVER spawn Explore, Plan,
general-purpose, or any other subagent type. The Task tool creates processes outside
workgraph, which defeats the entire system. If you need research, exploration, or planning
done — create a `wg add` task and let the coordinator dispatch it.

ALL tasks — including research, exploration, and planning — should be workgraph tasks.

### Orchestrating agent role

The orchestrating agent (the one the user interacts with directly) does ONLY:
- **Conversation** with the user
- **Inspection** via `wg show`, `wg viz`, `wg list`, `wg status`, and reading files
- **Task creation** via `wg add` with descriptions, dependencies, and context
- **Monitoring** via `wg agents`, `wg service status`, `wg watch`

It NEVER writes code, implements features, or does research itself.
Everything gets dispatched through `wg add` and `wg service start`.
