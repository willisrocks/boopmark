# CLI `--version` flag and `upgrade` command

## Overview

Add `-V`/`--version` flag and self-update `upgrade` subcommand to the `boop` CLI. All changes in `cli/src/main.rs`.

## Task 1: Add `--version` / `-V` flag

Add `version` to the `#[command(...)]` attribute on the `Cli` struct. Clap pulls version from `CARGO_PKG_VERSION` automatically.

**Verify:** `cargo run -p boop -- --version` outputs `boop 0.1.0`

## Task 2: Add `Upgrade` subcommand

### 2a. Add variant to `Commands` enum

```rust
/// Upgrade boop to the latest version
Upgrade,
```

### 2b. `detect_target()` helper

Map `std::env::consts::{ARCH, OS}` to release target triple:
- `("x86_64", "macos")` -> `"x86_64-apple-darwin"`
- `("aarch64", "macos")` -> `"aarch64-apple-darwin"`
- `("x86_64", "linux")` -> `"x86_64-unknown-linux-gnu"`
- `("aarch64", "linux")` -> `"aarch64-unknown-linux-gnu"`
- Anything else -> error

### 2c. `upgrade()` async function

1. Call `detect_target()`
2. Construct URL: `https://github.com/foundra-build/boopmark/releases/latest/download/boop-{target}`
3. Download binary via `reqwest::Client::new()` (follows redirects)
4. Check response status, read bytes
5. Get current exe path via `std::env::current_exe()`
6. Write to staging file (`{exe_path}.tmp.{pid}`) in same directory
7. Set executable permissions (mode `0o755`) via `std::os::unix::fs::PermissionsExt`
8. On macOS: run `xattr -cr` and `codesign --force --sign -` (ignore failures)
9. Atomic rename staging file over current exe
10. Print success message with new version info

### 2d. Wire up command

Add `Commands::Upgrade => upgrade().await` to the match in `run()`.

## Task 3: Tests

In `#[cfg(test)] mod tests` at bottom of `main.rs`:

1. `test_detect_target` — verify returns `Ok(...)` with known target triple
2. `test_cli_version_flag` — `Cli::try_parse_from(["boop", "--version"])` produces version display
3. `test_cli_upgrade_recognized` — `Cli::try_parse_from(["boop", "upgrade"])` parses successfully

## Verification

```
cargo test -p boop
cargo run -p boop -- --version
cargo run -p boop -- help upgrade
```

## Files Modified

- `cli/src/main.rs`
