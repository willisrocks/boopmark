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
for cmd in sh uname mktemp chmod mkdir mv cp rm cat sed grep printf echo test tr cut; do
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
    if grep -Eq -- "$_pattern" "$_file"; then
        pass "$_desc"
    else
        fail "$_desc" "pattern '$_pattern' not found in $_file"
    fi
}

assert_file_not_contains() {
    _file="$1"
    _pattern="$2"
    _desc="$3"
    if grep -Eq -- "$_pattern" "$_file"; then
        fail "$_desc" "pattern '$_pattern' unexpectedly found in $_file"
    else
        pass "$_desc"
    fi
}

assert_output_contains() {
    _output="$1"
    _pattern="$2"
    _desc="$3"
    if printf '%s' "$_output" | grep -Eq -- "$_pattern"; then
        pass "$_desc"
    else
        fail "$_desc" "pattern '$_pattern' not found in command output"
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
# Test 10: SKILL.md contains key commands
# ============================================================
echo "=== Test 10: SKILL.md contains key commands ==="

SKILL_MD="$REPO_ROOT/skills/boop/SKILL.md"
BOOP_HELP="$(cargo run -q -p boop -- --help)"
BOOP_ADD_HELP="$(cargo run -q -p boop -- help add)"
BOOP_EDIT_HELP="$(cargo run -q -p boop -- help edit)"
BOOP_SUGGEST_HELP="$(cargo run -q -p boop -- help suggest)"
BOOP_UPGRADE_HELP="$(cargo run -q -p boop -- help upgrade)"

assert_output_contains "$BOOP_HELP" '^[[:space:]]+add([[:space:]]|$)' "CLI help contains 'add'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+list([[:space:]]|$)' "CLI help contains 'list'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+search([[:space:]]|$)' "CLI help contains 'search'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+edit([[:space:]]|$)' "CLI help contains 'edit'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+suggest([[:space:]]|$)' "CLI help contains 'suggest'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+delete([[:space:]]|$)' "CLI help contains 'delete'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+upgrade([[:space:]]|$)' "CLI help contains 'upgrade'"
assert_output_contains "$BOOP_HELP" '^[[:space:]]+config([[:space:]]|$)' "CLI help contains 'config'"
assert_output_contains "$BOOP_ADD_HELP" '--description <DESCRIPTION>' "CLI add help contains '--description'"
assert_output_contains "$BOOP_ADD_HELP" '--suggest' "CLI add help contains '--suggest'"
assert_output_contains "$BOOP_EDIT_HELP" '--description <DESCRIPTION>' "CLI edit help contains '--description'"
assert_output_contains "$BOOP_EDIT_HELP" '--suggest' "CLI edit help contains '--suggest'"
assert_output_contains "$BOOP_SUGGEST_HELP" 'Usage: boop suggest <URL>' "CLI suggest help contains suggest usage"
assert_output_contains "$BOOP_UPGRADE_HELP" 'Usage: boop upgrade' "CLI upgrade help contains upgrade usage"

assert_file_contains "$SKILL_MD" 'boop add' "SKILL.md contains 'boop add'"
assert_file_contains "$SKILL_MD" 'boop list' "SKILL.md contains 'boop list'"
assert_file_contains "$SKILL_MD" 'boop search' "SKILL.md contains 'boop search'"
assert_file_contains "$SKILL_MD" 'boop edit' "SKILL.md contains 'boop edit'"
assert_file_contains "$SKILL_MD" 'boop suggest' "SKILL.md contains 'boop suggest'"
assert_file_contains "$SKILL_MD" 'boop delete' "SKILL.md contains 'boop delete'"
assert_file_contains "$SKILL_MD" 'boop upgrade' "SKILL.md contains 'boop upgrade'"
assert_file_contains "$SKILL_MD" 'boop config' "SKILL.md contains 'boop config'"
assert_file_contains "$SKILL_MD" '--description' "SKILL.md contains '--description'"
assert_file_contains "$SKILL_MD" '--suggest' "SKILL.md contains '--suggest'"
assert_file_contains "$SKILL_MD" 'LLM' "SKILL.md mentions LLM usage"
assert_file_contains "$SKILL_MD" 'boop edit <id>' "SKILL.md matches CLI edit placeholder"
assert_file_contains "$SKILL_MD" 'boop delete <id>' "SKILL.md matches CLI delete placeholder"
assert_file_contains "$SKILL_MD" 'boop edit <bookmark-uuid>' "SKILL.md shows UUID-shaped edit example"
assert_file_contains "$SKILL_MD" 'install.sh' "SKILL.md references install script"

# ============================================================
# Test 11: SKILL.md Gatekeeper common issue
# ============================================================
echo "=== Test 11: SKILL.md Gatekeeper common issue ==="

assert_file_contains "$SKILL_MD" 'Gatekeeper|quarantine' "SKILL.md mentions Gatekeeper/quarantine"
assert_file_contains "$SKILL_MD" 'xattr -cr' "SKILL.md mentions xattr -cr"
assert_file_contains "$SKILL_MD" 'codesign' "SKILL.md mentions codesign"

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
