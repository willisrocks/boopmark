# CLI Release Pipeline & Plugin Marketplace — Test Plan

## Harness Requirements

**No new test harnesses need to be built.** All tests use shell scripts and static file assertions:

- **Install script tests**: `tests/test_install.sh` — a self-contained POSIX shell test harness that sources functions from `install.sh` via a sentinel marker (`# __BOOP_INSTALL_MAIN__`), uses uname wrappers for platform simulation, and spins up a Python HTTP mock server for e2e download tests. Follows devproxy's `test_install.sh` pattern exactly.
- **Static assertions**: Folded into `tests/test_install.sh` as Tests 10-11 (SKILL.md content checks), matching devproxy's approach. No separate assertion framework needed.
- **Cargo tests**: Existing `cargo test -p boop` validates CLI crate compilation and unit tests. No new Rust tests required.

All tests are automated and run without network access (except the mock HTTP server on localhost).

---

## Test Plan

### Category 1: Platform Detection (install.sh)

Verifies: Implementation Plan Task 1, Step 1 (`detect_platform` function)

#### T01 — Darwin/arm64 detection

- **Description**: Verify `detect_platform` maps Darwin/arm64 to `aarch64-apple-darwin`
- **Expected**: `TARGET=aarch64-apple-darwin`
- **Verification**: `tests/test_install.sh` Test 1, using uname wrapper returning `Darwin`/`arm64`

#### T02 — Darwin/x86_64 detection

- **Description**: Verify `detect_platform` maps Darwin/x86_64 to `x86_64-apple-darwin`
- **Expected**: `TARGET=x86_64-apple-darwin`
- **Verification**: `tests/test_install.sh` Test 1, using uname wrapper returning `Darwin`/`x86_64`

#### T03 — Linux/x86_64 detection

- **Description**: Verify `detect_platform` maps Linux/x86_64 to `x86_64-unknown-linux-gnu`
- **Expected**: `TARGET=x86_64-unknown-linux-gnu`
- **Verification**: `tests/test_install.sh` Test 1, using uname wrapper returning `Linux`/`x86_64`

#### T04 — Linux/aarch64 detection

- **Description**: Verify `detect_platform` maps Linux/aarch64 to `aarch64-unknown-linux-gnu`
- **Expected**: `TARGET=aarch64-unknown-linux-gnu`
- **Verification**: `tests/test_install.sh` Test 1, using uname wrapper returning `Linux`/`aarch64`

#### T05 — Linux/amd64 alias detection

- **Description**: Verify `detect_platform` maps Linux/amd64 to `x86_64-unknown-linux-gnu`
- **Expected**: `TARGET=x86_64-unknown-linux-gnu`
- **Verification**: `tests/test_install.sh` Test 1, using uname wrapper returning `Linux`/`amd64`

#### T06 — Linux/arm64 alias detection

- **Description**: Verify `detect_platform` maps Linux/arm64 to `aarch64-unknown-linux-gnu`
- **Expected**: `TARGET=aarch64-unknown-linux-gnu`
- **Verification**: `tests/test_install.sh` Test 1, using uname wrapper returning `Linux`/`arm64`

### Category 2: Unsupported Platform Errors (install.sh)

Verifies: Implementation Plan Task 1, Step 1 (error paths in `detect_platform`)

#### T07 — Unsupported OS rejected

- **Description**: Verify `detect_platform` exits non-zero for FreeBSD with an error message containing "unsupported"
- **Expected**: Non-zero exit code, stderr contains "unsupported"
- **Verification**: `tests/test_install.sh` Test 2, FreeBSD uname wrapper

#### T08 — Unsupported architecture rejected

- **Description**: Verify `detect_platform` exits non-zero for mips with an error message containing "unsupported"
- **Expected**: Non-zero exit code, stderr contains "unsupported"
- **Verification**: `tests/test_install.sh` Test 2, mips uname wrapper

### Category 3: URL Construction (install.sh)

Verifies: Implementation Plan Task 1, Step 1 (`construct_url` function)

#### T09 — Latest version URL construction

