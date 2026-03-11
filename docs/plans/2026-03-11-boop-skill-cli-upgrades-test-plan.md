# Boop Skill CLI Upgrades Test Plan

The agreed strategy still holds after reconciling it with the implementation plan. The plan stays limited to `skills/boop/SKILL.md` and `tests/test_install.sh`, uses the compiled CLI help output plus `cli/src/main.rs` as the product contract, and does not introduce any new harnesses or external dependencies.

## Harness Requirements

No new harnesses need to be built.

### Existing harness: live CLI help

- **What it does**: Runs the compiled `boop` binary and exposes its user-visible help text for the top-level command list and subcommand flag surface.
- **What it exposes**: Process exit status and stdout from `cargo run -q -p boop -- --help` and `cargo run -q -p boop -- help <subcommand>`.
- **Estimated complexity to build**: None. It already exists.
- **Tests that depend on it**: 1, 2, 3, 4, 7

### Existing harness: shell regression script

- **What it does**: Executes the repository’s install/skill regression checks against `install.sh` and `skills/boop/SKILL.md`.
- **What it exposes**: Pass/fail result for file-content assertions in `tests/test_install.sh`, including `assert_file_contains` and the existing SKILL.md checks.
- **Estimated complexity to build**: Low. The only planned harness change is making `assert_file_contains` pass `--` to `grep -Eq` so flag-like patterns are testable.
- **Tests that depend on it**: 5, 6, 8, 9

## Test Plan

1. **Name**: Top-level CLI help advertises every command the skill must route
   - **Type**: scenario
   - **Harness**: live CLI help
   - **Preconditions**: The worktree builds the current `boop` CLI from [`cli/src/main.rs`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/cli/src/main.rs).
   - **Actions**:
     1. Run `cargo run -q -p boop -- --help`.
   - **Expected outcome**:
     - Stdout lists `add`, `list`, `search`, `edit`, `suggest`, `delete`, `upgrade`, and `config`, matching the command surface defined in [`cli/src/main.rs`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/cli/src/main.rs) and the implementation plan.
     - The command list is the source of truth for what [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md) must advertise.
   - **Interactions**: Clap parser -> compiled `boop` binary -> stdout contract.

2. **Name**: `boop add` help exposes the metadata and suggestion flags the skill must teach
   - **Type**: scenario
   - **Harness**: live CLI help
   - **Preconditions**: Same as Test 1.
   - **Actions**:
     1. Run `cargo run -q -p boop -- help add`.
   - **Expected outcome**:
     - Stdout shows `Usage: boop add [OPTIONS] <URL>`.
     - Stdout includes `--description <DESCRIPTION>` and `--suggest`, as defined in [`cli/src/main.rs`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/cli/src/main.rs) and called out in the implementation plan.
   - **Interactions**: Clap subcommand help -> stdout contract for doc examples and command table entries.

3. **Name**: `boop edit` and `boop suggest` help expose the newer LLM flows the skill omitted
   - **Type**: scenario
   - **Harness**: live CLI help
   - **Preconditions**: Same as Test 1.
   - **Actions**:
     1. Run `cargo run -q -p boop -- help edit`.
     2. Run `cargo run -q -p boop -- help suggest`.
   - **Expected outcome**:
     - `boop edit` help shows `Usage: boop edit [OPTIONS] <ID>` and includes `--description <DESCRIPTION>` plus `--suggest`.
     - `boop suggest` help shows `Usage: boop suggest <URL>`.
     - These outputs match the command surface in [`cli/src/main.rs`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/cli/src/main.rs) and the implementation plan’s required skill updates.
   - **Interactions**: Clap subcommand help -> stdout contract for frontmatter routing and usage examples.

4. **Name**: `boop upgrade` help remains a documented user-facing command
   - **Type**: scenario
   - **Harness**: live CLI help
   - **Preconditions**: Same as Test 1.
   - **Actions**:
     1. Run `cargo run -q -p boop -- help upgrade`.
   - **Expected outcome**:
     - Stdout shows `Usage: boop upgrade`.
     - This confirms `upgrade` is part of the user-visible CLI contract and must remain present in [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md), per the implementation plan and user request.
   - **Interactions**: Clap subcommand help -> stdout contract for skill command table and examples.

5. **Name**: Skill regression harness accepts flag-shaped patterns needed to pin `--description` and `--suggest`
   - **Type**: integration
   - **Harness**: shell regression script
   - **Preconditions**:
     - [`tests/test_install.sh`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/tests/test_install.sh) is updated so `assert_file_contains` uses `grep -Eq --`.
     - [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md) contains `--description` and `--suggest`.
   - **Actions**:
     1. Run `tests/test_install.sh`.
   - **Expected outcome**:
     - The script treats `--description` and `--suggest` as regex patterns, not `grep` options.
     - The run does not fail with argument-parsing errors from `grep`.
     - This behavior is required by the implementation plan’s helper change and by the shell harness design in [`tests/test_install.sh`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/tests/test_install.sh).
   - **Interactions**: POSIX shell -> `grep` option parsing -> SKILL.md content assertions.

