# Mino Container Images

## Architecture

Mino uses a single pre-built base image (`mino-base`) combined with a layer composition system for language toolchains.

```
┌─────────────────────────────────────────────────────────┐
│                    mino-base (GHCR)                     │
│  Fedora 43 + Node 22 LTS + tools + claude-code         │
│  Oh My Zsh + autosuggestions + history-substring-search │
│  nvm + eza + sd + yq + tokei                           │
└─────────────────────────────────────────────────────────┘
                          │
            Layer composition at runtime
          ┌───────────────┴───────────────┐
          ▼                               ▼
┌─────────────────────┐       ┌─────────────────────┐
│  typescript layer   │       │    rust layer        │
│  pnpm, tsx, tsc     │       │  cargo, clippy       │
│  biome, turbo, vite │       │  nextest, sccache    │
└─────────────────────┘       └─────────────────────┘
```

Language toolchains are **not** pre-built GHCR images. Instead, they are composed at runtime using `install.sh` scripts on top of `mino-base`. This enables multi-toolchain composition (`--layers typescript,rust`) and eliminates CI flakes from language image builds.

## Quick Start

```bash
# Use aliases with mino (triggers layer composition)
mino run --image typescript -- claude
mino run --image rust -- claude

# Compose multiple toolchains
mino run --layers typescript,rust -- claude

# Base image only
mino run --image base -- claude
```

## Image Aliases

| Alias | Behavior |
|-------|----------|
| `typescript`, `ts`, `node` | Layer composition (TypeScript toolchain on `mino-base`) |
| `rust`, `cargo` | Layer composition (Rust toolchain on `mino-base`) |
| `base` | Direct pull of `ghcr.io/dean0x/mino-base:latest` |

## Tool Inventory

### Base Image (`mino-base`)

All layers inherit these tools.

| Category | Tools | Notes |
|----------|-------|-------|
| **AI** | claude-code | `@anthropic-ai/claude-code` CLI |
| **Git** | git, gh, delta | delta for syntax-highlighted diffs |
| **Search** | ripgrep (rg), fd-find (fd), fzf | Modern grep/find replacements |
| **View/Edit** | bat, jq, yq, sd | Syntax highlighting, JSON/YAML processing, modern sed |
| **Code analysis** | tokei | Code statistics by language |
| **File listing** | eza | Modern ls + tree replacement |
| **Edit** | neovim | Modern vim |
| **Navigate** | zoxide | Smart cd with frecency ranking |
| **Shell** | zsh, Oh My Zsh, fzf | Autosuggestions, history-substring-search, fzf Ctrl+R history search |
| **Node management** | nvm | Node Version Manager (system Node 22 LTS as fallback) |
| **Network** | curl, wget, httpie, ssh | HTTP testing and SSH |
| **Runtime** | Node.js 22 LTS | Required for Claude Code |

### TypeScript Layer

Installed via `images/typescript/install.sh`, configured via `images/typescript/layer.toml`.

| Tool | Version | Description |
|------|---------|-------------|
| Node.js | 22 LTS | JavaScript runtime (from base) |
| pnpm | 9.x | Fast, disk-efficient package manager |
| tsx | latest | Run TypeScript directly |
| typescript (tsc) | 5.x | TypeScript compiler |
| npm-check-updates | latest | Upgrade dependencies |
| biome | latest | Fast Rust-based linter/formatter (eslint+prettier replacement) |
| turbo | latest | Monorepo build orchestrator |
| vite | latest | Build tool and dev server |

**Cache environment:**
```
PNPM_HOME=/cache/pnpm
npm_config_cache=/cache/npm
```

### Rust Layer

Installed via `images/rust/install.sh`, configured via `images/rust/layer.toml`.

