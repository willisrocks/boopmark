# Set Up devproxy for Local HTTPS Dev Subdomains — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Add local HTTPS dev subdomains via devproxy so the app is accessible over HTTPS during development.

**Architecture:** The app server (`boopmark-server`) already has a Dockerfile and listens on port 4000. We need to add it as a service in `docker-compose.yml` with the `devproxy.port` label, then document the setup. The server needs DATABASE_URL, SESSION_SECRET, and other env vars from `.env`, which we pass via `env_file`.

**Tech Stack:** Docker Compose, devproxy, Rust/Axum (existing), Playwright MCP (verification)

---

### Task 1: Add the app server to docker-compose.yml

**Files:**
- Modify: `docker-compose.yml`

**Step 1: Add the `server` service with devproxy label**

Add a `server` service that builds from the existing `Dockerfile`, depends on `db`, passes env vars via `env_file`, and has the `devproxy.port` label. Override `DATABASE_URL` so it points to the `db` container hostname instead of `localhost`.

```yaml
  server:
    build: .
    env_file: .env
    environment:
      DATABASE_URL: postgres://boopmark:devpassword@db:5432/boopmark
    ports:
      - "4000:4000"
    labels:
      - "devproxy.port=4000"
    depends_on:
      - db
    volumes:
      - ./uploads:/app/uploads
```

Key points:
- `env_file: .env` loads all env vars from the root `.env` file
- The `DATABASE_URL` override uses `db` (the compose service name) as hostname and port `5432` (the container-internal port, not the host-mapped `5434`)
- `devproxy.port=4000` tells devproxy which container port to proxy
- The `uploads` volume mount ensures local storage works inside the container

**Step 2: Verify the compose file is valid**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy && docker compose config --quiet`
Expected: No output (success)

**Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "feat: add server service to docker-compose with devproxy label"
```

---

### Task 2: Add `.devproxy-override.yml` to `.gitignore`

**Files:**
- Modify: `.gitignore`

**Step 1: Append the entry**

Add `.devproxy-override.yml` to the end of `.gitignore`:

```
.devproxy-override.yml
```

**Step 2: Verify**

Run: `grep devproxy .gitignore`
Expected: `.devproxy-override.yml`

**Step 3: Commit**

```bash
git add .gitignore
git commit -m "chore: add .devproxy-override.yml to .gitignore"
```

---

### Task 3: Update CLAUDE.md with devproxy and worktree sections

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add the devproxy section**

After the "Testing Notes" section and before the "Architecture" section, add:

```markdown
## Local HTTPS (devproxy)

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
```

**Step 2: Add the worktree setup section**

After the devproxy section, add:

```markdown
## Worktree Setup

When using trycycle or git worktrees, copy the root `.env` file to the worktree before running:

```bash
cp /path/to/main/repo/.env /path/to/worktree/.env
```

This is required for Docker Compose services that depend on environment variables.
```

**Step 3: Verify CLAUDE.md is valid markdown**

Read through the file to confirm formatting is correct.

**Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add devproxy and worktree setup sections to CLAUDE.md"
```

---

### Task 4: Create README.md with project overview and devproxy section

**Files:**
- Create: `README.md`

**Step 1: Create README.md**

```markdown
# Boopmark

A full-stack bookmark management app built with Rust (Axum), HTMX, and Tailwind CSS.

## Getting Started

1. Copy `.env.example` to `.env` and fill in values
2. Run `docker compose up -d` to start Postgres and MinIO
3. Run `cargo run -p boopmark-server` to start the dev server
4. Open `http://localhost:4000`

### Local HTTPS Dev URLs

This project supports local HTTPS subdomains via [devproxy](https://github.com/foundra-build/devproxy):

1. Install devproxy: `curl -fsSL https://raw.githubusercontent.com/foundra-build/devproxy/main/install.sh | sh`
2. Run `devproxy init` (one-time setup — see devproxy docs for DNS configuration)
3. Start the project: `devproxy up`
4. Open the printed HTTPS URL

Run `devproxy ls` to see all running projects (`*` marks the current directory).
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add README.md with getting started and devproxy sections"
```

---

### Task 5: E2E verification with devproxy and agent-browser

This task verifies the entire setup works end-to-end. It is a manual verification task using devproxy CLI and Playwright MCP.

**Step 1: Ensure .env exists in the worktree**

Run: `test -f /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy/.env && echo "exists" || echo "MISSING"`

If missing, copy it:
```bash
cp /Users/chrisfenton/Code/personal/boopmark/.env /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy/.env
```

**Step 2: Start devproxy**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy && devproxy up`
Expected: Prints an HTTPS URL like `https://xxxx-boopmark.mysite.dev`
Capture this URL for the next steps.

**Step 3: Verify devproxy ls shows current project**

Run: `devproxy ls`
Expected: Output contains a line with `*` indicating the current project

**Step 4: Verify devproxy get-url returns the URL**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy && devproxy get-url`
Expected: Returns the HTTPS URL from step 2

**Step 5: Use Playwright MCP to navigate to the HTTPS URL**

Use `mcp__playwright__browser_navigate` to open the URL from step 2.
Then use `mcp__playwright__browser_snapshot` to verify the page loads with content.

Expected: The page loads successfully showing the Boopmark login or home page.

**Step 6: Stop devproxy**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy && devproxy down`
Expected: Clean shutdown

**Step 7: Verify get-url returns empty after shutdown**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/setup-devproxy && devproxy get-url`
Expected: Empty string (no output)
