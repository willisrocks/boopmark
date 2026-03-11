---
name: boop
description: This skill should be used when the user mentions "boop", "bookmarks", "boopmark", asks about managing bookmarks from the CLI, wants to add/list/search/edit/delete bookmarks, wants LLM suggestions for bookmark metadata, needs to configure or upgrade the boop CLI, or asks about "boop add", "boop list", "boop search", "boop edit", "boop suggest", "boop delete", "boop upgrade", or "boop config".
---

# boop CLI

Command-line interface for managing bookmarks on Boopmark. Single Rust binary.

**Prerequisites:** The boop CLI must be installed. See the install section below.

## Commands Reference

| Command                                    | What it does                                           |
|--------------------------------------------|--------------------------------------------------------|
| `boop add <url>`                           | Add a bookmark                                         |
| `boop add <url> --title "My Title"`        | Add a bookmark with a title                            |
| `boop add <url> --description "Summary"`   | Add a bookmark with a description                      |
| `boop add <url> --tags "a,b,c"`            | Add a bookmark with tags                               |
| `boop add <url> --suggest`                 | Add a bookmark and ask the server to suggest metadata  |
| `boop list`                                | List all bookmarks (newest first)                      |
| `boop list --search "query"`               | List bookmarks matching a search query                 |
| `boop list --tags "tag1,tag2"`             | List bookmarks with specific tags                      |
| `boop list --sort oldest`                  | List bookmarks sorted oldest first                     |
| `boop search <query>`                      | Search bookmarks                                       |
| `boop edit <id> --title "New Title"`       | Edit an existing bookmark title                        |
| `boop edit <id> --description "Summary"`   | Edit an existing bookmark description                  |
| `boop edit <id> --tags "a,b,c"`            | Edit an existing bookmark's tags                       |
| `boop edit <id> --suggest`                 | Ask the server to suggest title, description, and tags |
| `boop suggest <url>`                       | Preview LLM suggestions without saving                 |
| `boop delete <id>`                         | Delete a bookmark by ID                                |
| `boop upgrade`                             | Upgrade `boop` to the latest version                   |
| `boop config set-server <url>`             | Set the Boopmark server URL                            |
| `boop config set-key <key>`                | Set your API key                                       |
| `boop config show`                         | Show current configuration                             |

## Installation

Install via the install script:

```bash
curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | sh
```

Install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | BOOP_VERSION=v0.1.0 sh
```

Custom install directory:

```bash
curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | BOOP_INSTALL_DIR=/usr/local/bin sh
```

Verify installation:

```bash
boop --help
```

## First-Time Setup

After installing, configure the CLI to connect to your Boopmark server:

```bash
boop config set-server https://your-boopmark-instance.example.com
boop config set-key YOUR_API_KEY
```

Verify the configuration:

```bash
boop config show
```

## Usage Examples

Add a bookmark with explicit metadata:
```bash
boop add https://example.com --title "Example Site" --description "Reference page" --tags "reference,docs"
```

Add a bookmark and let the server suggest missing metadata:
```bash
boop add https://example.com --suggest
```

Edit an existing bookmark description:
```bash
boop edit <bookmark-uuid> --description "Updated summary"
```

Ask the server to suggest metadata for an existing bookmark:
```bash
boop edit <bookmark-uuid> --suggest
```

Preview suggestions without saving:
```bash
boop suggest https://example.com
```

Search bookmarks:
```bash
boop search "rust async"
```

List recent bookmarks with tag filter:
```bash
boop list --tags "rust" --sort newest
```

Upgrade the CLI:
```bash
boop upgrade
```

## Common Issues

| Problem | Fix |
|---------|-----|
| "Server URL not configured" | Run `boop config set-server <url>` |
| "API key not configured" | Run `boop config set-key <key>` — generate a key in the Boopmark web UI settings |
| Binary "killed" on macOS | Gatekeeper quarantine. Run: `xattr -cr $(which boop) && codesign --force --sign - $(which boop)` |
| `boop: command not found` | `~/.local/bin` may not be in your PATH. Add: `export PATH="$HOME/.local/bin:$PATH"` to your shell profile |