6. **Name**: The skill frontmatter routes users to every supported CLI workflow, including LLM suggestions and upgrade
   - **Type**: integration
   - **Harness**: shell regression script
   - **Preconditions**:
     - [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md) has been updated according to the implementation plan.
   - **Actions**:
     1. Run `tests/test_install.sh`.
   - **Expected outcome**:
     - The SKILL.md assertion block passes for `boop add`, `boop list`, `boop search`, `boop edit`, `boop suggest`, `boop delete`, `boop upgrade`, and `boop config`.
     - The same block passes for `--description`, `--suggest`, and `LLM`.
     - These tokens are justified by the user request, the implementation plan, and the live CLI help from Tests 1 through 4.
   - **Interactions**: Shell regression -> SKILL.md frontmatter and command/example text -> future CI/local verification.

7. **Name**: Skill command examples cover the live metadata and suggestion flows without inventing unsupported syntax
   - **Type**: differential
   - **Harness**: live CLI help
   - **Preconditions**:
     - [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md) includes examples for `boop add --description`, `boop add --suggest`, `boop edit --description`, `boop edit --suggest`, `boop suggest <url>`, and `boop upgrade`, as required by the implementation plan.
   - **Actions**:
     1. Run the help commands from Tests 2 through 4.
     2. Compare the documented commands in [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md) against the help output.
   - **Expected outcome**:
     - Every newly documented example uses only commands and flags that appear in the live help output.
     - No example in the updated skill introduces a flag or subcommand absent from the live CLI contract in [`cli/src/main.rs`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/cli/src/main.rs).
   - **Interactions**: Markdown skill docs -> Clap help contract -> documentation drift boundary.

8. **Name**: Existing install and Gatekeeper guidance stays intact while the skill is refreshed
   - **Type**: regression
   - **Harness**: shell regression script
   - **Preconditions**: The skill refresh has been applied without intentionally changing installation or Gatekeeper sections.
   - **Actions**:
     1. Run `tests/test_install.sh`.
   - **Expected outcome**:
     - Existing assertions for `install.sh`, `Gatekeeper|quarantine`, `xattr -cr`, and `codesign` still pass.
     - This preserves the unchanged scope required by the implementation plan: update CLI coverage without rewriting installation or macOS remediation guidance.
   - **Interactions**: Shell regression -> SKILL.md unchanged sections -> install-script coupling.

9. **Name**: CLI parser tests remain green after the skill sync so the doc still describes a real binary
   - **Type**: invariant
   - **Harness**: live CLI help
   - **Preconditions**: The worktree includes the existing CLI parser/unit tests in [`cli/src/main.rs`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/cli/src/main.rs).
   - **Actions**:
     1. Run `cargo test -p boop`.
   - **Expected outcome**:
     - The `boop` crate test suite passes.
     - This confirms the command/flag surface documented in [`skills/boop/SKILL.md`](/Users/chrisfenton/Code/personal/boopmark/.worktrees/update-boop-skill-cli-upgrades/skills/boop/SKILL.md) is still backed by the current CLI implementation, as required by the approved medium-coverage strategy.
   - **Interactions**: Rust unit tests -> Clap command definitions -> documentation contract.

## Coverage Summary

- **Covered action space**:
  - Top-level `boop` commands the skill should route: `add`, `list`, `search`, `edit`, `suggest`, `delete`, `upgrade`, `config`
  - New metadata/suggestion flags the skill must teach: `--description`, `--suggest`
  - Skill surfaces affected by drift: frontmatter description, commands reference, usage examples
  - Existing unchanged support content that must not regress: install script guidance and macOS Gatekeeper remediation
  - Final contract verification that the updated skill still matches the live CLI and passing `boop` tests

- **Explicitly excluded per the agreed strategy**:
  - New generated docs-sync tooling
  - New Playwright or end-to-end harnesses
  - Broader CLI behavior changes outside documentation and the existing shell regression
  - Manual QA or subjective review of wording

- **Risk carried by those exclusions**:
  - The plan does not create a generalized diff between CLI help and SKILL.md, so future drift beyond the pinned commands/flags could still slip through until another targeted assertion is added.
  - The plan relies on representative command-token assertions rather than a full semantic parser of the skill markdown, which is acceptable for this low-complexity doc-drift fix but not a substitute for generated documentation.
