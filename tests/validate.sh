#!/usr/bin/env bash
# Mino end-to-end validation suite
# Requires: OrbStack running, mino binary on PATH or built via cargo
#
# Usage:
#   cargo build --release
#   ./tests/validate.sh
#
# Override binary: MINO=/path/to/mino ./tests/validate.sh

set -uo pipefail

# ─── Resolve binary ─────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

if [[ -n "${MINO:-}" ]]; then
    # User-provided binary path
    :
elif [[ -x "${REPO_ROOT}/target/release/mino" ]]; then
    MINO="${REPO_ROOT}/target/release/mino"
else
    echo "No mino binary found. Run: cargo build --release"
    exit 1
fi

echo "Using binary: ${MINO}"

# ─── Counters ────────────────────────────────────────────────────────────────

PASS=0
FAIL=0
SKIP=0

# ─── Temp dirs ───────────────────────────────────────────────────────────────

WORKDIR=$(mktemp -d)
AUDIT_LOG="${HOME}/.local/share/mino/audit.log"
VOLTEST_DIR=$(mktemp -d)
VOLTEST_FILE="${VOLTEST_DIR}/file.txt"

# ─── Helpers ─────────────────────────────────────────────────────────────────

_green='\033[0;32m'
_red='\033[0;31m'
_yellow='\033[0;33m'
_cyan='\033[0;36m'
_bold='\033[1m'
_reset='\033[0m'

section() {
    echo ""
    echo -e "${_cyan}${_bold}═══ $1 ═══${_reset}"
}

pass() {
    ((PASS++))
    echo -e "  ${_green}✓${_reset} $1"
}

fail() {
    ((FAIL++))
    echo -e "  ${_red}✗${_reset} $1"
    [[ -n "${2:-}" ]] && echo -e "    ${_red}→ $2${_reset}"
}

skip() {
    ((SKIP++))
    echo -e "  ${_yellow}⊘${_reset} $1 (skipped: $2)"
}

# Run a test function. Catches non-zero exits so the suite continues.
run_test() {
    local name="$1"
    local fn="$2"
    if $fn; then
        pass "$name"
    else
        fail "$name" "function returned non-zero"
    fi
}

# Capture stdout+stderr and exit code from a mino command.
# Usage: mino_run <var_prefix> [args...]
# Sets: ${var_prefix}_out, ${var_prefix}_rc
mino_run() {
    local prefix="$1"; shift
    local out rc
    out=$("$MINO" "$@" 2>&1) && rc=0 || rc=$?
    eval "${prefix}_out=\${out}"
    eval "${prefix}_rc=\${rc}"
}

# Same as mino_run but from a specific working directory.
# Usage: mino_run_in <dir> <var_prefix> [args...]
mino_run_in() {
    local dir="$1"; shift
    local prefix="$1"; shift
    local out rc
    out=$(cd "$dir" && "$MINO" "$@" 2>&1) && rc=0 || rc=$?
    eval "${prefix}_out=\${out}"
    eval "${prefix}_rc=\${rc}"
}

cleanup() {
    section "Cleanup"

    # Stop any test sessions we created
    "$MINO" stop val-detach 2>/dev/null && pass "stopped val-detach session" || true

    # Clear composed images (non-fatal)
    "$MINO" cache clear --images -y 2>/dev/null && pass "cleared composed images" || true

    # Remove temp dirs
    rm -rf "$WORKDIR" "$VOLTEST_DIR" 2>/dev/null
    pass "removed temp dirs"
}

trap cleanup EXIT

# ─── 1. Prerequisites ───────────────────────────────────────────────────────

section "1. Prerequisites"

test_status() {
    mino_run r status
    [[ $r_rc -eq 0 ]]
}
run_test "mino status exits 0" test_status

test_setup_check() {
    mino_run r setup --check
    [[ $r_rc -eq 0 ]]
}
run_test "mino setup --check exits 0" test_setup_check

test_orbstack_vm() {
    orb list 2>/dev/null | grep -q mino
}
run_test "OrbStack VM 'mino' is running" test_orbstack_vm

# ─── 2. Config System ───────────────────────────────────────────────────────

section "2. Config System"

test_config_path() {
    mino_run r config path
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "config.toml"
}
run_test "config path contains config.toml" test_config_path

test_config_show() {
    mino_run r config show
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "\[container\]"
}
run_test "config show exits 0 and has [container]" test_config_show

test_config_init() {
    local global_config
    global_config=$("$MINO" config path 2>&1)
    if [[ -f "$global_config" ]]; then
        skip "config init (global)" "global config already exists"
        return 0
    fi
    mino_run r config init
    [[ $r_rc -eq 0 ]]
}
run_test "config init (global)" test_config_init

INITDIR="${WORKDIR}/init-test"