- **Description**: Verify URL is `{base}/latest/download/boop-{target}` when `BOOP_VERSION=latest`
- **Expected**: `DOWNLOAD_URL=https://example.com/releases/latest/download/boop-aarch64-apple-darwin`
- **Verification**: `tests/test_install.sh` Test 3, Darwin/arm64 with `BOOP_VERSION=latest`

#### T10 — Specific version URL construction

- **Description**: Verify URL is `{base}/download/{version}/boop-{target}` when `BOOP_VERSION=v1.0.0`
- **Expected**: `DOWNLOAD_URL=https://example.com/releases/download/v1.0.0/boop-aarch64-apple-darwin`
- **Verification**: `tests/test_install.sh` Test 3, Darwin/arm64 with `BOOP_VERSION=v1.0.0`

#### T11 — Linux URL construction

- **Description**: Verify correct URL for Linux/x86_64 platform
- **Expected**: `DOWNLOAD_URL=https://example.com/releases/latest/download/boop-x86_64-unknown-linux-gnu`
- **Verification**: `tests/test_install.sh` Test 3, Linux/x86_64 with `BOOP_VERSION=latest`

### Category 4: Full Install E2E (install.sh)

Verifies: Implementation Plan Task 1, Step 1 (full `main` flow including `download_binary`, `verify_installation`)

#### T12 — Binary installed and executable via mock server

- **Description**: Run full install.sh against a local Python HTTP mock server serving a fake binary. Verify the binary is placed in the install directory and is executable.
- **Expected**: `$BOOP_INSTALL_DIR/boop` exists and has execute permission
- **Verification**: `tests/test_install.sh` Test 4, mock HTTP server with `BOOP_INSTALL_BASE_URL`

#### T13 — Installed binary produces expected output

- **Description**: Execute the installed mock binary and verify it produces the expected test output
- **Expected**: Output contains "boop mock"
- **Verification**: `tests/test_install.sh` Test 4, runs `$INSTALL_DIR/boop`

#### T14 — Idempotent reinstall succeeds

- **Description**: Running install.sh a second time into the same directory succeeds without error
- **Expected**: Zero exit code on second run
- **Verification**: `tests/test_install.sh` Test 4, second invocation

### Category 5: Error Handling (install.sh)

Verifies: Implementation Plan Task 1, Step 1 (`download_binary` error paths)

#### T15 — Download failure (404) produces error

- **Description**: Point install.sh at a nonexistent URL path on the mock server. Verify non-zero exit and error message.
- **Expected**: Non-zero exit code, output contains "error" or "fail"
- **Verification**: `tests/test_install.sh` Test 5, `BOOP_INSTALL_BASE_URL` set to nonexistent path

#### T16 — Missing downloader (no curl/wget) produces error

- **Description**: Run install.sh with a minimal PATH that excludes curl and wget. Verify non-zero exit and error mentioning curl/wget.
- **Expected**: Non-zero exit code, output mentions "curl" or "wget"
- **Verification**: `tests/test_install.sh` Test 6, restricted PATH

### Category 6: Gatekeeper Handling (install.sh)

Verifies: Implementation Plan Task 1, Step 1 (Darwin-specific `xattr`/`codesign` in `download_binary`)

#### T17 — install.sh contains Darwin guard with xattr and codesign

- **Description**: Verify install.sh source contains the Darwin platform check, `xattr -cr`, and `codesign --force --sign -`
- **Expected**: All three patterns present in install.sh
- **Verification**: `tests/test_install.sh` Test 7, static grep assertions

#### T18 — Gatekeeper commands in correct order

- **Description**: Verify `chmod` comes before `xattr`, which comes before `codesign`
- **Expected**: Line numbers are in strictly ascending order
- **Verification**: `tests/test_install.sh` Test 8, line-number comparison

#### T19 — xattr and codesign inside Darwin if/fi block

- **Description**: Verify `xattr` and `codesign` calls are enclosed within the `Darwin` conditional block
- **Expected**: All lines fall between the Darwin check line and the closing `fi` line
- **Verification**: `tests/test_install.sh` Test 9, structural line-number analysis

