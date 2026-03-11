# Boop Skill CLI Upgrades Implementation Plan

> **For Codex:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Update the Claude Code `boop` skill so it accurately documents the current CLI surface, including the recent LLM-suggestion and upgrade commands, and add regression coverage so future CLI/skill drift fails automatically.

**Architecture:** Use the compiled `boop` help output and `cli/src/main.rs` as the source of truth for the public CLI contract. Reflect that contract in `skills/boop/SKILL.md`, and keep the guardrail in the existing `tests/test_install.sh` harness instead of introducing a second documentation test system; this keeps the fix aligned with the repo’s current maintenance model and solves the exact drift that occurred.

**Tech Stack:** Rust CLI with Clap 4, Markdown skill docs, POSIX shell regression script, Cargo test runner.

---

## Strategy Gate

The real problem is not merely “mention LLM features in the skill.” The user-facing failure is contract drift: the live `boop` CLI gained `edit`, `suggest`, `upgrade`, `--description`, and `--suggest`, while the Claude Code skill still advertises the old command set. The right fix is therefore:

1. Verify the current compiled CLI surface, not a remembered diff.
2. Update the skill routing text and command/examples sections so they expose the new commands in the places an agent will actually use.
3. Expand the existing `tests/test_install.sh` assertions so the repo fails the next time the skill falls behind the CLI.

Do not build a generated-docs pipeline, a parser that compares help output to Markdown, or a second shell harness. Those would add complexity without improving the user outcome for this repo today.

### Task 1: Characterize the drift in the existing regression harness

**Files:**
- Modify: `tests/test_install.sh`
- Reference: `skills/boop/SKILL.md`
- Reference: `cli/src/main.rs`

**Step 1: Add the missing assertions to the existing skill-doc test block**

In `tests/test_install.sh`, update `assert_file_contains` so regex patterns that begin with `-` are treated as patterns, not `grep` options:

```sh
if grep -Eq -- "$_pattern" "$_file"; then
```

Then extend `Test 10: SKILL.md contains key commands` so it asserts the current CLI additions are documented. Keep the existing checks for `boop add`, `boop list`, `boop search`, `boop delete`, `boop config`, and install-script guidance, and add these assertions:

```sh
assert_file_contains "$SKILL_MD" 'boop edit' "SKILL.md contains 'boop edit'"
assert_file_contains "$SKILL_MD" 'boop suggest' "SKILL.md contains 'boop suggest'"
assert_file_contains "$SKILL_MD" 'boop upgrade' "SKILL.md contains 'boop upgrade'"
assert_file_contains "$SKILL_MD" '--description' "SKILL.md contains '--description'"
assert_file_contains "$SKILL_MD" '--suggest' "SKILL.md contains '--suggest'"
assert_file_contains "$SKILL_MD" 'LLM suggestions' "SKILL.md mentions LLM suggestions"
```

Do not overfit the assertions to one exact sentence in the frontmatter. The regression should pin required command/flag coverage and the LLM-suggestion concept, while still allowing wording cleanup later.

**Step 2: Run the shell regression to verify it fails for the right reason**

Run:

```bash
tests/test_install.sh
```

Expected:
- the script now accepts `--description` / `--suggest` as regex patterns instead of treating them as `grep` flags
- `Test 10: SKILL.md contains key commands` fails because `skills/boop/SKILL.md` still omits `boop edit`, `boop suggest`, `boop upgrade`, `--description`, and `--suggest`

**Step 3: Verify the live CLI surface before changing docs**

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
- `add` help shows `--description` and `--suggest`
- `edit` help shows `--description` and `--suggest`
- `suggest` help shows `Usage: boop suggest <URL>`
- `upgrade` help shows `Usage: boop upgrade`

This keeps the documentation update anchored to the compiled interface the user actually runs.

**Step 4: Commit the characterization change**

```bash
git add tests/test_install.sh
git commit -m "test: pin boop skill cli coverage"
```

### Task 2: Update the Claude Code skill to match the current CLI contract

**Files:**
- Modify: `skills/boop/SKILL.md`
- Reference: `cli/src/main.rs`

**Step 1: Expand the frontmatter description so routing covers the new CLI surface**