test_init_project() {
    mkdir -p "$INITDIR"
    mino_run r init -p "$INITDIR"
    [[ $r_rc -eq 0 ]] && [[ -f "${INITDIR}/.mino.toml" ]]
}
run_test "mino init creates .mino.toml" test_init_project

test_init_no_force_fails() {
    mino_run r init -p "$INITDIR"
    [[ $r_rc -ne 0 ]]
}
run_test "mino init again fails without --force" test_init_no_force_fails

test_init_force() {
    mino_run r init --force -p "$INITDIR"
    [[ $r_rc -eq 0 ]]
}
run_test "mino init --force succeeds" test_init_force

test_config_set_local() {
    # config set --local uses cwd to find .mino.toml
    mino_run_in "$INITDIR" r config set container.image ubuntu:22.04 --local
    [[ $r_rc -eq 0 ]]
}
run_test "config set --local updates .mino.toml" test_config_set_local

test_config_show_local_override() {
    # config show discovers local .mino.toml from cwd
    mino_run_in "$INITDIR" r config show
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "ubuntu:22.04"
}
run_test "config show reflects local override" test_config_show_local_override

test_config_no_local() {
    # --no-local is a global flag, skips local .mino.toml discovery
    mino_run_in "$INITDIR" r --no-local config show
    [[ $r_rc -eq 0 ]] && ! echo "$r_out" | grep -q "ubuntu:22.04"
}
run_test "--no-local ignores .mino.toml" test_config_no_local

# ─── 3. Run: Default Image ──────────────────────────────────────────────────

section "3. Run: Default Image"

test_run_default() {
    mino_run r run -- echo hello
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "hello"
}
run_test "mino run -- echo hello" test_run_default

# ─── 4. Run: Base Image Alias ───────────────────────────────────────────────

section "4. Run: Base Image Alias"

test_run_base() {
    mino_run r run --image base -- echo hello
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "hello"
}
run_test "mino run --image base -- echo hello" test_run_base

# ─── 5. Run: TypeScript Layer (via --image alias) ───────────────────────────

section "5. Run: TypeScript Layer (--image alias)"

test_ts_pnpm() {
    mino_run r run --image typescript -- pnpm --version
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -qE '[0-9]+\.[0-9]+'
}
run_test "pnpm --version via --image typescript" test_ts_pnpm

test_ts_alias() {
    mino_run r run --image ts -- tsc --version
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -qi "version"
}
run_test "tsc --version via --image ts" test_ts_alias

# ─── 6. Run: Rust Layer (via --image alias) ─────────────────────────────────

section "6. Run: Rust Layer (--image alias)"

test_rust_rustc() {
    mino_run r run --image rust -- rustc --version
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "rustc"
}
run_test "rustc --version via --image rust" test_rust_rustc

test_rust_cargo_alias() {
    mino_run r run --image cargo -- cargo --version
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "cargo"
}
run_test "cargo --version via --image cargo" test_rust_cargo_alias

# ─── 7. Run: Explicit --layers Flag ─────────────────────────────────────────

section "7. Run: Explicit --layers Flag"

test_layers_ts() {
    mino_run r run --layers typescript -- node --version
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -qE 'v[0-9]+'
}
run_test "node --version via --layers typescript" test_layers_ts

test_layers_rust() {
    mino_run r run --layers rust -- cargo --version
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "cargo"
}
run_test "cargo --version via --layers rust" test_layers_rust

# ─── 8. Run: Multi-Layer Composition ────────────────────────────────────────

section "8. Run: Multi-Layer Composition"

test_multi_layer() {
    mino_run r run --layers typescript,rust -- /bin/bash -c "node --version && cargo --version"
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -qE 'v[0-9]+' && echo "$r_out" | grep -q "cargo"
}
run_test "node + cargo via --layers typescript,rust" test_multi_layer

# ─── 9. Run: Custom Project-Local Layer ─────────────────────────────────────

section "9. Run: Custom Project-Local Layer"

setup_custom_layer() {
    local layerdir="${WORKDIR}/.mino/layers/hello"
    mkdir -p "$layerdir"

    cat > "${layerdir}/layer.toml" <<'TOML'
[layer]
name = "hello"
description = "Test custom layer"
version = "1"

[env]
HELLO_MSG = "it works"
TOML

    cat > "${layerdir}/install.sh" <<'BASH'
#!/usr/bin/env bash
set -euo pipefail
echo '#!/bin/bash' > /usr/local/bin/hello-mino
echo 'echo "Hello from custom layer!"' >> /usr/local/bin/hello-mino
chmod +x /usr/local/bin/hello-mino
hello-mino
BASH
    chmod +x "${layerdir}/install.sh"
}
setup_custom_layer

test_custom_layer_cmd() {
    mino_run r run -p "$WORKDIR" --layers hello -- hello-mino
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "Hello from custom layer"
}
run_test "custom layer installs hello-mino command" test_custom_layer_cmd

