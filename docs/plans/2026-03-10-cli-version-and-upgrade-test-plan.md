# Test Plan: CLI `--version` and `upgrade`

## Tests

| ID  | Category | Description | Expected | Verification |
|-----|----------|-------------|----------|--------------|
| T01 | version  | `Cli::try_parse_from(["boop", "--version"])` | Produces version display (clap ErrorKind::DisplayVersion) | `cargo test -p boop` |
| T02 | version  | `Cli::try_parse_from(["boop", "-V"])` | Same as T01 | `cargo test -p boop` |
| T03 | upgrade  | `Cli::try_parse_from(["boop", "upgrade"])` | Parses successfully as Commands::Upgrade | `cargo test -p boop` |
| T04 | platform | `detect_target()` returns Ok with valid target triple | Returns string containing current arch | `cargo test -p boop` |

## Verification Commands

```
cargo test -p boop
cargo run -p boop -- --version
cargo run -p boop -- help upgrade
```