Edit the `description:` line in `skills/boop/SKILL.md` so it still reads naturally but explicitly covers:
- bookmark editing from the CLI
- LLM suggestions for bookmarks
- upgrading the `boop` CLI
- the literal commands `boop edit`, `boop suggest`, and `boop upgrade`

Use one concise line of YAML frontmatter. Preserve the existing `boop`, `bookmarks`, `boopmark`, install/config, and CRUD routing language while extending it to the new commands.

**Step 2: Replace the partial command reference with the current public surface**

Update the command table so it includes representative rows for every current top-level command and the new flag-driven flows. Keep the existing rows that are still valid, and add at minimum:

```markdown
| `boop add <url> --description "Summary"` | Add a bookmark with a description |
| `boop add <url> --suggest`               | Add a bookmark and ask the server for suggested metadata |
| `boop edit <id> --title "New Title"`     | Edit an existing bookmark title |
| `boop edit <id> --description "Summary"` | Edit an existing bookmark description |
| `boop edit <id> --suggest`               | Ask the server to suggest missing bookmark metadata |
| `boop suggest <url>`                     | Preview suggested title, description, and tags without saving |
| `boop upgrade`                           | Upgrade `boop` to the latest version |
```

Do not try to exhaustively document every flag permutation. The skill should cover the whole user-visible surface without becoming a generated man page.

**Step 3: Refresh the usage examples so the new flows are discoverable**

Update `## Usage Examples` to include concrete examples for the new flows, while preserving the useful existing examples for search/list/install/setup. Include at least:

```bash
boop add https://example.com --description "Reference page"
boop add https://example.com --suggest
boop edit 123 --description "Updated summary"
boop edit 123 --suggest
boop suggest https://example.com
boop upgrade
```

Use realistic examples and keep the install and Gatekeeper guidance intact.

**Step 4: Run the shell regression to verify the skill now satisfies the new contract**

Run:

```bash
tests/test_install.sh
```

Expected: the expanded `SKILL.md` assertions pass alongside the existing install-script and Gatekeeper checks.

**Step 5: Commit the skill update**

```bash
git add skills/boop/SKILL.md
git commit -m "docs: update boop skill for cli upgrades"
```

### Task 3: Verify the docs and CLI remain in sync

**Files:**
- Verify: `skills/boop/SKILL.md`
- Verify: `tests/test_install.sh`
- Verify: `cli/src/main.rs`

**Step 1: Run the CLI unit tests**

Run:

```bash
cargo test -p boop
```

Expected: PASS. This confirms the documented commands and flags are still backed by the actual Clap parser and current CLI tests.

**Step 2: Re-run the shell regression from a clean state**

Run:

```bash
tests/test_install.sh
```

Expected: PASS with no regressions in install-script or skill-doc coverage.

**Step 3: Spot-check the final skill against the compiled help output**

Run:

```bash
cargo run -q -p boop -- --help
cargo run -q -p boop -- help add
cargo run -q -p boop -- help edit
cargo run -q -p boop -- help suggest
cargo run -q -p boop -- help upgrade
```

Expected: every command and flag newly documented in `skills/boop/SKILL.md` appears in the live CLI help output, and the recent additions are no longer undocumented.

**Step 4: Commit any verification-driven cleanup**

If verification exposed small wording or assertion fixes, commit them:

```bash
git add skills/boop/SKILL.md tests/test_install.sh
git commit -m "chore: finalize boop skill cli sync"
```

If verification is already clean, do not create an empty commit.

## Implementation Notes

- Favor literal command tokens in both the skill and the test harness. The drift happened because the regression guard was too narrow.
- Keep `tests/test_install.sh` as the single regression guard for install-script and skill-doc correctness. A second harness would be extra maintenance for little value here.
- Preserve valid existing content in `skills/boop/SKILL.md`, especially install/setup/Gatekeeper guidance. The task is to bring the skill up to date, not to rewrite it wholesale.
- Do not document speculative commands or flags. The live CLI currently exposes `add`, `list`, `search`, `edit`, `suggest`, `delete`, `upgrade`, and `config`.
- The critical missing user-facing pieces are the `edit`, `suggest`, and `upgrade` commands plus the `--description` and `--suggest` flags. If those are not documented and tested, the plan has not solved the actual problem.