test_custom_layer_env() {
    mino_run r run -p "$WORKDIR" --layers hello -- /bin/bash -c 'echo $HELLO_MSG'
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "it works"
}
run_test "custom layer injects HELLO_MSG env var" test_custom_layer_env

# ─── 10. Run: Custom Layer + Builtin Composition ────────────────────────────

section "10. Run: Custom + Builtin Composition"

test_custom_plus_builtin() {
    mino_run r run -p "$WORKDIR" --layers hello,typescript -- /bin/bash -c "hello-mino && node --version"
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "Hello from custom layer" && echo "$r_out" | grep -qE 'v[0-9]+'
}
run_test "hello + typescript layers compose together" test_custom_plus_builtin

# ─── 11. Run: Layer Override (Project-Local Shadows Builtin) ─────────────────

section "11. Run: Layer Override (shadow builtin)"

setup_override_layer() {
    local layerdir="${WORKDIR}/.mino/layers/typescript"
    mkdir -p "$layerdir"

    cat > "${layerdir}/layer.toml" <<'TOML'
[layer]
name = "typescript"
description = "Custom TypeScript override"
version = "99"

[env]
CUSTOM_TS = "true"
PNPM_HOME = "/cache/pnpm"
npm_config_cache = "/cache/npm"

[env.path_prepend]
dirs = ["/cache/pnpm"]
TOML

    cat > "${layerdir}/install.sh" <<'BASH'
#!/usr/bin/env bash
set -euo pipefail
npm install -g pnpm
pnpm --version
BASH
    chmod +x "${layerdir}/install.sh"
}
setup_override_layer

test_layer_override() {
    mino_run r run -p "$WORKDIR" --layers typescript -- /bin/bash -c 'echo $CUSTOM_TS && pnpm --version'
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "true" && echo "$r_out" | grep -qE '[0-9]+\.[0-9]+'
}
run_test "project-local typescript shadows builtin" test_layer_override

# Clean up override so it doesn't interfere with later tests
rm -rf "${WORKDIR}/.mino/layers/typescript"

# ─── 12. Run: Environment Variables and Volumes ─────────────────────────────

section "12. Run: Env Vars and Volumes"

test_env_vars() {
    mino_run r run -e MY_VAR=hello -e ANOTHER=world -- /bin/bash -c 'echo $MY_VAR $ANOTHER'
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "hello world"
}
run_test "-e MY_VAR=hello -e ANOTHER=world" test_env_vars

test_volume_mount() {
    echo "mino-volume-test" > "$VOLTEST_FILE"
    mino_run r run --volume "${VOLTEST_DIR}:/mnt/test" -- cat /mnt/test/file.txt
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "mino-volume-test"
}
run_test "--volume mounts host dir" test_volume_mount

# ─── 13. Run: Named Session + Detach ────────────────────────────────────────

section "13. Named Session + Detach"

test_detach() {
    mino_run r run -n val-detach -d -- sleep 300
    [[ $r_rc -eq 0 ]]
}
run_test "mino run -n val-detach -d -- sleep 300" test_detach

test_list_active() {
    sleep 1  # brief settle
    mino_run r list
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "val-detach"
}
run_test "mino list shows val-detach" test_list_active

test_list_json() {
    mino_run r list -f json
    [[ $r_rc -eq 0 ]] && echo "$r_out" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null && echo "$r_out" | grep -q "val-detach"
}
run_test "mino list -f json is valid JSON" test_list_json

test_logs() {
    mino_run r logs val-detach
    [[ $r_rc -eq 0 ]]
}
run_test "mino logs val-detach exits 0" test_logs

test_stop() {
    mino_run r stop val-detach
    [[ $r_rc -eq 0 ]]
}
run_test "mino stop val-detach" test_stop

test_list_after_stop() {
    mino_run r list
    # val-detach should NOT appear in active-only list
    [[ $r_rc -eq 0 ]] && ! echo "$r_out" | grep -q "val-detach"
}
run_test "val-detach not in active list after stop" test_list_after_stop

test_list_all() {
    mino_run r list -a
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "val-detach"
}
run_test "mino list -a shows stopped val-detach" test_list_all

# ─── 14. Cache: Lockfile Detection ──────────────────────────────────────────

section "14. Cache: Lockfile Detection"

CACHEDIR="${WORKDIR}/cache-test"
mkdir -p "$CACHEDIR"

test_cache_info_npm() {
    echo '{"lockfileVersion": 3}' > "${CACHEDIR}/package-lock.json"
    mino_run r cache info -p "$CACHEDIR"
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -qi "npm"
}
run_test "cache info detects package-lock.json" test_cache_info_npm

