#!/usr/bin/env bash
# Mino TypeScript layer install script
# Installs: pnpm, tsx, typescript, npm-check-updates, biome, turbo, vite
#
# Must run as root. Idempotent - safe to run multiple times.
set -euo pipefail

# Install global npm packages (npm install -g is idempotent)
npm install -g pnpm tsx typescript npm-check-updates @biomejs/biome turbo vite

# Clean npm cache to reduce image size
npm cache clean --force
