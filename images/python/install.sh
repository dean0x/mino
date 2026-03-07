#!/usr/bin/env bash
# Mino Python layer install script
# Installs: python3, python3-devel, uv, ruff, pytest
#
# Must run as root. Idempotent - safe to run multiple times.
set -euo pipefail

# Install system Python 3 + development headers (for building C extensions)
dnf install -y --setopt=install_weak_deps=False python3 python3-devel
dnf clean all
rm -rf /var/cache/dnf

# Install uv to /usr/local/bin (latest version via official installer)
if ! command -v uv &>/dev/null; then
    curl -LsSf --proto '=https' --tlsv1.2 https://astral.sh/uv/install.sh | env CARGO_HOME=/tmp/uv-install UV_INSTALL_DIR=/usr/local/bin sh
    rm -rf /tmp/uv-install
fi

# Install global dev tools via uv tool (isolated environments per tool)
export UV_TOOL_DIR="${UV_TOOL_DIR:-/opt/uv-tools}"
export UV_TOOL_BIN_DIR="${UV_TOOL_BIN_DIR:-/opt/uv-tools/bin}"
mkdir -p "${UV_TOOL_DIR}" "${UV_TOOL_BIN_DIR}"

uv tool install ruff
uv tool install pytest

# Fix permissions for developer user
chown -R developer:developer "${UV_TOOL_DIR}"
chmod -R a+rX "${UV_TOOL_DIR}"

# Verify installations
python3 --version
uv --version
ruff --version
pytest --version
