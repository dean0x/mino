# Mino

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)
[![Crates.io](https://img.shields.io/crates/v/mino.svg)](https://crates.io/crates/mino)
[![npm](https://img.shields.io/npm/v/@dean0x/mino)](https://www.npmjs.com/package/@dean0x/mino)
[![GitHub Release](https://img.shields.io/github/v/release/dean0x/mino)](https://github.com/dean0x/mino/releases)
[![CI](https://github.com/dean0x/mino/actions/workflows/ci.yml/badge.svg)](https://github.com/dean0x/mino/actions/workflows/ci.yml)

Secure sandbox wrapper for AI coding agents using OrbStack + Podman rootless containers.

Wraps **any command** in isolated containers with temporary cloud credentials and SSH agent forwarding. Works with Claude Code, Aider, Cursor, or any CLI tool.

<p align="center">
  <img src="demo.gif" alt="Mino terminal demo" width="800">
</p>

## Why Mino?

AI coding agents are powerful but require significant system access. Mino provides defense-in-depth:

- **Filesystem Isolation**: Agent only sees your project directory, not `~/.ssh`, `~/.aws`, or system files
- **Credential Scoping**: Short-lived cloud tokens instead of permanent credentials
- **Network Boundaries**: Four network modes — bridge (default), host, none, or allowlisted egress via iptables — with built-in presets for common services

## Features

- **Rootless Containers**: Podman containers inside OrbStack VMs - no root required
- **Temporary Credentials**: Generates short-lived AWS/GCP/Azure tokens (1-12 hours)
- **SSH Agent Forwarding**: Git authentication without exposing private keys
- **Persistent Caching**: Content-addressed dependency caches survive session crashes
- **Multi-Session**: Run multiple isolated sandboxes in parallel
- **Network Isolation**: Bridge networking by default with interactive prompt on first run. Block all traffic, allowlist specific destinations, or use built-in presets (`dev`, `registries`)
- **Zero Config**: Works out of the box with sensible defaults

## Requirements

- **macOS** with [OrbStack](https://orbstack.dev) installed
- Cloud CLIs (optional, for credential generation):
  - `aws` - AWS credentials via STS
  - `gcloud` - GCP access tokens
  - `az` - Azure access tokens
  - `gh` - GitHub token

## Installation

### npm

```bash
npm install -g @dean0x/mino
```

### Homebrew

```bash
brew install dean0x/tap/mino
```

### From Source

```bash
git clone https://github.com/dean0x/mino.git
cd mino
cargo install --path .
```

### Verify Installation

```bash
mino status
```

## Quick Start

```bash
# Interactive shell in sandbox
mino run

# Run Claude Code in sandbox
mino run -- claude

# Run with AWS credentials
mino run --aws -- bash

# Run with all cloud credentials
mino run --all-clouds -- bash

# Named session with specific project
mino run -n my-feature -p ~/projects/myapp -- zsh

# Use a different container image
mino run --image ubuntu:22.04 -- bash

# Use mino development images (with Claude Code pre-installed)
mino run --image typescript -- claude    # TypeScript/Node.js
mino run --image rust -- claude          # Rust
mino run --image base -- claude          # Base tools only
```

## CLI Reference

### Global Options

These options work with all commands:

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Enable verbose output |
| `-c, --config <PATH>` | Configuration file path (env: `MINO_CONFIG`) |
| `--no-local` | Skip local `.mino.toml` discovery |

### Commands

#### `mino run`

Start a sandboxed session.

```bash
mino run [OPTIONS] [-- COMMAND...]
```

| Option | Description |
|--------|-------------|
| `-n, --name <NAME>` | Session name (auto-generated if omitted) |
| `-p, --project <PATH>` | Project directory to mount (default: current dir) |
| `--image <IMAGE>` | Container image (default: fedora:43). Aliases: `typescript`/`ts`/`node`, `rust`/`cargo`, `base` |
| `--aws` | Include AWS credentials |
| `--gcp` | Include GCP credentials |
| `--azure` | Include Azure credentials |
| `--all-clouds` | Include all cloud credentials |
| `--github` | Include GitHub token (default: true) |
| `--ssh-agent` | Forward SSH agent (default: true) |
| `--layers <LAYERS>` | Composable layers (comma-separated, conflicts with `--image`) |
| `-e, --env <KEY=VALUE>` | Additional environment variable |
| `--volume <HOST:CONTAINER>` | Additional volume mount |
| `-d, --detach` | Run in background |
| `--no-cache` | Disable dependency caching |
| `--cache-fresh` | Force fresh cache (ignore existing) |
| `--network <MODE>` | Network mode: `bridge` (default), `host`, `none` |
| `--network-allow <RULES>` | Allowlisted destinations (`host:port`, comma-separated). Implies bridge + iptables |
| `--network-preset <PRESET>` | Network preset: `dev`, `registries` (conflicts with `--network-allow`) |

**Layer precedence**: `--layers` flag > `--image` flag > `MINO_LAYERS` env var > config `container.layers` > interactive selection > config `container.image`.

Set `MINO_LAYERS=rust,typescript` in your environment for non-interactive layer selection (CI, IDE plugins). When no layers or image are configured and the terminal is interactive, `mino run` prompts for layer selection with an option to save to config.

#### `mino list`

List sessions.

```bash
mino list [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-a, --all` | Show all sessions including stopped |
| `-f, --format <FORMAT>` | Output format: `table`, `json`, `plain` (default: table) |

#### `mino stop`

Stop a running session.

```bash
mino stop [OPTIONS] <SESSION>
```

| Option | Description |
|--------|-------------|
| `-f, --force` | Force stop without graceful shutdown |

#### `mino logs`

View session logs.

```bash
mino logs [OPTIONS] <SESSION>
```

| Option | Description |
|--------|-------------|
| `-f, --follow` | Follow log output (like `tail -f`) |
| `-l, --lines <N>` | Number of lines to show (default: 100, 0 = all) |

#### `mino status`

Check system health and dependencies.

```bash
mino status
```

#### `mino setup`

Install and configure prerequisites interactively.

```bash
mino setup [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-y, --yes` | Auto-approve all installation prompts |
| `--check` | Check prerequisites only, don't install |
| `--upgrade` | Upgrade existing dependencies to latest versions |

#### `mino init`

Initialize a project-local `.mino.toml` configuration file.

```bash
mino init [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-f, --force` | Overwrite existing `.mino.toml` |
| `-p, --path <DIR>` | Target directory (default: current directory) |

#### `mino cache`

Manage dependency caches.

```bash
mino cache <SUBCOMMAND>
```

| Subcommand | Description |
|------------|-------------|
| `list [-f FORMAT]` | List all cache volumes |
| `info [-p PATH]` | Show cache info for current/specified project |
| `gc [--days N] [--dry-run]` | Remove caches older than N days |
| `clear --volumes\|--images\|--all [-y]` | Clear cache volumes, composed images, or both |

#### `mino config`

Show or edit configuration.

```bash
mino config [SUBCOMMAND]
```

| Subcommand | Description |
|------------|-------------|
| `show` | Show current configuration (default) |
| `path` | Show configuration file path |
| `init [--force]` | Initialize default configuration |
| `set <KEY> <VALUE>` | Set a configuration value (e.g., `vm.name myvm`) |

## Configuration

Configuration is stored at `~/.config/mino/config.toml`:

```toml
[general]
verbose = false
log_format = "text"    # "text" or "json"
audit_log = true       # Security events written to state dir

[vm]
name = "mino"
distro = "fedora"

[container]
image = "fedora:43"
workdir = "/workspace"
network = "bridge"
# network_preset = "dev"              # Preset allowlist: dev, registries
# network_allow = ["github.com:443"]  # Implies bridge + iptables egress filtering
# env = { "MY_VAR" = "value" }       # Additional env vars
# volumes = ["/host/path:/container/path"]
# layers = ["typescript", "rust"]     # Composable language layers

[credentials.aws]
enabled = false                      # Enable via config (equivalent to --aws)
session_duration_secs = 3600         # Token lifetime (1-12 hours)
# role_arn = "arn:aws:iam::123456789012:role/MyRole"
# external_id = "my-external-id"
# profile = "default"
# region = "us-east-1"

[credentials.gcp]
enabled = false                      # Enable via config (equivalent to --gcp)
# project = "my-project"
# service_account = "sa@project.iam.gserviceaccount.com"

[credentials.azure]
enabled = false                      # Enable via config (equivalent to --azure)
# subscription = "subscription-id"
# tenant = "tenant-id"

[credentials.github]
host = "github.com"    # For GitHub Enterprise

[session]
shell = "/bin/bash"
auto_cleanup_hours = 720             # Auto-cleanup stopped sessions (0 = disabled)
# default_project_dir = "/path/to/default/project"

[cache]
enabled = true           # Enable dependency caching
gc_days = 30             # Auto-remove caches older than N days
max_total_gb = 50        # Max total cache size before GC
```

### Configuration Keys

Use `mino config set <key> <value>` to modify:

```
general.verbose
general.log_format
general.audit_log
vm.name
vm.distro
container.image
container.network
container.network_preset
container.workdir
container.network_allow
credentials.aws.enabled
credentials.aws.session_duration_secs
credentials.aws.role_arn
credentials.aws.profile
credentials.aws.region
credentials.gcp.enabled
credentials.gcp.project
credentials.azure.enabled
credentials.azure.subscription
credentials.azure.tenant
session.shell
session.auto_cleanup_hours
```

## Dependency Caching

Mino automatically caches package manager dependencies using content-addressed volumes. If a session crashes, the cache persists and is reused on the next run.

### How It Works

1. **Lockfile Detection**: On `mino run`, scans for lockfiles:
   - `package-lock.json` / `npm-shrinkwrap.json` -> npm
   - `yarn.lock` -> yarn
   - `pnpm-lock.yaml` -> pnpm
   - `Cargo.lock` -> cargo
   - `requirements.txt` / `Pipfile.lock` -> pip
   - `poetry.lock` -> poetry
   - `go.sum` -> go

2. **Cache Key**: `sha256(lockfile_contents)[:12]` - same lockfile = same cache

3. **Cache States**:
   | State | Mount | When |
   |-------|-------|------|
   | Miss | read-write | No cache exists, creating new |
   | Building | read-write | In progress or crashed (retryable) |
   | Complete | read-only | Finalized, immutable |

4. **Environment Variables**: Automatically configured:
   ```
   npm_config_cache=/cache/npm
   CARGO_HOME=/cache/cargo
   PIP_CACHE_DIR=/cache/pip
   XDG_CACHE_HOME=/cache/xdg
   ```

### Security

- **Tamper-proof**: Complete caches are mounted read-only
- **Content-addressed**: Changing dependencies = new hash = new cache
- **Isolated**: Each unique lockfile gets its own cache volume

### Cache Management

```bash
# View caches for current project
mino cache info

# List all cache volumes
mino cache list

# Remove old caches (default: 30 days)
mino cache gc

# Remove caches older than 7 days
mino cache gc --days 7

# Clear everything
mino cache clear --all
```

## Network Isolation

Mino supports four network modes for container sessions:

### Modes

| Mode | Flag | Behavior |
|------|------|----------|
| Bridge | `--network bridge` (default) | Standard bridge networking, isolated from host localhost |
| Host | `--network host` | Full host networking, no restrictions |
| None | `--network none` | No network access at all |
| Allowlist | `--network-allow host:port,...` | Bridge + iptables egress filtering |
| Preset | `--network-preset dev\|registries` | Allowlist with built-in rules for common services |

### Examples

```bash
# Default: bridge networking (isolated from host localhost)
mino run -- bash

# No network access (air-gapped)
mino run --network none -- bash

# Allow only GitHub and npm registry
mino run --network-allow github.com:443,registry.npmjs.org:443 -- claude

# Use dev preset (GitHub, npm, crates.io, PyPI, AI APIs)
mino run --network-preset dev -- claude

# Use registries preset (package repos only, most restrictive)
mino run --network-preset registries -- bash

# Full host networking (no isolation)
mino run --network host -- bash
```

### Allowlist Mode

When using `--network-allow`, Mino:

1. Sets the container to bridge networking
2. Adds `CAP_NET_ADMIN` capability
3. Wraps your command with iptables rules that:
   - DROP all outbound traffic (IPv4 + IPv6)
   - ACCEPT loopback traffic
   - ACCEPT established/related connections
   - ACCEPT DNS (port 53, UDP + TCP)
   - ACCEPT each allowlisted host:port

### Configuration

Set default network allowlist in config:

```toml
[container]
network = "bridge"                                  # default mode
# network_preset = "dev"                            # preset allowlist (conflicts with network_allow)
network_allow = ["github.com:443", "npmjs.org:443"] # implies bridge + iptables
```

Or via CLI: `mino config set container.network_allow "github.com:443,npmjs.org:443"`

### Known Limitations

- **DNS resolution at rule time**: iptables resolves hostnames to IPs when rules are inserted. CDN hosts with rotating IPs may become unreachable during long sessions.
- **iptables required**: The container image must include iptables. Fedora 43 and mino-base include it by default.
- **capsh required**: `--network-allow` and `--network-preset` modes require `capsh` (from `libcap`) in the container image to drop `CAP_NET_ADMIN` after iptables setup. The mino-base image includes it.

## Container Images

Mino uses a base image (`mino-base`) with a layer composition system for language toolchains.

| Alias | Behavior | Includes |
|-------|----------|----------|
| `typescript`, `ts`, `node` | Layer composition from `mino-base` | Node.js 22 LTS, pnpm, tsx, TypeScript, biome |
| `rust`, `cargo` | Layer composition from `mino-base` | rustup, cargo, clippy, bacon, sccache |
| `base` | Pulls `ghcr.io/dean0x/mino-base` | Claude Code, git, delta, ripgrep, zoxide |

Language aliases trigger layer composition at runtime — the toolchain is installed on top of `mino-base` using `install.sh` scripts. Layers can be composed together with `--layers typescript,rust`.

All images include: Claude Code CLI, git, gh CLI, delta (git diff), ripgrep, fd, bat, fzf, neovim, zsh, zoxide.

See [images/README.md](images/README.md) for full tool inventory and layer architecture.

## Custom Layers

You can create custom layers to extend `mino-base` with any toolchain.

### Layer Locations

| Location | Path | Scope |
|----------|------|-------|
| Project-local | `.mino/layers/{name}/` | Current project only |
| User-global | `~/.config/mino/layers/{name}/` | All projects |
| Built-in | Bundled with mino | All projects |

**Resolution order**: project-local > user-global > built-in (first match wins). This lets you override built-in layers per-project or per-user.

### Creating a Layer

Each layer needs two files: `layer.toml` (metadata) and `install.sh` (setup script).

**`layer.toml`** — declares environment variables, PATH extensions, and cache paths:

```toml
[layer]
name = "python"
description = "Python 3.12 + pip + uv"
version = "1"

[env]
UV_CACHE_DIR = "/cache/uv"
PIP_CACHE_DIR = "/cache/pip"
VIRTUAL_ENV = "/opt/venv"

[env.path_prepend]
dirs = ["/opt/venv/bin"]

[cache]
paths = ["/cache/uv", "/cache/pip"]
```

**`install.sh`** — runs as root on `mino-base`. Must be idempotent (safe to re-run):

```bash
#!/usr/bin/env bash
set -euo pipefail

# Idempotent: skip if already installed
if ! command -v python3.12 &>/dev/null; then
    dnf install -y python3.12 python3.12-pip
fi

# Install uv
if ! command -v uv &>/dev/null; then
    curl -LsSf https://astral.sh/uv/install.sh | sh
fi

# Create shared virtualenv
python3.12 -m venv /opt/venv
chmod -R a+rX /opt/venv
chown -R developer:developer /opt/venv

# Verify
python3.12 --version
uv --version
```

### Using Custom Layers

```bash
# Use by name (resolved from layer locations)
mino run --layers python

# Compose multiple layers
mino run --layers python,rust

# Set via environment for CI
export MINO_LAYERS=python
mino run -- pytest
```

### Overriding Built-in Layers

To customize a built-in layer, create a layer with the same name in your project or user config directory. Your version takes precedence:

```
.mino/layers/typescript/layer.toml    # overrides built-in typescript
.mino/layers/typescript/install.sh
```

## Architecture

```
macOS Host
    |
    +- mino CLI (Rust binary)
    |   - Validates environment (OrbStack, Podman)
    |   - Generates temp credentials (STS, gcloud, az)
    |   - Manages session lifecycle
    |
    +-> OrbStack VM (lightweight Linux, ~200MB)
        |
        +-> Podman rootless container
            - Mounted: /workspace (project dir only)
            - SSH agent socket forwarded
            - Temp credentials as env vars
            - NO access to: ~/.ssh, ~/, system dirs
```

## Credential Strategy

| Service | Method | Lifetime |
|---------|--------|----------|
| SSH/Git | Agent forwarding via socket | Session |
| GitHub | `gh auth token` | Existing token |
| AWS | STS GetSessionToken/AssumeRole | 1-12 hours |
| GCP | `gcloud auth print-access-token` | 1 hour |
| Azure | `az account get-access-token` | 1 hour |

Credentials are cached with TTL awareness - Mino automatically refreshes expired tokens.

## State Storage

```
~/.config/mino/config.toml           # User configuration

# State directory (platform-specific):
#   Linux:  ~/.local/state/mino/
#   macOS:  ~/Library/Application Support/mino/
<state_dir>/mino/
+-- sessions/*.json                  # Session state
+-- credentials/*.json               # Cached credentials (0o700 dir, 0o600 files)
+-- audit.log                        # Security audit log
```

## Security Considerations

Mino provides defense-in-depth but is not a complete security solution:

- **Container Hardening**: All containers run with `--cap-drop ALL`, `--security-opt no-new-privileges`, and `--pids-limit 4096` by default
- **Trust Boundary**: The container can access anything mounted into it
- **Network Access**: Default `bridge` mode isolates containers from host localhost. Use `--network none` for air-gapped sessions, `--network-allow` or `--network-preset` for fine-grained egress control
- **Credential Scope**: Temporary credentials still have the permissions of the source identity
- **OrbStack Trust**: You're trusting OrbStack's VM isolation
- **Container Cleanup**: All sessions (interactive and detached) remove containers after exit to prevent credential persistence via `podman inspect`

For maximum security:
1. Use dedicated cloud roles with minimal permissions
2. Use named sessions to track activity
3. Use `--network none` or `--network-allow` for network-restricted sessions
4. Use `--network-preset registries` to limit egress to package registries only

## Development

```bash
# Build debug
cargo build

# Build release
cargo build --release

# Run tests
cargo test

# Run with debug logging
RUST_LOG=mino=debug cargo run -- status

# Format code
cargo fmt

# Lint
cargo clippy
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [OrbStack](https://orbstack.dev) - Fast, lightweight macOS virtualization
- [Podman](https://podman.io) - Daemonless container engine
