#!/usr/bin/env bash
#
# Build and test mino-base container image locally.
#
# Language toolchains (TypeScript, Rust) are handled by the layer composition
# system at runtime — see `images/{lang}/install.sh` and `images/{lang}/layer.toml`.
#
# Usage:
#   ./build.sh              # Build base image and run tests
#   ./build.sh --test-only  # Test existing image (skip build)
#   ./build.sh --no-cache   # Fresh build (no docker cache)
#
# Exit codes:
#   0 - All builds and tests passed
#   1 - Build or test failure

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCKER="${DOCKER:-docker}"
BUILD_ARGS=""
TEST_ONLY=false

# Track failures
declare -a FAILURES=()

#------------------------------------------------------------------------------
# Helpers
#------------------------------------------------------------------------------

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

check_tool() {
    local image="$1"
    local tool="$2"
    local cmd="${3:-$tool --version}"

    if $DOCKER run --rm "$image" sh -c "$cmd" > /dev/null 2>&1; then
        log_success "$tool"
        return 0
    else
        log_error "$tool"
        FAILURES+=("$image: $tool")
        return 1
    fi
}

check_env() {
    local image="$1"
    local var="$2"
    local expected="$3"

    local actual
    actual=$($DOCKER run --rm "$image" sh -c "echo \$$var" 2>/dev/null || echo "")

    if [[ "$actual" == "$expected" ]]; then
        log_success "ENV $var=$expected"
        return 0
    else
        log_error "ENV $var expected '$expected', got '$actual'"
        FAILURES+=("$image: ENV $var")
        return 1
    fi
}

check_user() {
    local image="$1"
    local expected="$2"

    local actual
    actual=$($DOCKER run --rm "$image" whoami 2>/dev/null || echo "")

    if [[ "$actual" == "$expected" ]]; then
        log_success "user=$expected"
        return 0
    else
        log_error "user expected '$expected', got '$actual'"
        FAILURES+=("$image: user")
        return 1
    fi
}

check_dir() {
    local image="$1"
    local dir="$2"

    if $DOCKER run --rm "$image" test -d "$dir" 2>/dev/null; then
        log_success "directory $dir exists"
        return 0
    else
        log_error "directory $dir missing"
        FAILURES+=("$image: directory $dir")
        return 1
    fi
}

check_workdir() {
    local image="$1"
    local expected="$2"

    local actual
    actual=$($DOCKER run --rm "$image" pwd 2>/dev/null || echo "")

    if [[ "$actual" == "$expected" ]]; then
        log_success "workdir=$expected"
        return 0
    else
        log_error "workdir expected '$expected', got '$actual'"
        FAILURES+=("$image: workdir")
        return 1
    fi
}

check_path_exists() {
    local image="$1"
    local path="$2"
    local label="$3"

    if $DOCKER run --rm "$image" test -e "$path" 2>/dev/null; then
        log_success "$label"
        return 0
    else
        log_error "$label (path $path missing)"
        FAILURES+=("$image: $label")
        return 1
    fi
}

#------------------------------------------------------------------------------
# Build functions
#------------------------------------------------------------------------------

build_image() {
    local name="$1"
    local context="$2"
    local extra_args="${3:-}"

    log_info "Building $name..."

    if $DOCKER build $BUILD_ARGS $extra_args -t "$name" "$context"; then
        log_success "Built $name"
        return 0
    else
        log_error "Failed to build $name"
        FAILURES+=("build: $name")
        return 1
    fi
}

build_base() {
    build_image "mino-base" "$SCRIPT_DIR/base"
}

#------------------------------------------------------------------------------
# Test functions
#------------------------------------------------------------------------------

test_base() {
    log_section "Testing mino-base"

    echo ""
    log_info "Structure checks:"
    check_user "mino-base" "developer"
    check_workdir "mino-base" "/workspace"
    check_dir "mino-base" "/workspace"
    check_dir "mino-base" "/cache"

    echo ""
    log_info "AI tools:"
    check_tool "mino-base" "claude"

    echo ""
    log_info "Git tools:"
    check_tool "mino-base" "git"
    check_tool "mino-base" "gh"
    check_tool "mino-base" "delta"

    echo ""
    log_info "Search tools:"
    check_tool "mino-base" "rg" "rg --version"
    check_tool "mino-base" "fd"
    check_tool "mino-base" "fzf"

    echo ""
    log_info "View/Edit tools:"
    check_tool "mino-base" "bat"
    check_tool "mino-base" "jq"
    check_tool "mino-base" "nvim" "nvim --version"
    check_tool "mino-base" "yq"
    check_tool "mino-base" "sd"

    echo ""
    log_info "Navigation tools:"
    check_tool "mino-base" "zoxide"

    echo ""
    log_info "File tools:"
    check_tool "mino-base" "eza"
    check_tool "mino-base" "tokei"

    echo ""
    log_info "Network tools:"
    check_tool "mino-base" "curl"
    check_tool "mino-base" "http" "http --version"

    echo ""
    log_info "Runtime:"
    check_tool "mino-base" "node"

    echo ""
    log_info "Shell environment:"
    check_path_exists "mino-base" "/home/developer/.oh-my-zsh" "Oh My Zsh"
    check_path_exists "mino-base" "/home/developer/.oh-my-zsh/custom/plugins/zsh-autosuggestions" "zsh-autosuggestions"
    check_path_exists "mino-base" "/home/developer/.oh-my-zsh/custom/plugins/zsh-history-substring-search" "zsh-history-substring-search"
    check_path_exists "mino-base" "/home/developer/.nvm/nvm.sh" "nvm"

    echo ""
    log_info "Healthcheck:"
    check_tool "mino-base" "mino-healthcheck" "mino-healthcheck"
}

#------------------------------------------------------------------------------
# Main
#------------------------------------------------------------------------------

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Build and test mino-base container image.

Language toolchains (TypeScript, Rust) are handled by the layer composition
system at runtime. See images/{lang}/install.sh and images/{lang}/layer.toml.

Options:
    --test-only     Skip build, only run tests on existing image
    --no-cache      Build without using cache
    -h, --help      Show this help

Examples:
    $(basename "\$0")              # Build and test base image
    $(basename "\$0") --test-only  # Test existing image
    $(basename "\$0") --no-cache   # Fresh build
EOF
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --test-only)
                TEST_ONLY=true
                shift
                ;;
            --no-cache)
                BUILD_ARGS="--no-cache"
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
        esac
    done
}

main() {
    parse_args "$@"

    log_section "Mino Base Image Build & Test"
    log_info "Docker command: $DOCKER"
    log_info "Test only: $TEST_ONLY"
    log_info "Build args: ${BUILD_ARGS:-none}"

    # Build phase
    if [[ "$TEST_ONLY" == "false" ]]; then
        log_section "Build Phase"
        build_base || true
    fi

    # Test phase
    log_section "Test Phase"

    if ! $DOCKER image inspect "mino-base" > /dev/null 2>&1; then
        log_error "Image mino-base not found - skipping tests"
        FAILURES+=("missing: mino-base")
    else
        test_base
    fi

    # Summary
    log_section "Summary"

    if [[ ${#FAILURES[@]} -eq 0 ]]; then
        log_success "All builds and tests passed!"
        exit 0
    else
        log_error "Failures:"
        for f in "${FAILURES[@]}"; do
            echo "  - $f"
        done
        exit 1
    fi
}

main "$@"
