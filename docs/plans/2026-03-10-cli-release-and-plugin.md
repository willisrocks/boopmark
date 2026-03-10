# CLI Release Pipeline & Plugin Marketplace Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Add GitHub Actions release pipeline for the `boop` CLI binary (4 platforms), a universal install script with tests, justfile release commands, Claude Code plugin marketplace with a boop skill, and README install instructions.

**Architecture:** Closely follows the devproxy reference implementation at `~/Code/personal/devproxy`. Release workflow builds 4 platform binaries via `workflow_dispatch`, install.sh handles platform detection and download, tests validate the install script in CI. Plugin marketplace uses `.claude-plugin/` at repo root with a single plugin containing a `boop` skill.

**Tech Stack:** GitHub Actions, shell (POSIX sh), just, Claude Code plugin marketplace (marketplace.json + plugin.json + SKILL.md)

**Key Decisions:**
- Binary name in releases: `boop-{target}` (e.g. `boop-aarch64-apple-darwin`), matching devproxy's pattern.
- The release workflow builds only the `boop` CLI crate (`cargo build --release -p boop`), not the server.
- Env vars for install.sh: `BOOP_VERSION`, `BOOP_INSTALL_DIR`, `BOOP_INSTALL_BASE_URL` — matching devproxy's naming convention.
- Sentinel marker: `# __BOOP_INSTALL_MAIN__` — used by test harness to strip the main() call.
- Plugin marketplace name: `boopmark` (used as `@boopmark` in install commands).
- Plugin source: relative path `"./"` since skills live at repo root alongside marketplace.json.
- The CI workflow does NOT run `cargo test` for the server (which needs Postgres) — it only runs `cargo test -p boop` and the install script tests. The existing justfile `test` command continues to run all workspace tests locally.

---

### Task 1: Create `install.sh`

**Files:**
- Create: `install.sh`

**Step 1: Write install.sh**

Create `install.sh` at the repo root. This is adapted directly from devproxy's `install.sh`, replacing `devproxy` with `boop` and `DEVPROXY_` env var prefixes with `BOOP_`:

```sh
#!/bin/sh
set -eu

BOOP_VERSION="${BOOP_VERSION:-latest}"
BOOP_INSTALL_DIR="${BOOP_INSTALL_DIR:-${HOME}/.local/bin}"
BOOP_INSTALL_BASE_URL="${BOOP_INSTALL_BASE_URL:-https://github.com/foundra-build/boopmark/releases}"

main() {
    detect_platform
    construct_url
    create_install_dir
    download_binary
    verify_installation
    echo "boop installed successfully to ${BOOP_INSTALL_DIR}/boop"
    case ":${PATH}:" in
        *":${BOOP_INSTALL_DIR}:"*) ;;
        *) echo "Note: ${BOOP_INSTALL_DIR} is not in your PATH. Add it with:" >&2
           echo "  export PATH=\"${BOOP_INSTALL_DIR}:\$PATH\"" >&2 ;;
    esac
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin) OS_TARGET="apple-darwin" ;;
        Linux)  OS_TARGET="unknown-linux-gnu" ;;
        *)      echo "Error: unsupported operating system: $OS" >&2; exit 1 ;;
    esac

    case "$ARCH" in
        x86_64|amd64)  ARCH_TARGET="x86_64" ;;
        aarch64|arm64) ARCH_TARGET="aarch64" ;;
        *)             echo "Error: unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac

    TARGET="${ARCH_TARGET}-${OS_TARGET}"
}

construct_url() {
    BINARY_NAME="boop-${TARGET}"
    if [ "$BOOP_VERSION" = "latest" ]; then
        DOWNLOAD_URL="${BOOP_INSTALL_BASE_URL}/latest/download/${BINARY_NAME}"
    else
        DOWNLOAD_URL="${BOOP_INSTALL_BASE_URL}/download/${BOOP_VERSION}/${BINARY_NAME}"
    fi
}

create_install_dir() {
    if [ ! -d "$BOOP_INSTALL_DIR" ]; then
        if ! mkdir -p "$BOOP_INSTALL_DIR" 2>/dev/null; then
            echo "Error: failed to create install directory ${BOOP_INSTALL_DIR}" >&2
            echo "Try running with sudo or set BOOP_INSTALL_DIR to a writable location." >&2
            exit 1
        fi
    elif [ ! -w "$BOOP_INSTALL_DIR" ]; then
        echo "Error: install directory ${BOOP_INSTALL_DIR} is not writable" >&2
        echo "Try running with sudo or set BOOP_INSTALL_DIR to a writable location." >&2
        exit 1
    fi
}

download_binary() {
    TMPFILE="$(mktemp)"
    trap 'rm -f "$TMPFILE"' EXIT

    if command -v curl >/dev/null 2>&1; then
        if ! curl -fsSL -o "$TMPFILE" "$DOWNLOAD_URL"; then
            echo "Error: failed to download boop from ${DOWNLOAD_URL}" >&2
            exit 1
        fi
    elif command -v wget >/dev/null 2>&1; then
        if ! wget -q -O "$TMPFILE" "$DOWNLOAD_URL"; then
            echo "Error: failed to download boop from ${DOWNLOAD_URL}" >&2
            exit 1
        fi
    else
        echo "Error: neither curl nor wget found. Please install one and try again." >&2
        exit 1
    fi

    if ! cp "$TMPFILE" "${BOOP_INSTALL_DIR}/boop"; then
        echo "Error: failed to copy binary to ${BOOP_INSTALL_DIR}/boop" >&2
        exit 1
    fi
    if ! chmod 755 "${BOOP_INSTALL_DIR}/boop"; then
        echo "Error: failed to set executable permissions on ${BOOP_INSTALL_DIR}/boop" >&2
        exit 1
    fi
    if [ "$(uname -s)" = "Darwin" ]; then
        xattr -cr "${BOOP_INSTALL_DIR}/boop" 2>/dev/null || true
        if ! codesign --force --sign - "${BOOP_INSTALL_DIR}/boop" 2>/dev/null; then
            echo "Warning: failed to ad-hoc sign binary; Gatekeeper may kill the binary on first run" >&2
        fi
    fi
    rm -f "$TMPFILE"
    trap - EXIT
}

verify_installation() {
    if [ ! -x "${BOOP_INSTALL_DIR}/boop" ]; then
        echo "Error: installation failed — binary not found at ${BOOP_INSTALL_DIR}/boop" >&2
        exit 1
    fi
}

# __BOOP_INSTALL_MAIN__
main
```

