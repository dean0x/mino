#!/usr/bin/env bash
#
# Build and test minotaur container images locally.
#
# Usage:
#   ./build.sh              # Build all images and run tests
#   ./build.sh --test-only  # Test existing images (skip build)
#   ./build.sh --no-cache   # Fresh build (no docker cache)
#   ./build.sh base         # Build/test only base image
#   ./build.sh typescript   # Build/test only typescript image
#   ./build.sh rust         # Build/test only rust image
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
TARGET_IMAGE=""

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
    build_image "minotaur-base" "$SCRIPT_DIR/base"
}

build_typescript() {
    build_image "minotaur-typescript" "$SCRIPT_DIR/typescript" "--build-arg BASE_IMAGE=minotaur-base"
}

build_rust() {
    build_image "minotaur-rust" "$SCRIPT_DIR/rust" "--build-arg BASE_IMAGE=minotaur-base"
}

#------------------------------------------------------------------------------
# Test functions
#------------------------------------------------------------------------------

test_base() {
    log_section "Testing minotaur-base"

    echo ""
    log_info "Structure checks:"
    check_user "minotaur-base" "developer"
    check_workdir "minotaur-base" "/workspace"
    check_dir "minotaur-base" "/workspace"
    check_dir "minotaur-base" "/cache"

    echo ""
    log_info "AI tools:"
    check_tool "minotaur-base" "claude"

    echo ""
    log_info "Git tools:"
    check_tool "minotaur-base" "git"
    check_tool "minotaur-base" "gh"
    check_tool "minotaur-base" "delta"

    echo ""
    log_info "Search tools:"
    check_tool "minotaur-base" "rg" "rg --version"
    check_tool "minotaur-base" "fd"
    check_tool "minotaur-base" "fzf"

    echo ""
    log_info "View/Edit tools:"
    check_tool "minotaur-base" "bat"
    check_tool "minotaur-base" "jq"
    check_tool "minotaur-base" "nvim" "nvim --version"

    echo ""
    log_info "Navigation tools:"
    check_tool "minotaur-base" "zoxide"
    check_tool "minotaur-base" "mcfly"

    echo ""
    log_info "Network tools:"
    check_tool "minotaur-base" "curl"
    check_tool "minotaur-base" "http" "http --version"

    echo ""
    log_info "Runtime:"
    check_tool "minotaur-base" "node"
}

test_typescript() {
    log_section "Testing minotaur-typescript"

    echo ""
    log_info "Structure checks:"
    check_user "minotaur-typescript" "developer"
    check_workdir "minotaur-typescript" "/workspace"
    check_env "minotaur-typescript" "PNPM_HOME" "/cache/pnpm"
    check_env "minotaur-typescript" "npm_config_cache" "/cache/npm"

    echo ""
    log_info "TypeScript tools:"
    check_tool "minotaur-typescript" "pnpm"
    check_tool "minotaur-typescript" "tsx"
    check_tool "minotaur-typescript" "tsc" "tsc --version"
    check_tool "minotaur-typescript" "ncu" "ncu --version"

    echo ""
    log_info "Inherited from base:"
    check_tool "minotaur-typescript" "claude"
    check_tool "minotaur-typescript" "node"
}

test_rust() {
    log_section "Testing minotaur-rust"

    echo ""
    log_info "Structure checks:"
    check_user "minotaur-rust" "developer"
    check_workdir "minotaur-rust" "/workspace"
    check_env "minotaur-rust" "CARGO_HOME" "/cache/cargo"
    check_env "minotaur-rust" "RUSTUP_HOME" "/opt/rustup"

    echo ""
    log_info "Rust tools:"
    check_tool "minotaur-rust" "rustc"
    check_tool "minotaur-rust" "cargo"
    check_tool "minotaur-rust" "rustfmt"
    check_tool "minotaur-rust" "clippy" "cargo clippy --version"
    check_tool "minotaur-rust" "bacon"
    check_tool "minotaur-rust" "cargo-add" "cargo add --help > /dev/null"
    check_tool "minotaur-rust" "cargo-outdated" "cargo outdated --version"

    echo ""
    log_info "Inherited from base:"
    check_tool "minotaur-rust" "claude"
    check_tool "minotaur-rust" "node"
}

#------------------------------------------------------------------------------
# Main
#------------------------------------------------------------------------------

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS] [IMAGE]

Build and test minotaur container images.

Options:
    --test-only     Skip build, only run tests on existing images
    --no-cache      Build without using cache
    -h, --help      Show this help

Images:
    base            Build/test only base image
    typescript      Build/test only typescript image
    rust            Build/test only rust image
    (none)          Build/test all images in order

Examples:
    $(basename "$0")              # Build and test all images
    $(basename "$0") --test-only  # Test existing images
    $(basename "$0") --no-cache   # Fresh build
    $(basename "$0") typescript   # Only typescript image
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
            base|typescript|rust)
                TARGET_IMAGE="$1"
                shift
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

    log_section "Minotaur Image Build & Test"
    log_info "Docker command: $DOCKER"
    log_info "Test only: $TEST_ONLY"
    log_info "Build args: ${BUILD_ARGS:-none}"
    log_info "Target image: ${TARGET_IMAGE:-all}"

    # Determine which images to process
    local images=()
    if [[ -z "$TARGET_IMAGE" ]]; then
        images=(base typescript rust)
    else
        images=("$TARGET_IMAGE")
    fi

    # Build phase
    if [[ "$TEST_ONLY" == "false" ]]; then
        log_section "Build Phase"

        for img in "${images[@]}"; do
            # For language images, ensure base is built first
            if [[ "$img" != "base" && ! " ${images[*]} " =~ " base " ]]; then
                if ! $DOCKER image inspect minotaur-base > /dev/null 2>&1; then
                    log_warn "Base image required for $img - building base first"
                    build_base || true
                fi
            fi

            case $img in
                base) build_base || true ;;
                typescript) build_typescript || true ;;
                rust) build_rust || true ;;
            esac
        done
    fi

    # Test phase
    log_section "Test Phase"

    for img in "${images[@]}"; do
        # Verify image exists before testing
        if ! $DOCKER image inspect "minotaur-$img" > /dev/null 2>&1; then
            log_error "Image minotaur-$img not found - skipping tests"
            FAILURES+=("missing: minotaur-$img")
            continue
        fi

        case $img in
            base) test_base ;;
            typescript) test_typescript ;;
            rust) test_rust ;;
        esac
    done

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
