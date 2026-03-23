# Self-Hosting Boopmark

Boopmark is a self-hostable bookmark manager. Here's how to get a server running.

## Quickest Path: Docker Compose

Prerequisites: Docker, [just](https://github.com/casey/just), openssl

```bash
git clone https://github.com/willisrocks/boopmark
cd boopmark
just bootstrap you@example.com --password yourpassword
```

This generates secrets, starts Postgres + the server, and creates your owner account. The server runs at `http://localhost:4000`.

## What `just bootstrap` does

1. Copies `.env.example` to `.env`
2. Generates random `SESSION_SECRET` and `LLM_SETTINGS_ENCRYPTION_KEY`
3. Starts Docker Compose (Postgres + server)
4. Waits for readiness
5. Creates your owner account

## After Setup

1. Open `http://localhost:4000` in your browser
2. Log in with the email/password you used in `just bootstrap`
3. Go to **Settings** → **API Keys** → **Generate New Key**
4. Use that key to configure the `boop` CLI:
   ```bash
   boop config set-server http://localhost:4000
   boop config set-key YOUR_API_KEY
   ```

## Adding More Users

Boopmark is invite-only. As the owner:
- Go to the **Admin** panel in the web UI
- Create invite links for your users
- Users claim invites and create their accounts

## Cloud Deployment (Railway + Neon)

1. Provision a [Neon](https://neon.tech) Postgres database
2. Fork the repo and connect to [Railway](https://railway.app)
3. Set env vars: `DATABASE_URL`, `SESSION_SECRET`, `LLM_SETTINGS_ENCRYPTION_KEY`, `APP_URL`
4. Deploy — the server auto-migrates on startup

## Configuration Reference

See `.env.example` in the repo for all available environment variables.
Key ones: `LOGIN_ADAPTER`, `STORAGE_BACKEND`, `SCREENSHOT_BACKEND`.