**Step 2: Verify install.sh is well-formed**

Run: `sh -n install.sh`
Expected: no output (syntax OK)

**Step 3: Commit**

```bash
git add install.sh
git commit -m "feat: add universal install script for boop CLI"
```

---

### Task 2: Create `tests/test_install.sh`

**Files:**
- Create: `tests/test_install.sh`

**Step 1: Write test_install.sh**

Adapted from devproxy's test suite, replacing all `devproxy`/`DEVPROXY` references with `boop`/`BOOP`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALL_SCRIPT="$REPO_ROOT/install.sh"

PASS=0
FAIL=0
TOTAL=0

pass() {
    PASS=$((PASS + 1))
    TOTAL=$((TOTAL + 1))
    echo "  PASS: $1"
}

fail() {
    FAIL=$((FAIL + 1))
    TOTAL=$((TOTAL + 1))
    echo "  FAIL: $1"
    if [ -n "${2:-}" ]; then
        echo "        $2"
    fi
}

cleanup() {
    if [ -n "${MOCK_SERVER_PID:-}" ]; then
        kill "$MOCK_SERVER_PID" 2>/dev/null || true
        wait "$MOCK_SERVER_PID" 2>/dev/null || true
    fi
    if [ -n "${TMPDIR_ROOT:-}" ]; then
        rm -rf "$TMPDIR_ROOT"
    fi
}
trap cleanup EXIT

TMPDIR_ROOT="$(mktemp -d)"

# --- Helper: create a uname wrapper that returns custom OS/ARCH ---
make_uname_wrapper() {
    _os="$1"
    _arch="$2"
    _dir="$TMPDIR_ROOT/uname-wrapper-${_os}-${_arch}"
    mkdir -p "$_dir"
    cat > "$_dir/uname" <<WRAPPER
#!/bin/sh
case "\$1" in
    -s) echo "$_os" ;;
    -m) echo "$_arch" ;;
    *)  /usr/bin/uname "\$@" ;;
esac
WRAPPER
    chmod +x "$_dir/uname"
    echo "$_dir"
}

# --- Helper: build a harness script from install.sh ---
make_harness() {
    _harness_file="$1"
    if ! grep -q '^# __BOOP_INSTALL_MAIN__$' "$INSTALL_SCRIPT"; then
        echo "FATAL: install.sh is missing the # __BOOP_INSTALL_MAIN__ sentinel marker" >&2
        exit 2
    fi
    sed '/^# __BOOP_INSTALL_MAIN__$/,$d' "$INSTALL_SCRIPT" > "$_harness_file"
}

# --- Helper: extract detect_platform + construct_url and print TARGET/URL ---
run_detection() {
    _uname_dir="$1"
    _base_url="${2:-https://github.com/foundra-build/boopmark/releases}"
    _version="${3:-latest}"
    _harness="$TMPDIR_ROOT/harness-$$.sh"
    make_harness "$_harness"
    cat >> "$_harness" <<'HARNESS'
detect_platform
construct_url
echo "TARGET=$TARGET"
echo "DOWNLOAD_URL=$DOWNLOAD_URL"
HARNESS
    PATH="$_uname_dir:$PATH" \
        BOOP_INSTALL_BASE_URL="$_base_url" \
        BOOP_VERSION="$_version" \
        sh "$_harness" 2>/dev/null
    rm -f "$_harness"
}

# ============================================================
# Test 1: OS/arch detection — all 4 platform combos
# ============================================================
echo "=== Test 1: OS/arch detection ==="

