# Production Deployment Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy boopmark to production using Neon (Postgres), Railway (app), and Cloudflare (DNS/CDN/R2) with a single `just deploy` command.

**Architecture:** Railway hosts the Docker container, Neon provides managed Postgres, Cloudflare proxies traffic (CDN/SSL) and provides R2 object storage. `just deploy` builds CSS locally then runs `railway up` to ship source to Railway.

**Tech Stack:** Railway CLI, Neon (Postgres), Cloudflare (DNS, R2), just, Docker

**Spec:** `docs/superpowers/specs/2026-03-17-prod-deployment-design.md`

---

## Chunk 1: Prerequisites & Service Setup

### Task 1: Install Railway CLI

**Files:** None

- [ ] **Step 1: Install Railway CLI via Homebrew**

```bash
brew install railway
```

- [ ] **Step 2: Authenticate**

```bash
railway login
```

This opens a browser for OAuth. Follow the prompts.

- [ ] **Step 3: Verify installation**

```bash
railway --version
```

Expected: version string printed.

---

### Task 2: Create Neon Postgres Database

**Files:** None (dashboard setup)

- [ ] **Step 1: Create Neon account and project**

Go to https://neon.tech — sign up and create a new project:
- Project name: `boopmark`
- Postgres version: 16
- Region: US East (closest to Railway's us-east)

- [ ] **Step 2: Copy the connection string**

From the Neon dashboard, copy the connection string. It looks like:
```
postgresql://neondb_owner:<password>@<host>.neon.tech/neondb?sslmode=require
```

Save this — it becomes the `DATABASE_URL` env var in Railway.

- [ ] **Step 3: Verify connection**

```bash
psql "<connection-string>"
```

Expected: connected to Neon Postgres. Type `\q` to quit.

---

### Task 3: Set Up Cloudflare Site & DNS

**Files:** None (dashboard + Namecheap setup)

- [ ] **Step 1: Create Cloudflare account and add site**

Go to https://dash.cloudflare.com — sign up, then "Add a site" → enter `boopmark.com` → select the Free plan.

- [ ] **Step 2: Note Cloudflare nameservers**

Cloudflare will show two nameservers, e.g.:
```
anna.ns.cloudflare.com
bob.ns.cloudflare.com
```

- [ ] **Step 3: Update Namecheap nameservers**

In Namecheap dashboard → Domain List → boopmark.com → Nameservers → "Custom DNS" → enter the two Cloudflare nameservers.

- [ ] **Step 4: Wait for propagation and verify**

Back in Cloudflare dashboard, click "Check nameservers." This can take minutes to hours. Cloudflare will email you when active.

Verify:
```bash
dig NS boopmark.com +short
```

Expected: Cloudflare nameservers returned.

---

### Task 4: Create Cloudflare R2 Buckets

**Files:** None (dashboard setup)

- [ ] **Step 1: Enable R2 in Cloudflare dashboard**

Cloudflare dashboard → R2 Object Storage → might require adding a payment method (R2 has a generous free tier: 10GB storage, 10M reads/month).

- [ ] **Step 2: Create uploads bucket**

Create bucket: `boopmark-uploads`
- Location: Automatic (or US East)

- [ ] **Step 3: Create images bucket**

Create bucket: `boopmark-images`
- Location: Automatic (or US East)

- [ ] **Step 4: Create R2 API token**

R2 → Manage R2 API Tokens → Create API Token:
- Token name: `boopmark-server`
- Permissions: Object Read & Write
- Scope: Apply to specific buckets → `boopmark-uploads` and `boopmark-images`

Save the Access Key ID and Secret Access Key — these become `S3_ACCESS_KEY` and `S3_SECRET_KEY`.

- [ ] **Step 5: Note the S3 endpoint**

The R2 S3-compatible endpoint is:
```
https://<account-id>.r2.cloudflarestorage.com
```

Find your account ID in the Cloudflare dashboard URL or R2 overview page.

---

## Chunk 2: Railway Setup & Codebase Changes

### Task 5: Create Railway Project and Configure

**Files:** None (CLI + dashboard)

- [ ] **Step 1: Create Railway project**

```bash
cd /Users/chrisfenton/Code/personal/boopmark
railway init
```

When prompted, name the project `boopmark`.

- [ ] **Step 2: Set all environment variables**

Note: Railway CLI syntax may vary by version. Run `railway variables --help` to confirm. The `--set` flag is used in v3+:

```bash
railway variables --set "DATABASE_URL=<neon-connection-string>"
railway variables --set "SESSION_SECRET=$(openssl rand -hex 32)"
railway variables --set "GOOGLE_CLIENT_ID=<your-google-client-id>"
railway variables --set "GOOGLE_CLIENT_SECRET=<your-google-client-secret>"
railway variables --set "LLM_SETTINGS_ENCRYPTION_KEY=<your-base64-key>"
railway variables --set "APP_URL=https://boopmark.com"
railway variables --set "STORAGE_BACKEND=s3"
railway variables --set "S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com"
railway variables --set "S3_BUCKET=boopmark-uploads"
railway variables --set "S3_ACCESS_KEY=<r2-access-key>"
railway variables --set "S3_SECRET_KEY=<r2-secret-key>"
railway variables --set "S3_REGION=auto"
railway variables --set "S3_IMAGES_BUCKET=boopmark-images"
railway variables --set "PORT=4000"
```

Note: Get `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, and `LLM_SETTINGS_ENCRYPTION_KEY` from your local `.env` file.

- [ ] **Step 3: Verify variables are set**

```bash
railway variables
```

Expected: all 14 variables listed.

---

### Task 6: Create .dockerignore

**Files:**
- Create: `.dockerignore`

Without a `.dockerignore`, `railway up` uploads the entire working directory including `target/` (potentially gigabytes of Rust build artifacts). This makes deploys extremely slow or may cause timeouts.

- [ ] **Step 1: Create .dockerignore**

```
target/
node_modules/
.git/
uploads/
playwright-report/
test-results/
.env
.env.*
```

- [ ] **Step 2: Verify it works**

```bash
docker build --no-cache -t boopmark-test . 2>&1 | head -5
```

Expected: build starts without sending a multi-GB context.

- [ ] **Step 3: Commit**

```bash
git add .dockerignore
git commit -m "chore: add .dockerignore to speed up Railway deploys"
```

---

### Task 7: Update Justfile

**Files:**
- Modify: `justfile:56-58`

- [ ] **Step 1: Replace deploy recipe**

Change the `deploy` recipe from:
```just
deploy:
    just css-build
    fly deploy
```

To:
```just
deploy:
    just css-build
    railway up
```

- [ ] **Step 2: Verify justfile syntax**

```bash
just --list
```

Expected: `deploy` listed among available recipes.

- [ ] **Step 3: Commit**

```bash
git add justfile
git commit -m "chore: switch deploy from Fly.io to Railway"
```

---

### Task 8: Remove Fly.io Config

**Files:**
- Delete: `fly.toml`

- [ ] **Step 1: Delete fly.toml**

```bash
rm fly.toml
```

- [ ] **Step 2: Commit**

```bash
git add -u fly.toml
git commit -m "chore: remove fly.toml (deploying via Railway)"
```

---

## Chunk 3: Deploy & Verify

### Task 9: First Deploy

**Files:** None

- [ ] **Step 1: Run the deploy**

```bash
just deploy
```

This builds CSS, then `railway up` uploads the source. Railway builds using the Dockerfile. Watch the build logs for errors.

Expected: Build succeeds, service starts, migrations run on startup.

- [ ] **Step 2: Get the Railway public URL**

Generate a Railway domain. CLI syntax varies by version — check `railway domain --help`:
```bash
railway domain
```

If that doesn't work, try the Railway dashboard → Settings → Networking → Generate Domain.

This gives a URL like `boopmark-production.up.railway.app`.

- [ ] **Step 3: Verify the app is running**

```bash
curl -s https://<railway-domain>/health
```

Expected: 200 OK (or whatever your health endpoint returns).

- [ ] **Step 4: Check Railway logs for migration output**

```bash
railway logs
```

Expected: logs show migrations running and server listening on port 4000.

---

### Task 10: Configure Custom Domain on Railway

**Files:** None (CLI + Cloudflare dashboard)

- [ ] **Step 1: Add custom domain in Railway**

Add via CLI or dashboard. CLI syntax varies by version — check `railway domain --help`:
```bash
railway domain add boopmark.com
```

Alternatively: Railway dashboard → Settings → Networking → Custom Domain → `boopmark.com`.

Railway will show you the CNAME target (e.g., `boopmark-production.up.railway.app`).

- [ ] **Step 2: Add DNS records in Cloudflare**

In Cloudflare dashboard → DNS → Records:

1. Add CNAME record:
   - Name: `@`
   - Target: `<railway-cname-target>`
   - Proxy status: Proxied (orange cloud)

2. Add CNAME record:
   - Name: `www`
   - Target: `boopmark.com`
   - Proxy status: Proxied (orange cloud)

- [ ] **Step 3: Configure Cloudflare SSL mode**

Cloudflare dashboard → SSL/TLS → Overview → Set mode to **Full (strict)**.

This ensures Cloudflare verifies Railway's SSL certificate on the backend connection.

- [ ] **Step 4: Wait for DNS propagation and verify**

```bash
dig CNAME boopmark.com +short
curl -s https://boopmark.com/health
```

Expected: CNAME resolves, health check returns 200.

---

### Task 11: Update Google OAuth Redirect URI

**Files:** None (Google Cloud Console)

- [ ] **Step 1: Add production redirect URI**

In Google Cloud Console → APIs & Services → Credentials → your OAuth client:

Add authorized redirect URI:
```
https://boopmark.com/auth/google/callback
```

- [ ] **Step 2: Add authorized JavaScript origin**

Add:
```
https://boopmark.com
```

---

### Task 12: End-to-End Verification

**Files:** None

- [ ] **Step 1: Verify homepage loads**

Open `https://boopmark.com` in a browser. The login page should render.

- [ ] **Step 2: Verify Google OAuth flow**

Click "Sign in with Google" and complete the OAuth flow. Should redirect back to the app.

- [ ] **Step 3: Verify bookmark creation with image upload**

Create a bookmark with an image. Verify the image uploads to R2 (check R2 dashboard for objects in `boopmark-uploads` and `boopmark-images`).

- [ ] **Step 4: Verify the CLI works against production**

```bash
boop --api-url https://boopmark.com <some-command>
```

Expected: CLI communicates with the production server.

- [ ] **Step 5: Final commit**

If any tweaks were needed during verification, commit them:
```bash
git add -A
git commit -m "chore: finalize production deployment config"
```
