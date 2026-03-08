# Boopmark development commands

# List available commands
default:
    @just --list

# Bootstrap the project (install deps, generate types)
setup:
    bun install
    bunx react-router typegen

# Start the dev server
dev:
    bun run dev

# Run typechecking
typecheck:
    bun run typecheck

# Run all tests
test:
    bunx vitest run

# Run tests in watch mode
test-watch:
    bunx vitest

# Run a specific test file
test-file file:
    bunx vitest run {{file}}