for combo in "Darwin:arm64:aarch64-apple-darwin" \
             "Darwin:x86_64:x86_64-apple-darwin" \
             "Linux:x86_64:x86_64-unknown-linux-gnu" \
             "Linux:aarch64:aarch64-unknown-linux-gnu" \
             "Linux:amd64:x86_64-unknown-linux-gnu" \
             "Linux:arm64:aarch64-unknown-linux-gnu"; do
    os="$(echo "$combo" | cut -d: -f1)"
    arch="$(echo "$combo" | cut -d: -f2)"
    expected="$(echo "$combo" | cut -d: -f3)"

    wrapper_dir="$(make_uname_wrapper "$os" "$arch")"
    result="$(run_detection "$wrapper_dir" | grep '^TARGET=' | cut -d= -f2)"

    if [ "$result" = "$expected" ]; then
        pass "$os/$arch -> $expected"
    else
        fail "$os/$arch -> expected $expected, got $result"
    fi
done

# ============================================================
# Test 2: Unsupported platform error
# ============================================================
echo "=== Test 2: Unsupported platform error ==="

run_detection_with_stderr() {
    _uname_dir="$1"
    _harness="$TMPDIR_ROOT/harness-unsup-$$.sh"
    make_harness "$_harness"
    cat >> "$_harness" <<'HARNESS'
detect_platform
HARNESS
    _rc=0
    PATH="$_uname_dir:$PATH" \
        BOOP_INSTALL_BASE_URL="https://example.com" \
        BOOP_VERSION="latest" \
        sh "$_harness" 2>&1 || _rc=$?
    rm -f "$_harness"
    return $_rc
}

# Unsupported OS
wrapper_dir="$(make_uname_wrapper "FreeBSD" "x86_64")"
if output="$(run_detection_with_stderr "$wrapper_dir")"; then
    fail "FreeBSD should fail but exited 0"
else
    if echo "$output" | grep -qi "unsupported"; then
        pass "FreeBSD rejected with error message"
    else
        fail "FreeBSD rejected but no 'unsupported' in message" "$output"
    fi
fi

# Unsupported arch
wrapper_dir="$(make_uname_wrapper "Linux" "mips")"
if output="$(run_detection_with_stderr "$wrapper_dir")"; then
    fail "mips should fail but exited 0"
else
    if echo "$output" | grep -qi "unsupported"; then
        pass "mips rejected with error message"
    else
        fail "mips rejected but no 'unsupported' in message" "$output"
    fi
fi

# ============================================================
# Test 3: URL construction
# ============================================================
echo "=== Test 3: URL construction ==="

BASE="https://example.com/releases"

# Latest version
wrapper_dir="$(make_uname_wrapper "Darwin" "arm64")"
url="$(run_detection "$wrapper_dir" "$BASE" "latest" | grep '^DOWNLOAD_URL=' | cut -d= -f2-)"
expected_url="https://example.com/releases/latest/download/boop-aarch64-apple-darwin"
if [ "$url" = "$expected_url" ]; then
    pass "latest URL for Darwin/arm64"
else
    fail "latest URL: expected $expected_url, got $url"
fi

# Specific version
url="$(run_detection "$wrapper_dir" "$BASE" "v1.0.0" | grep '^DOWNLOAD_URL=' | cut -d= -f2-)"
expected_url="https://example.com/releases/download/v1.0.0/boop-aarch64-apple-darwin"
if [ "$url" = "$expected_url" ]; then
    pass "versioned URL for Darwin/arm64"
else
    fail "versioned URL: expected $expected_url, got $url"
fi

# Linux x86_64
wrapper_dir="$(make_uname_wrapper "Linux" "x86_64")"
url="$(run_detection "$wrapper_dir" "$BASE" "latest" | grep '^DOWNLOAD_URL=' | cut -d= -f2-)"
expected_url="https://example.com/releases/latest/download/boop-x86_64-unknown-linux-gnu"
if [ "$url" = "$expected_url" ]; then
    pass "latest URL for Linux/x86_64"
else
    fail "latest URL: expected $expected_url, got $url"
fi

# ============================================================
# Test 4: Full install e2e with mock server
# ============================================================
echo "=== Test 4: Full install e2e ==="

MOCK_DIR="$TMPDIR_ROOT/mock-server"
mkdir -p "$MOCK_DIR/latest/download"

_mock_arch="$(uname -m | sed 's/arm64/aarch64/')"
_mock_os=""
case "$(uname -s)" in
    Darwin) _mock_os="apple-darwin" ;;
    Linux)  _mock_os="unknown-linux-gnu" ;;
    *)      echo "  SKIP: e2e tests not supported on $(uname -s)" ;;
esac

if [ -z "$_mock_os" ]; then
    echo "=== Test 5: Download failure (404) ==="
    echo "  SKIP: e2e tests not supported on $(uname -s)"
else

MOCK_BINARY="$MOCK_DIR/latest/download/boop-${_mock_arch}-${_mock_os}"
cat > "$MOCK_BINARY" <<'MOCKBIN'
#!/bin/sh
echo "boop mock 0.0.1-test"
MOCKBIN
chmod +x "$MOCK_BINARY"

MOCK_PORT=0
MOCK_PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()")
cd "$MOCK_DIR"
python3 -m http.server "$MOCK_PORT" >/dev/null 2>&1 &
MOCK_SERVER_PID=$!
cd "$REPO_ROOT"
_retries=0
while ! curl -s -o /dev/null "http://localhost:${MOCK_PORT}/" 2>/dev/null; do
    _retries=$((_retries + 1))
    if [ "$_retries" -ge 50 ]; then
        echo "FATAL: mock HTTP server failed to start on port $MOCK_PORT" >&2
        exit 2
    fi
    sleep 0.1
