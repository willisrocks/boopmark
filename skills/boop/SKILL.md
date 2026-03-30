---
name: boop
description: Use this skill when the user mentions "boop", "bookmarks", "boopmark", wants to save/find/manage bookmarks, asks about the boop CLI, or wants to add/list/search/edit/delete/suggest/export/import bookmarks. Also use when the user asks about installing or configuring boop, getting an API key, or connecting to a Boopmark server. Trigger phrases include "boop add", "boop list", "boop search", "save this link", "bookmark this", "find my bookmarks", or any bookmark management task.
---

# boop CLI

Command-line bookmark manager for [Boopmark](https://github.com/willisrocks/boopmark). Works for both humans at the terminal and AI agents with shell access.

## Quick Reference

| Command | What it does |
|---------|-------------|
| `boop add <url>` | Save a bookmark |
| `boop add <url> --suggest` | Save with AI-suggested title, description, and tags |
| `boop add <url> --title "X" --description "Y" --tags "a,b"` | Save with explicit metadata |
| `boop search <query>` | Search bookmarks |
| `boop list` | List recent bookmarks |
| `boop list --tags "rust,tools"` | Filter by tags |
| `boop edit <id> --suggest` | LLM-suggest metadata for existing bookmark (pass a bookmark-uuid) |
| `boop edit <id> --description "Y"` | Update bookmark description |
| `boop suggest <url>` | Preview AI suggestions without saving |
| `boop delete <id>` | Delete a bookmark |
| `boop export --format jsonl` | Export bookmarks |
| `boop import <file>` | Import bookmarks |
| `boop upgrade` | Upgrade to latest version |
| `boop config show` | Show current configuration |

## Setup (if not already configured)

If `boop` isn't installed or configured, follow these steps:

**1. Install the binary:**
```bash
curl -fsSL https://raw.githubusercontent.com/willisrocks/boopmark/main/install.sh | sh
```

**2. Point it at your Boopmark server:**
```bash
# Local development server
boop config set-server http://localhost:4000

# Or your hosted instance
boop config set-server https://boopmark.yourdomain.com
```

**3. Get your API key from the web app:**

Log in to the Boopmark web app in your browser, go to **Settings**, and generate an API key. Then:
```bash
boop config set-key YOUR_API_KEY
```

**4. Verify it works:**
```bash
boop config show
boop list
```

> **No Boopmark server yet?** See `references/server-setup.md` for how to self-host one.

## Common Patterns

**Save a link you found useful:**
```bash
boop add https://example.com/article --tags "rust,async" --suggest
```

**Find something you saved before:**
```bash
boop search "rust error handling"
```

**Bulk export for backup:**
```bash
boop export --format jsonl > bookmarks.jsonl
```

## Troubleshooting

| Problem | Fix |
|---------|-----|
| "Server URL not configured" | `boop config set-server <url>` |
| "API key not configured" | Log in to Boopmark web UI → Settings → generate API key → `boop config set-key <key>` |
| Binary "killed" on macOS | Gatekeeper quarantine: `xattr -cr $(which boop) && codesign --force --sign - $(which boop)` |
| `boop: command not found` | Add `~/.local/bin` to PATH: `export PATH="$HOME/.local/bin:$PATH"` |

For detailed installation options, version pinning, and server setup, see `references/setup-guide.md`.
