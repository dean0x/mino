#!/usr/bin/env bash
# Mino Rust layer install script
# Installs: rustup, stable toolchain, rustfmt, clippy, cargo-binstall,
#           bacon, cargo-edit, cargo-outdated, cargo-nextest, sccache
#
# Must run as root. Idempotent - safe to run multiple times.
set -euo pipefail

RUSTUP_HOME="${RUSTUP_HOME:-/opt/rustup}"
CARGO_HOME="${CARGO_HOME:-/opt/cargo}"
export RUSTUP_HOME CARGO_HOME
export PATH="${CARGO_HOME}/bin:${PATH}"

# Install rustup + stable toolchain
if ! command -v rustup &>/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --default-toolchain stable --profile minimal
fi

# Ensure components
rustup component add rustfmt clippy

# Install cargo-binstall for fast binary downloads
if ! command -v cargo-binstall &>/dev/null; then
    curl -L --proto '=https' --tlsv1.2 -sSf \
        https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
fi

# Install tools via binstall (idempotent - skips already installed)
cargo binstall -y bacon cargo-edit cargo-outdated cargo-nextest sccache

# Fix permissions for shared access
chmod -R a+rX "${RUSTUP_HOME}" "${CARGO_HOME}"
chown -R developer:developer "${CARGO_HOME}" "${RUSTUP_HOME}"

# Verify installations
rustc --version
cargo --version
sccache --version
