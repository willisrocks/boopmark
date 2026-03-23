# boop Setup Guide

## Installation Options

### Standard install (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/willisrocks/boopmark/main/install.sh | sh
```

Installs to `~/.local/bin/boop`. Make sure `~/.local/bin` is in your PATH.

### Specific version

```bash
curl -fsSL https://raw.githubusercontent.com/willisrocks/boopmark/main/install.sh | BOOP_VERSION=v0.7.1 sh
```

### Custom install directory

```bash
curl -fsSL https://raw.githubusercontent.com/willisrocks/boopmark/main/install.sh | BOOP_INSTALL_DIR=/usr/local/bin sh
```

### Upgrade an existing install

```bash
boop upgrade
```

## Connecting to a Server

The `boop` CLI needs two things: a server URL and an API key.

### Set the server URL

```bash
# Local development (default Docker Compose port)
boop config set-server http://localhost:4000

# Hosted instance with a custom domain
boop config set-server https://boopmark.yourdomain.com

# Railway or other cloud deployment
boop config set-server https://your-app.up.railway.app
```

### Get your API key

1. Open the Boopmark web app in your browser (the same URL you used for `set-server`)
2. Log in with your account
3. Go to **Settings** (gear icon or `/settings`)
4. Under **API Keys**, click **Generate New Key**
5. Copy the key and run:

```bash
boop config set-key YOUR_API_KEY
```

### Verify

```bash
boop config show    # Shows server URL and key status
boop list           # Should return your bookmarks (empty list is fine)
```

## macOS Gatekeeper Issues

If macOS kills the binary with no error message, it's Gatekeeper quarantine. Fix with:

```bash
xattr -cr $(which boop) && codesign --force --sign - $(which boop)
```

## All Commands

| Command | What it does |
|---------|-------------|
| `boop add <url>` | Add a bookmark |
| `boop add <url> --title "My Title"` | Add with a title |
| `boop add <url> --description "Summary"` | Add with a description |
| `boop add <url> --tags "a,b,c"` | Add with tags |
| `boop add <url> --suggest` | Add and ask server to suggest metadata |
| `boop list` | List all bookmarks (newest first) |
| `boop list --search "query"` | List matching a search query |
| `boop list --tags "tag1,tag2"` | List with specific tags |
| `boop list --sort oldest` | List sorted oldest first |
| `boop search <query>` | Search bookmarks |
| `boop edit <id> --title "New Title"` | Edit title |
| `boop edit <id> --description "Summary"` | Edit description |
| `boop edit <id> --tags "a,b,c"` | Edit tags |
| `boop edit <id> --suggest` | Ask server to suggest metadata |
| `boop suggest <url>` | Preview LLM suggestions without saving |
| `boop delete <id>` | Delete a bookmark by ID |
| `boop export --format jsonl` | Export as JSONL |
| `boop export --format csv` | Export as CSV |
| `boop import <file>` | Import from file |
| `boop upgrade` | Upgrade to latest version |
| `boop config set-server <url>` | Set server URL |
| `boop config set-key <key>` | Set API key |
| `boop config show` | Show current configuration |
