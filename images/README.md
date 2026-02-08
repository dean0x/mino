# Mino Container Images

Pre-built development images with Claude Code and productivity tools.

## Available Images

| Image | Registry | Description |
|-------|----------|-------------|
| `mino-base` | `ghcr.io/dean0x/mino-base:latest` | Foundation with Claude Code and dev tools |
| `mino-typescript` | `ghcr.io/dean0x/mino-typescript:latest` | TypeScript/Node.js development |
| `mino-rust` | `ghcr.io/dean0x/mino-rust:latest` | Rust development |

## Quick Start

```bash
# Use aliases with mino
mino run --image typescript -- claude
mino run --image rust -- claude

# Or use full image paths
mino run --image ghcr.io/dean0x/mino-typescript:latest -- claude
```

## Image Aliases

| Alias | Image |
|-------|-------|
| `typescript`, `ts`, `node` | `mino-typescript` |
| `rust`, `cargo` | `mino-rust` |
| `base` | `mino-base` |

## Tool Inventory

### Base Image (`mino-base`)

All language images inherit these tools.

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

### TypeScript Image (`mino-typescript`)

| Tool | Version | Description |
|------|---------|-------------|
| Node.js | 22 LTS | JavaScript runtime |
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

### Rust Image (`mino-rust`)

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

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  mino-base                           │
│  Fedora 43 + Node 22 LTS + tools + claude-code          │
│  Oh My Zsh + autosuggestions + history-substring-search  │
│  nvm + eza + sd + yq + tokei                            │
└─────────────────────────────────────────────────────────┘
                          │
          ┌───────────────┴───────────────┐
          ▼                               ▼
┌─────────────────────┐       ┌─────────────────────┐
│  mino-typescript │       │    mino-rust    │
│  pnpm, tsx, tsc      │       │  cargo, clippy      │
│  biome, turbo, vite  │       │  nextest, sccache   │
└─────────────────────┘       └─────────────────────┘
```

## Local Development

### Build & Test Script

Use `build.sh` for local development - it builds images in the correct order and runs comprehensive tests.

```bash
# Build and test all images
./images/build.sh

# Test existing images (skip build)
./images/build.sh --test-only

# Fresh build without cache
./images/build.sh --no-cache

# Build/test specific image only
./images/build.sh base
./images/build.sh typescript
./images/build.sh rust

# Use podman instead of docker
DOCKER=podman ./images/build.sh
```

The script validates:
- **Structure**: User is `developer`, workdir is `/workspace`, `/cache` directory exists
- **Environment**: Cache paths configured correctly (`PNPM_HOME`, `CARGO_HOME`, `SCCACHE_DIR`, etc.)
- **Tools**: Every tool listed in the inventory runs `--version` successfully
- **Shell**: Oh My Zsh, zsh-autosuggestions, zsh-history-substring-search, nvm installed

### Manual Building

If you need to build manually:

```bash
# Build base first (required by other images)
docker build -t mino-base ./images/base

# Build language images (reference local base)
docker build -t mino-typescript --build-arg BASE_IMAGE=mino-base ./images/typescript
docker build -t mino-rust --build-arg BASE_IMAGE=mino-base ./images/rust
```

### Interactive Testing

```bash
# Shell into an image
docker run --rm -it mino-typescript

# Mount current directory
docker run --rm -it -v $(pwd):/workspace mino-typescript
```

## CI/CD

Images are automatically built and pushed to GHCR:

- **Trigger**: Push to `images/**`, weekly cron (Mondays), manual dispatch
- **Platforms**: `linux/amd64`, `linux/arm64`
- **Tags**: `latest`, `<sha>`, `<YYYYMMDD>` (for scheduled builds)

See `.github/workflows/images.yml` for details.

## Tool Selection Rationale

### Why These Tools?

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
- **Tools**: Latest stable, rebuilt weekly for security updates