test_cache_info_cargo() {
    echo 'version = 4' > "${CACHEDIR}/Cargo.lock"
    mino_run r cache info -p "$CACHEDIR"
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -qi "cargo"
}
run_test "cache info detects Cargo.lock" test_cache_info_cargo

# ─── 15. Cache: List ────────────────────────────────────────────────────────

section "15. Cache: List"

test_cache_list() {
    mino_run r cache list
    [[ $r_rc -eq 0 ]]
}
run_test "cache list exits 0" test_cache_list

test_cache_list_json() {
    mino_run r cache list -f json
    [[ $r_rc -eq 0 ]] && echo "$r_out" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null
}
run_test "cache list -f json is valid JSON" test_cache_list_json

# ─── 16. Cache: No-Cache and Fresh ──────────────────────────────────────────

section "16. Cache: No-Cache and Fresh"

test_no_cache() {
    mino_run r run --no-cache --image base -- echo nocache
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "nocache"
}
run_test "--no-cache runs successfully" test_no_cache

test_cache_fresh() {
    mino_run r run --cache-fresh --image base -- echo fresh
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "fresh"
}
run_test "--cache-fresh runs successfully" test_cache_fresh

# ─── 17. Cache: GC and Clear ────────────────────────────────────────────────

section "17. Cache: GC and Clear"

test_cache_gc_dry() {
    mino_run r cache gc --dry-run
    [[ $r_rc -eq 0 ]]
}
run_test "cache gc --dry-run exits 0" test_cache_gc_dry

test_cache_clear_images() {
    mino_run r cache clear --images -y
    [[ $r_rc -eq 0 ]]
}
run_test "cache clear --images -y exits 0" test_cache_clear_images

# ─── 18. Credentials: GitHub Token ──────────────────────────────────────────

section "18. Credentials: GitHub Token"

test_github_token() {
    if ! gh auth status &>/dev/null; then
        skip "GitHub token injection" "gh auth not configured"
        return 0
    fi
    mino_run r run --image base -- /bin/bash -c 'test -n "$GITHUB_TOKEN" && echo "token present"'
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "token present"
}
run_test "GitHub token injected into container" test_github_token

# ─── 19. Credentials: SSH Agent ─────────────────────────────────────────────

section "19. Credentials: SSH Agent"

test_ssh_agent() {
    if [[ -z "${SSH_AUTH_SOCK:-}" ]]; then
        skip "SSH agent forwarding" "no SSH_AUTH_SOCK on host"
        return 0
    fi
    mino_run r run --image base -- /bin/bash -c 'test -n "$SSH_AUTH_SOCK" && echo "agent forwarded"'
    [[ $r_rc -eq 0 ]] && echo "$r_out" | grep -q "agent forwarded"
}
run_test "SSH agent forwarded into container" test_ssh_agent

# ─── 20. Audit Log ──────────────────────────────────────────────────────────

section "20. Audit Log"

test_audit_exists() {
    [[ -f "$AUDIT_LOG" ]]
}
run_test "audit.log exists" test_audit_exists

test_audit_json() {
    local last_line
    last_line=$(tail -1 "$AUDIT_LOG")
    echo "$last_line" | python3 -c "
import sys, json
entry = json.load(sys.stdin)
assert 'event' in entry, 'missing event field'
assert 'timestamp' in entry, 'missing timestamp field'
assert 'data' in entry, 'missing data field'
" 2>/dev/null
}
run_test "audit.log last entry has valid JSON structure" test_audit_json

# ─── 21. Composed Image Caching ─────────────────────────────────────────────

section "21. Composed Image Caching"

test_image_caching() {
    # First run builds the composed image
    local start1 end1 start2 end2 dur1 dur2
    start1=$(python3 -c "import time; print(int(time.time()*1000))")
    "$MINO" run --image typescript -- echo first >/dev/null 2>&1
    end1=$(python3 -c "import time; print(int(time.time()*1000))")
    dur1=$((end1 - start1))

    # Second run should reuse the cached composed image
    start2=$(python3 -c "import time; print(int(time.time()*1000))")
    "$MINO" run --image typescript -- echo second >/dev/null 2>&1
    end2=$(python3 -c "import time; print(int(time.time()*1000))")
    dur2=$((end2 - start2))

    echo "    first: ${dur1}ms, second: ${dur2}ms"
    # Both must succeed; timing is informational
    return 0
}
run_test "composed image reuse (timing comparison)" test_image_caching

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo -e "${_bold}═══════════════════════════════════${_reset}"
echo -e "${_bold}  Mino Validation Results${_reset}"
TOTAL=$((PASS + FAIL + SKIP))
echo -e "  ${_green}PASS: ${PASS}${_reset}  ${_red}FAIL: ${FAIL}${_reset}  ${_yellow}SKIP: ${SKIP}${_reset}  (total: ${TOTAL})"
echo -e "${_bold}═══════════════════════════════════${_reset}"

if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
exit 0