done

INSTALL_DIR="$TMPDIR_ROOT/install-target"
mkdir -p "$INSTALL_DIR"

if BOOP_INSTALL_BASE_URL="http://localhost:${MOCK_PORT}" \
   BOOP_INSTALL_DIR="$INSTALL_DIR" \
   sh "$INSTALL_SCRIPT" >/dev/null 2>&1; then
    if [ -x "$INSTALL_DIR/boop" ]; then
        pass "binary installed and executable"
    else
        fail "binary not found or not executable at $INSTALL_DIR/boop"
    fi

    mock_output="$("$INSTALL_DIR/boop" 2>&1 || true)"
    if echo "$mock_output" | grep -q "boop mock"; then
        pass "installed binary produces expected output"
    else
        fail "binary output unexpected" "$mock_output"
    fi

    if BOOP_INSTALL_BASE_URL="http://localhost:${MOCK_PORT}" \
       BOOP_INSTALL_DIR="$INSTALL_DIR" \
       sh "$INSTALL_SCRIPT" >/dev/null 2>&1; then
        pass "idempotent reinstall succeeds"
    else
        fail "idempotent reinstall failed"
    fi
else
    fail "install script failed"
fi

# ============================================================
# Test 5: Download failure (404)
# ============================================================
echo "=== Test 5: Download failure (404) ==="

INSTALL_DIR_404="$TMPDIR_ROOT/install-404"
mkdir -p "$INSTALL_DIR_404"

wrapper_dir="$(make_uname_wrapper "Linux" "aarch64")"
if output="$(PATH="$wrapper_dir:$PATH" \
   BOOP_INSTALL_BASE_URL="http://localhost:${MOCK_PORT}/nonexistent" \
   BOOP_INSTALL_DIR="$INSTALL_DIR_404" \
   sh "$INSTALL_SCRIPT" 2>&1)"; then
    fail "404 should cause non-zero exit"
else
    if echo "$output" | grep -Eqi "error|fail"; then
        pass "404 produces error message"
    else
        fail "404 exited non-zero but no error in output" "$output"
    fi
fi

fi  # end of _mock_os check for e2e tests (Tests 4 and 5)

# ============================================================
# Test 6: Missing downloader
# ============================================================
echo "=== Test 6: Missing downloader ==="

INSTALL_DIR_NODL="$TMPDIR_ROOT/install-nodl"
mkdir -p "$INSTALL_DIR_NODL"

MINIMAL_BIN="$TMPDIR_ROOT/minimal-bin"
mkdir -p "$MINIMAL_BIN"
for cmd in sh uname mktemp chmod mkdir mv rm cat sed grep printf echo test tr cut; do
    cmd_path="$(command -v "$cmd" 2>/dev/null || true)"
    if [ -n "$cmd_path" ]; then
        ln -sf "$cmd_path" "$MINIMAL_BIN/$cmd" 2>/dev/null || true
    fi
