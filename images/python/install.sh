#!/usr/bin/env bash
# Mino Python layer root-level install script
# Installs system packages only. User-level tools (uv, ruff, pytest)
# are installed via bootstrap — see [user_install] in layer.toml.
#
# Must run as root. Idempotent - safe to run multiple times.
set -euo pipefail

dnf install -y --setopt=install_weak_deps=False python3 python3-devel \
    && dnf clean all \
    && rm -rf /var/cache/dnf

python3 --version