| Tool | Version | Description |
|------|---------|-------------|
| rustc | stable | Rust compiler |
| cargo | stable | Rust package manager |
| rustfmt | stable | Code formatter |
| clippy | stable | Linter |
| bacon | latest | TUI file watcher (replaces cargo-watch) |
| cargo-edit | latest | `cargo add/rm/upgrade` commands |
| cargo-outdated | latest | Check for outdated dependencies |
| cargo-nextest | latest | Structured test runner with per-test timing |
| sccache | latest | Shared compilation cache across sessions |

**Cache environment:**
```
CARGO_HOME=/cache/cargo
RUSTUP_HOME=/opt/rustup
RUSTC_WRAPPER=sccache
SCCACHE_DIR=/cache/sccache
```

## Layer System

Each language layer consists of two files:

- **`layer.toml`** — Metadata (name, description, env vars, cache paths)
- **`install.sh`** — Idempotent install script (runs as root, ends with `--version` verification)

Both are compiled into the `mino` binary via `include_str!`. At runtime, `--image typescript` or `--layers typescript` triggers composition: a Dockerfile is generated from `mino-base` + `install.sh`, built, and cached as `mino-composed-{hash}`.

### Adding a new language layer

1. Create `images/{language}/layer.toml`:
   ```toml
   name = "{language}"
   description = "Mino {language} development layer"

   [env]
   {LANG}_CACHE = "/cache/{lang}"

   [cache]
   paths = ["/cache/{lang}"]
   ```

2. Create `images/{language}/install.sh`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   # Install toolchain (runs as root, must be idempotent)
   # ...
   # Verify installations
   {tool} --version
   ```

3. Add `include_str!` in `src/layer/mod.rs` for the new layer.

4. Add alias in `src/cli/commands/run.rs` `image_alias_to_layer()`:
   ```rust
   "{language}" | "{alias}" => Some("{language}"),
   ```

5. Update this README with tool inventory.

## Local Development

### Build & Test Base Image

```bash
# Build and test base image
./images/build.sh

# Test existing image (skip build)
./images/build.sh --test-only

# Fresh build without cache
./images/build.sh --no-cache

# Use podman instead of docker
DOCKER=podman ./images/build.sh
```

## CI/CD

The base image is automatically built and pushed to GHCR:

- **Trigger**: Push to `images/**`, weekly cron (Mondays), manual dispatch
- **Platforms**: `linux/amd64`, `linux/arm64`
- **Tags**: `latest`, `<sha>`, `<YYYYMMDD>` (for scheduled builds)

See `.github/workflows/images.yml` for details.

## Tool Selection Rationale

| Tool | Over | Reason |
|------|------|--------|
| **delta** | diff-so-fancy | Syntax highlighting for 200+ languages, within-line highlighting |
| **zoxide** | autojump/z | 10x faster startup (5ms vs 50ms), Rust-based, fzf integration |
| **fzf** | atuin/mcfly | Already installed for file search, Ctrl+R for fuzzy history, zero extra dependencies |
| **eza** | ls/tree | Single binary replaces both `ls` and `tree`, color/git integration |
| **sd** | sed | Intuitive regex syntax, no escaping nightmares, string literal mode |
| **yq** | python-yaml | `jq` syntax for YAML, single binary, no runtime dependencies |
| **tokei** | cloc | 10x faster, accurate language detection, Rust-based |
| **biome** | eslint+prettier | Single tool, Rust-based, 100x faster, zero-config |
| **cargo-nextest** | cargo test | Per-test timing, structured output, retry support |
| **sccache** | none | Shared compilation cache, accelerates rebuilds across sessions |
| **pnpm** | npm/yarn | 70% less disk space, fastest installs, ~20% market share |
| **bacon** | cargo-watch | cargo-watch archived Jan 2025, bacon has TUI, multi-job support |

### Version Policy

- **Node.js**: LTS versions only (currently 22, becomes maintenance Apr 2027)
- **Rust**: Stable toolchain via rustup (auto-updates)
- **Tools**: Latest stable, base image rebuilt weekly for security updates