done
if [ -f /bin/[ ]; then
    ln -sf /bin/[ "$MINIMAL_BIN/[" 2>/dev/null || true
fi
ln -sf "$(command -v env)" "$MINIMAL_BIN/env" 2>/dev/null || true

if output="$(PATH="$MINIMAL_BIN" \
   BOOP_INSTALL_BASE_URL="http://localhost:${MOCK_PORT:-0}" \
   BOOP_INSTALL_DIR="$INSTALL_DIR_NODL" \
   sh "$INSTALL_SCRIPT" 2>&1)"; then
    fail "missing downloader should cause non-zero exit"
else
    if echo "$output" | grep -Eqi "curl|wget"; then
        pass "missing downloader error mentions curl/wget"
    else
        fail "missing downloader exited non-zero but no curl/wget mention" "$output"
    fi
fi

# ============================================================
# Test 7: Gatekeeper fix — Darwin guard present with xattr and codesign
# ============================================================
echo "=== Test 7: Gatekeeper fix — Darwin guard ==="

assert_file_contains() {
    _file="$1"
    _pattern="$2"
    _desc="$3"
    if grep -Eq "$_pattern" "$_file"; then
        pass "$_desc"
    else
        fail "$_desc" "pattern '$_pattern' not found in $_file"
    fi
}

assert_line_before() {
    _file="$1"
    _first="$2"
    _second="$3"
    _desc="$4"
    _line_first="$(grep -n "$_first" "$_file" | head -1 | cut -d: -f1)"
    _line_second="$(grep -n "$_second" "$_file" | head -1 | cut -d: -f1)"
    if [ -z "$_line_first" ] || [ -z "$_line_second" ]; then
        fail "$_desc" "could not find lines for '$_first' or '$_second'"
    elif [ "$_line_first" -lt "$_line_second" ]; then
        pass "$_desc"
    else
        fail "$_desc" "'$_first' (line $_line_first) should come before '$_second' (line $_line_second)"
    fi
}

assert_file_contains "$INSTALL_SCRIPT" 'uname -s.*Darwin' "install.sh contains Darwin guard"
assert_file_contains "$INSTALL_SCRIPT" 'xattr -cr' "install.sh contains xattr -cr"
assert_file_contains "$INSTALL_SCRIPT" 'codesign --force --sign -' "install.sh contains codesign"

echo "=== Test 8: Gatekeeper fix — ordering ==="
assert_line_before "$INSTALL_SCRIPT" 'chmod 755' 'xattr -cr' "chmod before xattr"
assert_line_before "$INSTALL_SCRIPT" 'chmod 755' 'codesign' "chmod before codesign"
assert_line_before "$INSTALL_SCRIPT" 'xattr -cr' 'codesign' "xattr before codesign"

echo "=== Test 9: Gatekeeper fix — Darwin-only guard ==="

_darwin_line="$(grep -n 'uname -s.*Darwin' "$INSTALL_SCRIPT" | head -1 | cut -d: -f1)"
_xattr_line="$(grep -n 'xattr -cr' "$INSTALL_SCRIPT" | head -1 | cut -d: -f1)"
_codesign_line="$(grep -n 'codesign --force --sign -' "$INSTALL_SCRIPT" | head -1 | cut -d: -f1)"
_fi_line="$(awk -v start="$_darwin_line" 'NR > start && /^    fi$/ { print NR; exit }' "$INSTALL_SCRIPT")"

if [ -n "$_darwin_line" ] && [ -n "$_xattr_line" ] && [ -n "$_codesign_line" ] && [ -n "$_fi_line" ]; then
    if [ "$_darwin_line" -lt "$_xattr_line" ] && \
       [ "$_xattr_line" -lt "$_codesign_line" ] && \
       [ "$_codesign_line" -lt "$_fi_line" ]; then
        pass "xattr and codesign are inside Darwin if/fi block"
    else
        fail "xattr and codesign ordering within Darwin block" \
             "darwin=$_darwin_line xattr=$_xattr_line codesign=$_codesign_line fi=$_fi_line"
    fi
else
    fail "could not find all Darwin guard markers" \
         "darwin=$_darwin_line xattr=$_xattr_line codesign=$_codesign_line fi=$_fi_line"
fi

# ============================================================
# Summary
# ============================================================
echo ""
echo "============================================================"
echo "Results: $PASS passed, $FAIL failed, $TOTAL total"
echo "============================================================"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
```

**Step 2: Run the install script tests locally**

Run: `sh tests/test_install.sh`
Expected: All tests pass (0 failures)

**Step 3: Commit**

```bash
git add tests/test_install.sh
git commit -m "test: add install script tests for boop CLI"
```

---

### Task 3: Create GitHub Actions release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Step 1: Write release.yml**

Adapted from devproxy's release workflow. Key difference: uses `cargo build --release -p boop` to build only the CLI crate, and renames the binary from `boop` to `boop-{target}`:

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      version:
        description: 'Release version (e.g., 0.1.0)'
        required: true
        type: string

concurrency:
  group: "release"
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always

jobs:
  validate:
    name: Validate inputs
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}

      - name: Validate version format
        env:
          RELEASE_VERSION: ${{ github.event.inputs.version }}
        run: |
          if ! echo "${RELEASE_VERSION}" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
            echo "Error: version '${RELEASE_VERSION}' does not match semver format (e.g., 0.1.0)" >&2
            exit 1
          fi

      - name: Check tag does not already exist
        env:
          RELEASE_VERSION: ${{ github.event.inputs.version }}
        run: |
          if git ls-remote --tags origin | grep -q "refs/tags/v${RELEASE_VERSION}$"; then
            echo "Error: tag v${RELEASE_VERSION} already exists" >&2
            exit 1
          fi

  build:
    name: Build ${{ matrix.target }}
    needs: validate
    runs-on: ${{ matrix.os }}
    timeout-minutes: 30
    strategy:
      matrix:
        include:
          - target: x86_64-apple-darwin
            os: macos-latest
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}

      - name: Run tests
        if: matrix.target == 'aarch64-apple-darwin' || matrix.target == 'x86_64-unknown-linux-gnu'
        run: cargo test -p boop

      - name: Install cross (Linux cross-compile)
        if: matrix.target == 'aarch64-unknown-linux-gnu'
        run: cargo install cross --git https://github.com/cross-rs/cross --rev f8151ae777290430cf2108efacf3976d9528500b

      - name: Build (native)
        if: matrix.target != 'aarch64-unknown-linux-gnu'
        run: cargo build --release -p boop --target ${{ matrix.target }}

      - name: Build (cross)
        if: matrix.target == 'aarch64-unknown-linux-gnu'
        run: cross build --release -p boop --target ${{ matrix.target }}

      - name: Rename binary
        run: cp target/${{ matrix.target }}/release/boop boop-${{ matrix.target }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: boop-${{ matrix.target }}
          path: boop-${{ matrix.target }}

  release:
    name: Create Release
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}

      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Prepare and verify binaries
        run: |
          mkdir -p release
          for dir in artifacts/boop-*/; do
            cp "$dir"boop-* release/
          done
          chmod +x release/*
          EXPECTED_COUNT=4
          ACTUAL_COUNT=$(ls release/ | wc -l | tr -d ' ')
          if [ "$ACTUAL_COUNT" -ne "$EXPECTED_COUNT" ]; then
            echo "Error: expected $EXPECTED_COUNT binaries but found $ACTUAL_COUNT" >&2
            ls -la release/
            exit 1
          fi
          ls -la release/

      - name: Re-check tag does not exist
        env:
          RELEASE_VERSION: ${{ github.event.inputs.version }}
        run: |
          if git ls-remote --tags origin | grep -q "refs/tags/v${RELEASE_VERSION}$"; then
            echo "Error: tag v${RELEASE_VERSION} was created since validation (concurrent release?)" >&2
            exit 1
          fi

      - name: Create tag
        env:
          RELEASE_VERSION: ${{ github.event.inputs.version }}
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git tag -a "v${RELEASE_VERSION}" -m "Release v${RELEASE_VERSION}"
          git push origin "v${RELEASE_VERSION}"

      - name: Create GitHub Release
        env:
          GH_TOKEN: ${{ github.token }}
          RELEASE_VERSION: ${{ github.event.inputs.version }}
        run: |
          gh release create "v${RELEASE_VERSION}" \
            --title "v${RELEASE_VERSION}" \
            --generate-notes \
            release/*
```

**Step 2: Verify YAML is valid**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"`
Expected: no error (if PyYAML is available), or manually verify structure

**Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add GitHub Actions release workflow for boop CLI"
```

---

### Task 4: Create GitHub Actions CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

**Step 1: Write ci.yml**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

concurrency:
  group: "ci-${{ github.ref }}"
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    name: Check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format check
        run: cargo fmt -- --check
      - name: Clippy
        run: cargo clippy -p boop --all-targets -- -D warnings
      - name: Tests
        run: cargo test -p boop

  install-script:
    name: Install script tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run install script tests
        run: sh tests/test_install.sh
```

**Note:** CI only checks the `boop` crate (not the server, which requires Postgres). This is intentional — the server has its own test infrastructure needs.

**Step 2: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add CI workflow for boop CLI and install script"
```

---

### Task 5: Create Claude Code plugin marketplace

**Files:**
- Create: `.claude-plugin/marketplace.json`
- Create: `.claude-plugin/plugin.json`
- Create: `skills/boop/SKILL.md`

**Step 1: Write marketplace.json**

```json
{
  "name": "boopmark",
  "owner": {
    "name": "Foundra",
    "email": "hello@foundra.build"
  },
  "metadata": {
    "description": "Claude Code plugins for boopmark — CLI bookmark management"
  },
  "plugins": [
    {
      "name": "boop",
      "source": "./",
      "description": "Skills for using the boop CLI to manage bookmarks from Claude Code",
      "version": "0.1.0",
      "author": {
        "name": "Foundra"
      },
      "homepage": "https://github.com/foundra-build/boopmark",
      "repository": "https://github.com/foundra-build/boopmark",
      "license": "MIT",
      "keywords": ["bookmarks", "cli", "boop", "boopmark"],
      "category": "developer-tools",
      "tags": ["bookmarks", "cli", "productivity"]
    }
  ]
}
```

**Step 2: Write plugin.json**

```json
{
  "name": "boop",
  "version": "0.1.0",
  "description": "Claude Code plugin for managing bookmarks with the boop CLI",
  "author": {
    "name": "Foundra",
    "url": "https://github.com/foundra-build"
  },
  "repository": "https://github.com/foundra-build/boopmark",
  "license": "MIT",
  "keywords": ["bookmarks", "cli", "boop", "boopmark"]
}
```

**Step 3: Write skills/boop/SKILL.md**

This skill teaches Claude Code how to use the boop CLI for bookmark management. It covers installation, configuration, and all commands:

```markdown
---
name: boop
description: This skill should be used when the user mentions "boop", "bookmarks", "boopmark", asks about managing bookmarks from the CLI, wants to add/list/search/delete bookmarks, needs to configure the boop CLI, or asks about "boop add", "boop list", "boop search", "boop delete", "boop config".
---

# boop CLI

Command-line interface for managing bookmarks on Boopmark. Single Rust binary.

**Prerequisites:** The boop CLI must be installed. See the install section below.

## Commands Reference

| Command                              | What it does                              |
|--------------------------------------|-------------------------------------------|
| `boop add <url>`                     | Add a bookmark                            |
| `boop add <url> --title "My Title"`  | Add a bookmark with a title               |
| `boop add <url> --tags "a,b,c"`      | Add a bookmark with tags                  |
| `boop list`                          | List all bookmarks (newest first)         |
| `boop list --search "query"`         | List bookmarks matching a search query    |
| `boop list --tags "tag1,tag2"`       | List bookmarks with specific tags         |
| `boop list --sort oldest`            | List bookmarks sorted oldest first        |
| `boop search <query>`               | Search bookmarks                          |
| `boop delete <id>`                   | Delete a bookmark by ID                   |
| `boop config set-server <url>`       | Set the Boopmark server URL               |
| `boop config set-key <key>`          | Set your API key                          |
| `boop config show`                   | Show current configuration                |

## Installation

Install via the install script:

```bash
curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | sh
```

Install a specific version:

```bash
BOOP_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | sh
```

Custom install directory:

```bash
BOOP_INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | sh
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

Add a bookmark:
```bash
boop add https://example.com --title "Example Site" --tags "reference,docs"
```

Search bookmarks:
```bash
boop search "rust async"
```

List recent bookmarks with tag filter:
```bash
boop list --tags "rust" --sort newest
```

## Common Issues

| Problem | Fix |
|---------|-----|
| "Server URL not configured" | Run `boop config set-server <url>` |
| "API key not configured" | Run `boop config set-key <key>` — generate a key in the Boopmark web UI settings |
| Binary "killed" on macOS | Gatekeeper quarantine. Run: `xattr -cr $(which boop) && codesign --force --sign - $(which boop)` |
| `boop: command not found` | `~/.local/bin` may not be in your PATH. Add: `export PATH="$HOME/.local/bin:$PATH"` to your shell profile |
```

**Step 4: Commit**

```bash
git add .claude-plugin/marketplace.json .claude-plugin/plugin.json skills/boop/SKILL.md
git commit -m "feat: add Claude Code plugin marketplace with boop skill"
```

---

### Task 6: Update justfile with release commands

**Files:**
- Modify: `justfile`

**Step 1: Add new commands to the justfile**

Append the following commands to the existing justfile (after the `deploy` recipe):

```just
# Run install script tests
test-install:
    sh tests/test_install.sh

# Format check (CI)
fmt-check:
    cargo fmt -- --check

# Run clippy
check:
    cargo clippy -p boop --all-targets -- -D warnings
    cargo test -p boop
```

**Step 2: Verify justfile is valid**

Run: `just --list`
Expected: new commands `test-install`, `fmt-check`, `check` appear in the list

**Step 3: Run the install tests through just**

Run: `just test-install`
Expected: All tests pass

**Step 4: Commit**

```bash
git add justfile
git commit -m "feat: add test-install, fmt-check, and check commands to justfile"
```

---

### Task 7: Update README with CLI install instructions

**Files:**
- Modify: `README.md`

**Step 1: Add CLI section to README**

After the existing "Local HTTPS Dev URLs" section (line 39), add a new section:

```markdown

### CLI (`boop`)

Manage your bookmarks from the terminal:

1. Install: `curl -fsSL https://raw.githubusercontent.com/foundra-build/boopmark/main/install.sh | sh`
2. Configure:
   ```bash
   boop config set-server https://your-boopmark-instance.example.com
   boop config set-key YOUR_API_KEY
   ```
3. Use:
   ```bash
   boop add https://example.com --title "Example" --tags "ref"
   boop list
   boop search "query"
   ```

See `boop --help` for all commands.
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add boop CLI install instructions to README"
```

---

### Task 8: Add CI/CD static assertion tests

**Files:**
- Create: `tests/test_ci_cd.sh`

**Step 1: Write test_ci_cd.sh**

This script performs static assertions against the workflow YAML, justfile, plugin structure, and README to ensure they stay in sync:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PASS=0
FAIL=0
TOTAL=0

pass() {
    PASS=$((PASS + 1))
    TOTAL=$((TOTAL + 1))
    echo "  PASS: $1"
}

fail() {
    FAIL=$((FAIL + 1))
    TOTAL=$((TOTAL + 1))
    echo "  FAIL: $1"
    if [ -n "${2:-}" ]; then
        echo "        $2"
    fi
}

assert_file_exists() {
    if [ -f "$1" ]; then
        pass "$2"
    else
        fail "$2" "file not found: $1"
    fi
}

assert_file_contains() {
    _file="$1"
    _pattern="$2"
    _desc="$3"
    if grep -Eq "$_pattern" "$_file"; then
        pass "$_desc"
    else
        fail "$_desc" "pattern '$_pattern' not found in $_file"
    fi
}

# ============================================================
# Test 1: Release workflow structure
# ============================================================
echo "=== Test 1: Release workflow ==="

RELEASE_YML="$REPO_ROOT/.github/workflows/release.yml"
assert_file_exists "$RELEASE_YML" "release.yml exists"
assert_file_contains "$RELEASE_YML" 'workflow_dispatch' "release.yml is workflow_dispatch"
assert_file_contains "$RELEASE_YML" 'x86_64-apple-darwin' "release.yml has x86_64 macOS target"
assert_file_contains "$RELEASE_YML" 'aarch64-apple-darwin' "release.yml has aarch64 macOS target"
assert_file_contains "$RELEASE_YML" 'x86_64-unknown-linux-gnu' "release.yml has x86_64 Linux target"
assert_file_contains "$RELEASE_YML" 'aarch64-unknown-linux-gnu' "release.yml has aarch64 Linux target"
assert_file_contains "$RELEASE_YML" 'cargo build --release -p boop' "release.yml builds boop crate"
assert_file_contains "$RELEASE_YML" 'cross build --release -p boop' "release.yml cross-builds boop crate"
assert_file_contains "$RELEASE_YML" 'EXPECTED_COUNT=4' "release.yml verifies 4 binaries"
assert_file_contains "$RELEASE_YML" 'gh release create' "release.yml creates GitHub release"

# ============================================================
# Test 2: CI workflow structure
# ============================================================
echo "=== Test 2: CI workflow ==="

CI_YML="$REPO_ROOT/.github/workflows/ci.yml"
assert_file_exists "$CI_YML" "ci.yml exists"
assert_file_contains "$CI_YML" 'cargo fmt -- --check' "ci.yml runs format check"
assert_file_contains "$CI_YML" 'cargo clippy' "ci.yml runs clippy"
assert_file_contains "$CI_YML" 'cargo test -p boop' "ci.yml runs boop tests"
assert_file_contains "$CI_YML" 'test_install.sh' "ci.yml runs install script tests"

# ============================================================
# Test 3: Plugin marketplace structure
# ============================================================
echo "=== Test 3: Plugin marketplace ==="

MARKETPLACE="$REPO_ROOT/.claude-plugin/marketplace.json"
PLUGIN="$REPO_ROOT/.claude-plugin/plugin.json"
SKILL="$REPO_ROOT/skills/boop/SKILL.md"

assert_file_exists "$MARKETPLACE" "marketplace.json exists"
assert_file_exists "$PLUGIN" "plugin.json exists"
assert_file_exists "$SKILL" "SKILL.md exists"

assert_file_contains "$MARKETPLACE" '"name": "boopmark"' "marketplace.json has correct name"
assert_file_contains "$MARKETPLACE" '"name": "boop"' "marketplace.json lists boop plugin"
assert_file_contains "$PLUGIN" '"name": "boop"' "plugin.json has correct name"

# ============================================================
# Test 4: SKILL.md content
# ============================================================
echo "=== Test 4: SKILL.md content ==="

assert_file_contains "$SKILL" 'boop add' "SKILL.md contains 'boop add'"
assert_file_contains "$SKILL" 'boop list' "SKILL.md contains 'boop list'"
assert_file_contains "$SKILL" 'boop search' "SKILL.md contains 'boop search'"
assert_file_contains "$SKILL" 'boop delete' "SKILL.md contains 'boop delete'"
assert_file_contains "$SKILL" 'boop config' "SKILL.md contains 'boop config'"
assert_file_contains "$SKILL" 'install.sh' "SKILL.md references install script"
assert_file_contains "$SKILL" 'Gatekeeper|quarantine' "SKILL.md mentions Gatekeeper/quarantine"
assert_file_contains "$SKILL" 'xattr -cr' "SKILL.md mentions xattr -cr"
assert_file_contains "$SKILL" 'codesign' "SKILL.md mentions codesign"

# ============================================================
# Test 5: Justfile commands
# ============================================================
echo "=== Test 5: Justfile commands ==="

JUSTFILE="$REPO_ROOT/justfile"
assert_file_contains "$JUSTFILE" 'test-install' "justfile has test-install command"
assert_file_contains "$JUSTFILE" 'test_install.sh' "justfile references test_install.sh"

# ============================================================
# Test 6: README content
# ============================================================
echo "=== Test 6: README content ==="

README="$REPO_ROOT/README.md"
assert_file_contains "$README" 'install.sh' "README references install script"
assert_file_contains "$README" 'boop config set-server' "README has config instructions"
assert_file_contains "$README" 'boop add' "README shows boop add usage"

# ============================================================
# Test 7: install.sh references correct repo
# ============================================================
echo "=== Test 7: install.sh repo reference ==="

INSTALL="$REPO_ROOT/install.sh"
assert_file_contains "$INSTALL" 'foundra-build/boopmark' "install.sh references correct GitHub repo"
assert_file_contains "$INSTALL" '__BOOP_INSTALL_MAIN__' "install.sh has sentinel marker"

# ============================================================
# Summary
# ============================================================
echo ""
echo "============================================================"
echo "Results: $PASS passed, $FAIL failed, $TOTAL total"
echo "============================================================"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
```

**Step 2: Run the CI/CD tests**

Run: `sh tests/test_ci_cd.sh`
Expected: All tests pass

**Step 3: Commit**

```bash
git add tests/test_ci_cd.sh
git commit -m "test: add CI/CD and plugin structure static assertions"
```

---

### Task 9: Final verification

**Step 1: Run all install script tests**

Run: `just test-install`
Expected: All tests pass (0 failures)

**Step 2: Run CI/CD static tests**

Run: `sh tests/test_ci_cd.sh`
Expected: All tests pass (0 failures)

**Step 3: Run cargo tests for boop crate**

Run: `cargo test -p boop`
Expected: All tests pass

**Step 4: Verify justfile commands**

Run: `just --list`
Expected: Shows `test-install`, `fmt-check`, `check` among the available commands

**Step 5: Verify all files are committed**

Run: `git status`
Expected: Clean working tree

---

## File Summary

| File | Action | Description |
|------|--------|-------------|
| `install.sh` | Create | Universal installer with platform detection, macOS Gatekeeper handling |
| `tests/test_install.sh` | Create | Comprehensive install script tests (9 test groups) |
| `tests/test_ci_cd.sh` | Create | Static assertions for CI/CD, plugin, README content |
| `.github/workflows/release.yml` | Create | Manual release workflow, 4 platform builds |
| `.github/workflows/ci.yml` | Create | PR/push CI: fmt, clippy, test, install tests |
| `.claude-plugin/marketplace.json` | Create | Plugin marketplace catalog |
| `.claude-plugin/plugin.json` | Create | Plugin manifest |
| `skills/boop/SKILL.md` | Create | Claude Code skill for boop CLI usage |
| `justfile` | Modify | Add test-install, fmt-check, check commands |
| `README.md` | Modify | Add CLI install instructions |
