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
          ┌───────────────┼───────────────┐
          ▼               ▼               ▼
┌───────────────┐ ┌───────────────┐ ┌───────────────┐
│  typescript   │ │    rust       │ │    python     │
│  pnpm, tsx    │ │  cargo,       │ │  uv, ruff,    │
│  biome, turbo │ │  clippy,      │ │  pytest       │
│               │ │  sccache      │ │               │
└───────────────┘ └───────────────┘ └───────────────┘
```

Language toolchains are **not** pre-built GHCR images. Instead, they are installed at container startup via the bootstrap system. Each layer defines a `[user_install]` section in its `layer.toml`, and the `mino-bootstrap` script handles runtime/tool installation (nvm, rustup, uv) on first run. Layers that also need root-level packages (e.g., `python3-devel`) use a `[root_install]` section, which triggers a Dockerfile compose step. This enables multi-toolchain composition (`--layers typescript,rust`) and eliminates CI flakes from language image builds.

## Quick Start

```bash
# Use aliases with mino (triggers layer composition)
mino run --image typescript -- claude
mino run --image rust -- claude
mino run --image python -- claude

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
| `python`, `py` | Layer composition (Python toolchain on `mino-base`) |
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

Installed via bootstrap (`[user_install]` with nvm runtime), configured via `images/typescript/layer.toml`.

| Tool | Version | Description |
|------|---------|-------------|
| Node.js | 22 LTS | JavaScript runtime (via nvm) |
| pnpm | latest | Fast, disk-efficient package manager |
| tsx | latest | Run TypeScript directly |
| typescript (tsc) | latest | TypeScript compiler |
| npm-check-updates | latest | Upgrade dependencies |
| biome | latest | Fast Rust-based linter/formatter (eslint+prettier replacement) |
| turbo | latest | Monorepo build orchestrator |
| vite | latest | Build tool and dev server |

**Environment:**
```
PNPM_HOME=/cache/pnpm
npm_config_cache=/cache/npm
NODE_ENV=development
PATH prepend: /cache/pnpm, /home/developer/.npm-global/bin
```

### Rust Layer

Installed via bootstrap (`[user_install]` with rustup runtime), configured via `images/rust/layer.toml`.

| Tool | Version | Description |
|------|---------|-------------|
| rustc | stable | Rust compiler (via rustup) |
| cargo | stable | Rust package manager |
| rustfmt | stable | Code formatter |
| clippy | stable | Linter |
| bacon | latest | TUI file watcher (replaces cargo-watch) |
| cargo-edit | latest | `cargo add/rm/upgrade` commands |
| cargo-outdated | latest | Check for outdated dependencies |
| cargo-nextest | latest | Structured test runner with per-test timing |
| sccache | latest | Shared compilation cache across sessions |

**Environment:**
```
CARGO_HOME=/home/developer/.cargo
RUSTUP_HOME=/home/developer/.rustup
RUSTC_WRAPPER=sccache
SCCACHE_DIR=/cache/sccache
PATH prepend: /home/developer/.cargo/bin
```

### Python Layer

System packages installed via `[root_install]` (Dockerfile compose step), tools installed via bootstrap (`[user_install]` with uv runtime). Configured via `images/python/layer.toml`.

| Tool | Version | Description |
|------|---------|-------------|
| python3 | 3.13 | System Python (Fedora 43, via `[root_install]`) |
| python3-devel | 3.13 | Development headers for C extensions (via `[root_install]`) |
| uv | latest | Universal Python package/project manager (via bootstrap) |
| ruff | latest | Extremely fast linter + formatter (via `uv tool install`) |
| pytest | latest | Universal test framework (via `uv tool install`) |

**Environment:**
```
UV_CACHE_DIR=/cache/uv
UV_PYTHON_INSTALL_DIR=/cache/uv/python
PYTHONDONTWRITEBYTECODE=1
PYTHONUNBUFFERED=1
PATH prepend: /home/developer/.local/bin
```

## Layer System

Each language layer is defined by a `layer.toml` file and an optional `install.sh` script. Both are compiled into the `mino` binary via `include_str!`.

### layer.toml sections

- **`[layer]`** -- Metadata: `name`, `description`, `version`
- **`[user_install]`** -- Bootstrap-based installation (runs as the developer user at container startup):
  - `runtime` -- Runtime installer: `nvm`, `rustup`, or `uv`
  - `runtime_version` -- Version to install (e.g., `"22"`, `"stable"`)
  - `npm_globals` -- List of npm packages to install globally (for nvm runtime)
  - `cargo_tools` -- List of cargo tools to install via `cargo-binstall` (for rustup runtime)
  - `uv_tools` -- List of Python tools to install via `uv tool install` (for uv runtime)
- **`[root_install]`** -- Dockerfile compose step (runs as root during image build):
  - `packages` -- List of system packages to install via `dnf`
- **`[env]`** -- Environment variables injected into the container
- **`[env.path_prepend]`** -- Directories to prepend to `PATH` via `MINO_PATH_PREPEND`
- **`[cache]`** -- Paths for persistent cache volume mounts

### How layers are installed

Layers using only `[user_install]` (e.g., TypeScript, Rust) do **not** require a Dockerfile compose step. The bootstrap script installs everything at container startup via `MINO_LAYER_MANIFEST`. This means no image build is needed -- the base image is used directly.

Layers that include `[root_install]` (e.g., Python) trigger a Dockerfile compose step to install system packages, then bootstrap handles the `[user_install]` portion at startup.

### install.sh

Optional. Only needed when `[root_install]` requires custom logic beyond `dnf install`. For layers that use only `[user_install]`, the install.sh is a placeholder comment:

```bash
#!/usr/bin/env bash
# User-level install via bootstrap — see [user_install] in layer.toml
```

When present and non-trivial, install.sh runs as root during the Dockerfile compose step, must be idempotent, and should end with `--version` verification.

### Adding a new language layer

1. Create `images/{language}/layer.toml`:
   ```toml
   [layer]
   name = "{language}"
   description = "Mino {language} development layer"
   version = "2"

   [user_install]
   runtime = "nvm"        # or "rustup" or "uv"
   runtime_version = "22" # version for the runtime installer
   npm_globals = ["tool1", "tool2"]  # for nvm runtime
   # cargo_tools = [...]  # for rustup runtime
   # uv_tools = [...]     # for uv runtime

   [env]
   {LANG}_CACHE = "/cache/{lang}"

   [env.path_prepend]
   dirs = ["/home/developer/.local/bin"]

   [cache]
   paths = ["/cache/{lang}"]
   ```

2. Create `images/{language}/install.sh` (placeholder if `[user_install]` handles everything):
   ```bash
   #!/usr/bin/env bash
   # User-level install via bootstrap — see [user_install] in layer.toml
   ```

   Or, if root packages are needed via `[root_install]`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   dnf install -y --setopt=install_weak_deps=False {packages} \
       && dnf clean all && rm -rf /var/cache/dnf
   {tool} --version
   ```

3. Add `include_str!` in `src/layer/resolve.rs` for the new layer.

4. Add alias in `src/cli/commands/run/image.rs` `image_alias_to_layer()`:
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
- **Python**: System Python from Fedora (currently 3.13), uv manages additional versions
- **Tools**: Latest stable, base image rebuilt weekly for security updates
