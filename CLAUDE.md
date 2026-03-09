# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- **Build all:** `cargo build`
- **Run all tests:** `cargo test`
- **Run server:** `cargo run -p boopmark-server`
- **Run CLI:** `cargo run -p boop`
- **Local dev stack:** `docker compose up`

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