### Category 7: SKILL.md Content Assertions

Verifies: Implementation Plan Task 5, Step 3 (SKILL.md content)

#### T20 — SKILL.md contains all CLI commands

- **Description**: Verify SKILL.md references `boop add`, `boop list`, `boop search`, `boop delete`, `boop config`, and `install.sh`
- **Expected**: All six patterns found in `skills/boop/SKILL.md`
- **Verification**: `tests/test_install.sh` Test 10, grep assertions

#### T21 — SKILL.md documents Gatekeeper common issue

- **Description**: Verify SKILL.md mentions Gatekeeper/quarantine, `xattr -cr`, and `codesign`
- **Expected**: All three patterns found in `skills/boop/SKILL.md`
- **Verification**: `tests/test_install.sh` Test 11, grep assertions

### Category 8: Cargo Tests

Verifies: Implementation Plan Tasks 1-7 (CLI crate compiles and passes unit tests)

#### T22 — boop crate tests pass

- **Description**: Run `cargo test -p boop` to verify the CLI crate compiles and all existing unit tests pass
- **Expected**: All tests pass, zero exit code
- **Verification**: `cargo test -p boop` (run via `just check` or directly)

### Category 9: Structural Validation (CI/Release/Plugin Files)

Verifies: Implementation Plan Tasks 3, 4, 5

#### T23 — release.yml contains all 4 target triples

- **Description**: Verify `.github/workflows/release.yml` contains target entries for all four platforms
- **Expected**: File contains `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- **Verification**: Manual grep or CI lint (covered by `sh -n` syntax check in Task 3 Step 2 and visual review)

#### T24 — ci.yml contains expected jobs

- **Description**: Verify `.github/workflows/ci.yml` contains `check` and `install-script` jobs
- **Expected**: File contains both job definitions
- **Verification**: YAML parse validation (`python3 -c "import yaml; ..."`) and visual review

#### T25 — marketplace.json is valid JSON with required fields

- **Description**: Verify `.claude-plugin/marketplace.json` parses as valid JSON and contains `name`, `plugins` array
- **Expected**: Valid JSON, `name` is `"boopmark"`, `plugins` array has one entry
- **Verification**: `python3 -c "import json; ..."` or visual review

#### T26 — plugin.json is valid JSON with required fields

- **Description**: Verify `.claude-plugin/plugin.json` parses as valid JSON and contains `name`, `version`, `description`
- **Expected**: Valid JSON with all required fields present
- **Verification**: `python3 -c "import json; ..."` or visual review

#### T27 — install.sh syntax check

- **Description**: Verify install.sh has no syntax errors
- **Expected**: `sh -n install.sh` exits zero with no output
- **Verification**: `sh -n install.sh` (Task 1, Step 2)

### Category 10: Justfile Integration

Verifies: Implementation Plan Task 6

#### T28 — just --list shows new commands

- **Description**: Verify `just --list` output includes `test-install`, `fmt-check`, and `check`
- **Expected**: All three commands appear in the listing
- **Verification**: `just --list` (Task 6, Step 2)

#### T29 — just test-install runs successfully

- **Description**: Verify `just test-install` executes `tests/test_install.sh` and all tests pass
- **Expected**: Zero exit code, summary shows 0 failures
- **Verification**: `just test-install` (Task 6, Step 3)

---

## Verification Commands

Run these commands to execute the full test plan:

```bash
# All install script tests (T01-T21)
just test-install

# Cargo tests for boop crate (T22)
cargo test -p boop

# Structural checks (T23-T27)
sh -n install.sh
python3 -c "import json; json.load(open('.claude-plugin/marketplace.json'))"
python3 -c "import json; json.load(open('.claude-plugin/plugin.json'))"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" 2>/dev/null || echo "PyYAML not available, skip"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" 2>/dev/null || echo "PyYAML not available, skip"

# Justfile integration (T28-T29)
just --list
just test-install
```

**All tests are automated.** No manual QA or human validation is required.
