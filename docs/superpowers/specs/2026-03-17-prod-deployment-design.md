# Production Deployment Design

Deploy boopmark to production via `just deploy` using Neon (Postgres), Railway (app hosting), and Cloudflare (DNS/CDN/R2 storage).

## Services & Responsibilities

| Service | Role | What we configure |
|---------|------|-------------------|
| Neon | Postgres database | Create project, get connection string |
| Railway | App hosting (Docker) | Project + service, env vars, custom domain |
| Cloudflare | DNS, CDN/SSL, R2 storage | Add site, DNS records, R2 bucket + API keys |
| Namecheap | Domain registrar | Point nameservers to Cloudflare |

## Traffic Flow

```
User → boopmark.com → Cloudflare (proxy/CDN/SSL) → Railway (app:4000)
                       Cloudflare R2 ← app (S3 uploads/images)
                       Neon Postgres ← app (DATABASE_URL)
```

Cloudflare proxies all traffic (orange cloud), providing SSL termination and CDN caching. Railway provides its own HTTPS, but Cloudflare sits in front.

## Environment Variables (Railway)

```
DATABASE_URL=<neon connection string with ?sslmode=require>
SESSION_SECRET=<generated 64-char hex>
GOOGLE_CLIENT_ID=<existing>
GOOGLE_CLIENT_SECRET=<existing>
LLM_SETTINGS_ENCRYPTION_KEY=<existing base64 key>
APP_URL=https://boopmark.com
STORAGE_BACKEND=s3
S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
S3_BUCKET=boopmark-uploads
S3_ACCESS_KEY=<R2 API token>
S3_SECRET_KEY=<R2 API secret>
S3_REGION=auto
S3_IMAGES_BUCKET=boopmark-images
```

## `just deploy` Command

Replaces the current `fly deploy`:

```just
deploy:
    just css-build
    railway up
```

CSS built locally (fresh Tailwind output), then `railway up` ships source to Railway which builds via the existing Dockerfile.

## Cloudflare DNS Records

| Type | Name | Value | Proxy |
|------|------|-------|-------|
| CNAME | `@` | Railway-provided domain | Proxied |
| CNAME | `www` | `boopmark.com` | Proxied |

## R2 Storage

- Two buckets: `boopmark-uploads` and `boopmark-images`
- API token scoped to R2 read/write
- Public access via R2 custom domain or signed URLs

## Codebase Changes

1. **justfile** — replace `deploy` recipe (`railway up` instead of `fly deploy`)
2. **fly.toml** — delete (not using Fly)
3. **Dockerfile** — keep as-is (Railway uses it)
4. **No code changes** — app already supports S3 backend and env-based config

## Design Decisions

- **Auto-migrate on startup**: Migrations run via `sqlx::migrate!()` in `main.rs`. No separate migration step needed for a personal project.
- **Cloudflare proxy mode**: SSL termination at Cloudflare edge, CDN caching for static assets.
- **R2 over Railway volumes**: S3-compatible, fits the Cloudflare stack, generous free tier, app already has S3 backend.
- **Source deploy via `railway up`**: Simplest workflow. Railway builds using existing Dockerfile. No local Docker build or registry needed.
