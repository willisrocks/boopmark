# Boop Skill CLI Upgrades Implementation Plan

> **For Codex:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Update the Claude Code `boop` skill so it reflects the current CLI command surface, including the recent LLM-suggestion flows, and add a regression guard so the skill cannot silently drift behind the CLI again.

**Architecture:** Treat the compiled Clap help output and `cli/src/main.rs` as the product contract, then mirror that contract into `skills/boop/SKILL.md`. Keep the regression in the existing `tests/test_install.sh` harness rather than adding a new docs test system; this is the idiomatic low-complexity fix for a small Rust CLI repo with a shell-based install/skill regression script.

**Tech Stack:** Rust CLI with Clap 4, Markdown skill docs, POSIX shell regression script, Cargo.

---

## Strategy Gate

The user did not ask for a broader doc refresh. The actual failure is narrower and more important: the `boop` skill still routes users toward an outdated CLI surface, even though the binary now supports `edit`, `suggest`, `upgrade`, `--description`, and `--suggest`. The plan should land the direct end state:

1. Prove what the live CLI exposes.
2. Make the skill advertise those commands where skill routing and example usage actually depend on them.
3. Expand the existing regression so future CLI/skill drift fails in CI or local verification.

Do not build a generated sync pipeline, do not add a second test harness, and do not rewrite the skill beyond what is needed to cover the current user-visible CLI. Those would be architectural noise for a documentation-drift fix.

### Task 1: Pin the missing command coverage in the existing shell regression

**Files:**
- Modify: `tests/test_install.sh`
- Reference: `skills/boop/SKILL.md`
- Reference: `cli/src/main.rs`

**Step 1: Verify the compiled CLI surface before changing any docs**

Run:

```bash
cargo run -q -p boop -- --help
cargo run -q -p boop -- help add
cargo run -q -p boop -- help edit
cargo run -q -p boop -- help suggest
cargo run -q -p boop -- help upgrade
```

Expected:
- top-level help lists `add`, `list`, `search`, `edit`, `suggest`, `delete`, `upgrade`, and `config`
- `add` help includes `--description` and `--suggest`
- `edit` help includes `--description` and `--suggest`
- `suggest` help shows `Usage: boop suggest <URL>`
- `upgrade` help shows `Usage: boop upgrade`

**Step 2: Make `assert_file_contains` safe for flag-like regex patterns**

In `tests/test_install.sh`, change the helper from:

```sh
if grep -Eq "$_pattern" "$_file"; then
```

to:

```sh
if grep -Eq -- "$_pattern" "$_file"; then
```

This is required so assertions for `--description` and `--suggest` are interpreted as patterns instead of `grep` options.

**Step 3: Extend Test 10 with the new command and flag assertions**

Immediately after the existing `boop add/list/search/delete/config` checks in `tests/test_install.sh`, add exactly these assertions:

```sh
assert_file_contains "$SKILL_MD" 'boop edit' "SKILL.md contains 'boop edit'"
assert_file_contains "$SKILL_MD" 'boop suggest' "SKILL.md contains 'boop suggest'"
assert_file_contains "$SKILL_MD" 'boop upgrade' "SKILL.md contains 'boop upgrade'"
assert_file_contains "$SKILL_MD" '--description' "SKILL.md contains '--description'"
assert_file_contains "$SKILL_MD" '--suggest' "SKILL.md contains '--suggest'"
assert_file_contains "$SKILL_MD" 'LLM' "SKILL.md mentions LLM usage"
```

Use `LLM` rather than a longer exact phrase so the test remains robust to small wording changes while still pinning the feature family that caused the drift.

**Step 4: Run the regression and verify it fails for the right reason**

Run:

```bash
tests/test_install.sh
```

Expected:
- the script no longer mis-parses `--description` and `--suggest`
- Test 10 fails because `skills/boop/SKILL.md` does not yet mention the newly asserted commands/flags
- install-script and Gatekeeper checks remain unchanged

**Step 5: Commit the failing-test characterization**

```bash
git add tests/test_install.sh
git commit -m "test: cover boop skill cli upgrades"
```

### Task 2: Update the Claude Code skill to match the live CLI

**Files:**
- Modify: `skills/boop/SKILL.md`
- Reference: `cli/src/main.rs`

**Step 1: Replace the frontmatter description with one that routes the new CLI surface**

Replace the existing `description:` value in `skills/boop/SKILL.md` with this exact line:

```yaml
description: This skill should be used when the user mentions "boop", "bookmarks", "boopmark", asks about managing bookmarks from the CLI, wants to add/list/search/edit/delete bookmarks, wants LLM suggestions for bookmark metadata, needs to configure or upgrade the boop CLI, or asks about "boop add", "boop list", "boop search", "boop edit", "boop suggest", "boop delete", "boop upgrade", or "boop config".
```

This solves the actual routing problem rather than only updating the visible examples.

**Step 2: Replace the command table with one that covers the current public surface**

Replace the entire `## Commands Reference` table in `skills/boop/SKILL.md` with this exact table:

```markdown
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
```

This is intentionally representative, not exhaustive. It covers every top-level command and every new flag-driven flow without turning the skill into a generated man page.

**Step 3: Replace the usage examples block so the new flows are discoverable**

Replace the current `## Usage Examples` content with this exact content:

````markdown
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
boop edit 123 --description "Updated summary"
```

Ask the server to suggest metadata for an existing bookmark:
```bash
boop edit 123 --suggest
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
````

Preserve the existing installation, setup, and macOS Gatekeeper sections unchanged.

**Step 4: Run the shell regression and verify the skill now satisfies the pinned contract**

Run:

```bash
tests/test_install.sh
```

Expected: Test 10 and Test 11 pass, along with the existing install-script checks.

**Step 5: Commit the skill update**

```bash
git add skills/boop/SKILL.md
git commit -m "docs: refresh boop skill cli coverage"
```

### Task 3: Verify the final state against the live CLI

**Files:**
- Verify: `skills/boop/SKILL.md`
- Verify: `tests/test_install.sh`
- Verify: `cli/src/main.rs`

**Step 1: Run the CLI test suite**

Run:

```bash
cargo test -p boop
```

Expected: PASS. This confirms the commands and flags documented in the skill still exist in the actual Clap parser and current CLI tests.

**Step 2: Re-run the shell regression**

Run:

```bash
tests/test_install.sh
```

Expected: PASS with no failures.

**Step 3: Re-run the CLI help spot-check**

Run:

```bash
cargo run -q -p boop -- --help
cargo run -q -p boop -- help add
cargo run -q -p boop -- help edit
cargo run -q -p boop -- help suggest
cargo run -q -p boop -- help upgrade
```

Expected:
- every newly documented top-level command appears in top-level help
- `add` and `edit` help still show `--description` and `--suggest`
- `suggest` and `upgrade` help still match the skill examples

**Step 4: Commit only if verification required follow-up cleanup**

If verification exposed small wording or assertion fixes, run:

```bash
git add skills/boop/SKILL.md tests/test_install.sh
git commit -m "chore: finalize boop skill sync"
```

If all verification is already clean, do not create an empty commit.

## Implementation Notes

- The most important doc surface is the skill frontmatter description plus the command/examples blocks. Updating only one of those would leave either routing drift or discoverability drift unresolved.
- `tests/test_install.sh` is already the repo’s guardrail for install-script and skill-document correctness. Extending it is the cleanest fix.
- Do not invent support for commands or flags not present in `cli/src/main.rs`.
- Do not touch install instructions, first-time setup, or Gatekeeper remediation except to preserve them while updating the CLI command coverage.
