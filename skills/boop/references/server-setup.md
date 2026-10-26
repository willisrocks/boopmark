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

## Cloud Deployment (Fly.io)

Production runs on Fly.io. The committed `fly.toml` targets app `boopmark`, internal port 4000, and a `/health` check; change `app` to your own name (or run `fly launch --no-deploy`) and keep the port and check.

1. Postgres: `fly mpg create` then `fly mpg attach <cluster-id> -a <your-app>` sets `DATABASE_URL`, or set your own with `fly secrets set DATABASE_URL='postgres://...'`
2. Required secrets (names only, generate the values yourself):
   ```bash
   fly secrets set \
     SESSION_SECRET="$(openssl rand -hex 32)" \
     LLM_SETTINGS_ENCRYPTION_KEY="$(openssl rand -base64 32)" \
     APP_URL="https://your-app.fly.dev"
   ```
3. Deploy: `fly deploy`, or run a published image with `fly deploy --image ghcr.io/willisrocks/boopmark:<version>`
4. Custom domain: `fly certs add your.domain`, create the DNS records it prints, then set `APP_URL` to the public URL
5. Uploads need S3 (`STORAGE_BACKEND=s3` + `S3_*`) — `STORAGE_BACKEND=local` writes to the machine's ephemeral disk
6. Create the first owner: `fly ssh console`, run `echo '<password>' | ./hash_password`, then insert the hash with `role = 'owner'` into `users` (use `fly mpg connect <cluster-id>`, or any client against `DATABASE_URL`)

See the README's [Fly.io section](../../../README.md#flyio) for the full walkthrough.

## Configuration Reference

See `.env.example` in the repo for all available environment variables.
Key ones: `LOGIN_ADAPTER`, `STORAGE_BACKEND`, `SCREENSHOT_BACKEND`.
