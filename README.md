# Mino

[![Website](https://img.shields.io/badge/Website-dean0x.github.io%2Fx%2Fmino-blue?style=for-the-badge)](https://dean0x.github.io/x/mino/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-orange?style=for-the-badge)](https://www.rust-lang.org)
[![Crates.io](https://img.shields.io/crates/v/mino?style=for-the-badge)](https://crates.io/crates/mino)
[![npm](https://img.shields.io/npm/v/@dean0x/mino?style=for-the-badge)](https://www.npmjs.com/package/@dean0x/mino)
[![GitHub Release](https://img.shields.io/github/v/release/dean0x/mino?style=for-the-badge)](https://github.com/dean0x/mino/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/dean0x/mino/ci.yml?style=for-the-badge&label=CI)](https://github.com/dean0x/mino/actions/workflows/ci.yml)

Secure sandbox wrapper for AI coding agents using rootless Podman containers.

Wraps **any command** in isolated containers with temporary cloud credentials and SSH agent forwarding. Works with Claude Code, Aider, Cursor, or any CLI tool.

<p align="center">
  <img src=".github/assets/demo.gif" alt="Mino terminal demo" width="800">
</p>

## Why Mino?

AI coding agents are powerful but require significant system access. Mino provides defense-in-depth:

- **Filesystem Isolation**: Agent only sees your project directory, not `~/.ssh`, `~/.aws`, or system files
- **Credential Scoping**: Short-lived cloud tokens instead of permanent credentials
- **Network Boundaries**: Four network modes — bridge (default), host, none, or allowlisted egress via iptables — with built-in presets for common services

## Features

- **Rootless Containers**: Podman containers with no root required (OrbStack VM on macOS, native on Linux)
- **Temporary Credentials**: Generates short-lived AWS/GCP/Azure tokens (1-12 hours)
- **SSH Agent Forwarding**: Git authentication without exposing private keys
- **Persistent Caching**: Content-addressed dependency caches survive session crashes
- **Multi-Session**: Run multiple isolated sandboxes in parallel
- **Network Isolation**: Bridge networking by default with interactive prompt on first run. Block all traffic, allowlist specific destinations, or use built-in presets (`dev`, `registries`)
- **Zero Config**: Works out of the box with sensible defaults

## Requirements

- **macOS**: [OrbStack](https://orbstack.dev) installed (manages a lightweight Linux VM with Podman)
- **Linux**: [Podman](https://podman.io) installed in rootless mode (no VM needed)
- Cloud CLIs (optional): `aws`, `gcloud`, `az`, `gh`

Run `mino setup` to check and install prerequisites for your platform.

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
mino run --image python -- claude        # Python
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
| `--image <IMAGE>` | Container image (default: fedora:43). Aliases: `typescript`/`ts`/`node`, `rust`/`cargo`, `python`/`py`, `base` |
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
| `--runtime <MODE>` | Runtime mode: `container` (default), `native` |

**Layer precedence**: `--layers` flag > `--image` flag > `MINO_LAYERS` env var > config `container.layers` > interactive selection > config `container.image`.

Set `MINO_LAYERS=rust,typescript` in your environment for non-interactive layer selection (CI, IDE plugins). When no layers or image are configured and the terminal is interactive, `mino run` prompts for layer selection with an option to save to config. Selecting "Base only" persists `image = "base"` to your config, skipping the prompt on subsequent runs.

On Unix systems, Mino automatically saves and restores terminal state when a session is interrupted (e.g., Ctrl+C during a prompt or container run), preventing shell corruption.

#### `mino exec`

Execute a command in a running session.

```bash
mino exec [SESSION] [-- COMMAND...]
```

| Option | Description |
|--------|-------------|
| `SESSION` | Session name (defaults to most recent running session) |
| `COMMAND` | Command to run (defaults to `/bin/zsh`) |

Examples:

```bash
mino exec                              # Shell into most recent session
mino exec my-session                   # Shell into named session
mino exec my-session -- ls -la         # Run command in named session
```

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

#### `mino completions`

Generate shell completion scripts.

```bash
mino completions <SHELL>
```

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

**Installation:**

```bash
# Bash — write to completions directory
mino completions bash > ~/.local/share/bash-completion/completions/mino

# Zsh — write to fpath directory
mino completions zsh > ${fpath[1]}/_mino

# Fish — write to completions directory
mino completions fish > ~/.config/fish/completions/mino.fish
```

Alternatively, add `eval "$(mino completions bash)"` or `eval "$(mino completions zsh)"` to your shell's rc file.

## Configuration

Configuration is stored at `~/.config/mino/config.toml` on Linux and `~/Library/Application Support/mino/config.toml` on macOS:

```toml
[general]
verbose = false
log_format = "text"    # "text" or "json"
audit_log = true       # Security events written to state dir
update_check = true    # Check for new versions (once/24h)
runtime = "container"  # "container", "native", or "auto"

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
general.update_check
general.runtime
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
sandbox.sandbox_user
sandbox.max_memory_mb
sandbox.max_processes
sandbox.max_cpu_seconds
sandbox.max_file_size_mb
sandbox.cache_mode
sandbox.allow_sensitive
sandbox.allow_sensitive_paths
sandbox.network
sandbox.network_allow
sandbox.network_preset
sandbox.env_passthrough
sandbox.auto_passthrough_dirs
sandbox.auto_copy_dirs
```

> **Note**: Most `[sandbox]` fields are managed by `mino setup --native` or edited directly
> in the config file. `mino config set sandbox.*` is supported for the scalar fields listed above.

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
   - `uv.lock` -> uv
   - `go.sum` -> go

2. **Cache Key**: `sha256(lockfile_contents)[:12]` - same lockfile = same cache

3. **Cache States**:
   | State | Mount | When |
   |-------|-------|------|
   | Miss | read-write | No cache exists, creating new |
   | Building | read-write | In progress or crashed (retryable) |
   | Complete | read-write | Finalized, skip re-finalization |

4. **Environment Variables**: Automatically configured:
   ```
   npm_config_cache=/cache/npm
   CARGO_HOME=/cache/cargo
   PIP_CACHE_DIR=/cache/pip
   UV_CACHE_DIR=/cache/uv
   XDG_CACHE_HOME=/cache/xdg
   ```

### Security

- **Content-addressed**: Same lockfile = same cache volume; changing dependencies = new hash = new cache
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

### Presets

| Preset | Destinations | Use case |
|--------|-------------|----------|
| `dev` | github.com (443, 22), api.github.com, registry.npmjs.org, crates.io, static.crates.io, index.crates.io, pypi.org, files.pythonhosted.org, api.anthropic.com, api.openai.com | Dev with AI agents |
| `registries` | registry.npmjs.org, crates.io, static.crates.io, index.crates.io, pypi.org, files.pythonhosted.org | Package install only |

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

## Sandbox Modes

Mino supports two isolation strategies. Choose based on your platform and how much startup
overhead you can accept.

### Overview

| | Container | Native |
|---|---|---|
| **Mechanism** | Rootless Podman container | macOS: dedicated system user + pf firewall |
| **Startup time** | 2–4 s (image pull cached) | < 0.5 s |
| **Platform** | macOS (via OrbStack), Linux | macOS only (Linux coming) |
| **Root required** | No | No (sudoers grants one command) |
| **Filesystem isolation** | Container image | ACLs on project + dotfile dirs |
| **Network isolation** | iptables in container | pf + SOCKS5 proxy |
| **Language toolchains** | Layer system (`--layers rust`) | Host tools (auto-detected) |

### When to Use Native vs Container

**Use native when:**
- You want near-instant sandbox startup (< 0.5 s vs 2–4 s)
- You use macOS and already have your toolchain installed on the host
- You are running Claude Code interactively and tolerate less filesystem isolation

**Use container when:**
- You need strict filesystem isolation (read-only root, immutable image)
- You need reproducible toolchain versions across machines
- You are on Linux or prefer Podman-based isolation

### Platform Support

| Platform | Container | Native |
|---|---|---|
| macOS (Apple Silicon, x86) | via OrbStack | supported |
| Linux | direct Podman | experimental |

### Setup (macOS Native)

```bash
# One-time setup: creates _mino_agent system user, installs helper binary,
# configures sudoers and pf anchor
mino setup --native

# Verify setup is complete
mino setup --native --check
```

The setup creates:
- System user `_mino_agent` (UID in system range, no login shell)
- `/usr/local/bin/mino-sandbox-helper` (root-owned, sudoers-controlled)
- `/etc/sudoers.d/mino` (grants your user passwordless sudo for the helper only)
- `/etc/pf.anchors/mino` (packet filter anchor for network isolation)

### Running (Native Mode)

```bash
# Select native mode via CLI flag
mino run --runtime native -- claude
```

```toml
# Or set as default in config (avoids repeating the flag)
# ~/.config/mino/config.toml
[general]
runtime = "native"  # "container" (default), "native", or "auto"
```

```bash
# With network allow (pf-based, not iptables)
mino run --runtime native --network-allow github.com:443 -- claude

# Fully offline
mino run --runtime native --network none -- claude
```

### Configuration

All native sandbox settings live under `[sandbox]` in the mino config file
(`~/.config/mino/config.toml` on Linux, `~/Library/Application Support/mino/config.toml` on macOS).
Network and env fields fall back to `[container]` values when not set in `[sandbox]`.

```toml
[sandbox]
# Dedicated macOS system user for process isolation
sandbox_user = "_mino_agent"          # default

# Resource limits
max_memory_mb = 4096                  # 0 = no limit
max_processes = 256
max_cpu_seconds = 0                   # 0 = no limit
max_file_size_mb = 0                  # max size of a single file, 0 = no limit

# Cache access mode for build caches mounted from the host
# "read-only" (default), "read-write", or "none"
cache_mode = "read-only"

# Allow mounting paths from the built-in sensitive blocklist
# (e.g. ~/.ssh, ~/.aws). Leave false unless you have a specific reason.
allow_sensitive = false

# Network isolation (falls back to [container] values if not set)
network = "bridge"                    # host | none | bridge
network_allow = ["github.com:443"]    # implies bridge + pf filter
# network_preset = "dev"              # preset allowlist

# Environment passthrough: which host env vars the sandbox inherits
# Default: ["ANTHROPIC_API_KEY", "LANG", "LC_ALL", "TZ", "TERM"]
# Add other AI provider keys here:
# env_passthrough = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LANG", "LC_ALL", "TZ", "TERM"]

# Explicit env vars injected into every sandbox session
# (falls back to [container].env if not set)
[sandbox.env]
# MY_VAR = "my_value"

# Additional read-only paths (absolute paths only, not in sensitive list)
# passthrough_paths = ["/usr/local/share/my-data"]

# Additional writable paths
# writable_paths = ["/tmp/my-workspace"]

# Dotfiles to copy from your host $HOME into the sandbox home
# (sanitized: .gitconfig has credential sections stripped)
# dotfiles = [".vimrc", ".bashrc"]

# Directories to mount read-only as symlinks (opt-in, empty by default)
# Detected and populated by `mino setup --native` (toolchain auto-detection)
# auto_passthrough_dirs = [".cargo", ".nvm", ".pyenv"]

# Specific sensitive paths to allow despite the blocklist.
# Narrower than allow_sensitive = true — only the listed paths are permitted.
# Written by `mino setup --native` when you opt in to .config/gh, .docker, etc.
# allow_sensitive_paths = [".config/gh", ".docker"]

# Directories to copy (mutable sandbox-local copy, opt-in)
# .claude uses an allowlist (CLAUDE.md, settings.json, agents, commands, skills)
# auto_copy_dirs = [".claude"]
```

### Toolchain Auto-Detection

`mino setup --native` detects installed toolchain directories and offers to add them to
`auto_passthrough_dirs` so shell init files can source them without errors like:

```
/bin/zsh: /Users/you/.cargo/env: no such file or directory
```

**Safe passthrough (auto-accepted in non-interactive / CI):**
Rust (`.cargo`, `.rustup`), Node.js (`.nvm`, `.npm`, `.yarn`, `.volta`, `.bun`, `.pnpm`),
Python (`.pyenv`, `.pipx`, `.poetry`, `.uv`), Ruby (`.rbenv`, `.gem`), JVM (`.sdkman`, `.gradle`, `.m2`),
Go (`.go`), Haskell (`.ghcup`, `.cabal`, `.stack`), shell plugins (`.oh-my-zsh`, `.fzf`, `.starship`), and more.

**Credential directories (interactive-only, opt-in with warning):**
`.config/gh` (GitHub CLI token), `.docker` (registry auth), `.kube` (Kubernetes config).
These are added to both `auto_passthrough_dirs` AND `allow_sensitive_paths` so
`SandboxConfig` validation allows them. `--yes` / non-interactive mode **never**
auto-accepts these — you must run setup interactively to opt in.

**Strictly blocked directories (never offered by detection):**
`.ssh`, `.aws`, `.azure`, `.gnupg`, `.config/gcloud`, `.netrc` — these are pure
credential stores and remain on the `SENSITIVE_PATHS` blocklist. They can only
be unblocked by setting the nuclear `allow_sensitive = true` flag, which
bypasses the entire blocklist (power-user escape hatch, not recommended).

**`.claude` auto-copy (interactive confirm):**
If `~/.claude` exists, setup offers to add it to `auto_copy_dirs`. Only the allowlisted
subset is copied (CLAUDE.md, settings.json, agents, commands, skills, current project memory).

**Opting out per-directory:** in the interactive multiselect you can deselect any
detected entry with the space bar before confirming. Declined entries are not
written to the config. After installing a new toolchain, simply re-run:

```bash
mino setup --native
```

Setup always runs detection and merges any newly-detected entries into the
existing `auto_passthrough_dirs` list — already-present entries are left
untouched. To permanently remove an entry you previously accepted, edit the
config file directly. The `.claude` auto-copy step is skipped outright if
`auto_copy_dirs` is already non-empty.

### Security Model

**What the native sandbox does:**
- Runs the agent process as `_mino_agent` (unprivileged system user, no home, no login)
- ACLs on the project directory allow `_mino_agent` read-write access
- ACLs on passthrough paths allow read-only access
- pf anchor blocks all outbound traffic except the allowlist
- SOCKS5 proxy for `--network-allow` rules (no iptables required on host)
- Home directory created fresh in `/tmp/mino-home-<session-id>` on each run, removed after exit

**What it does NOT do:**
- Syscall filtering (no seccomp / sandbox-exec)
- Filesystem root isolation (host filesystem is visible via path traversal)
- Memory isolation beyond OS-level user boundaries
- Network namespace isolation (pf rules only)

**Threat model:** The sandbox assumes the agent is non-malicious but potentially buggy or
prompt-injected. It limits the blast radius to the project directory and the agent's session
home. It does NOT defend against a fully adversarial process with local exploit capabilities.

### Troubleshooting

**`mino: helper binary not found`**
Run `mino setup --native` to install the helper binary.

**`Operation not permitted` on startup**
The helper binary may have wrong ownership. Re-run `mino setup --native`.

**`ACL setup failed on home dir`**
Check that `_mino_agent` user exists: `dscl . -read /Users/_mino_agent`.
Re-run `mino setup --native` if the user is missing.

**`pf anchor not loaded`**
The pf anchor is loaded on each run. If pf is not enabled on your system:
`sudo pfctl -e` to enable, then re-run.

**Sandbox process can access host files**
Native mode uses ACLs, not filesystem namespaces. The agent runs as `_mino_agent`
which cannot write to your home directory, but can read world-readable files.
Use container mode for strict read-only filesystem isolation.

**`mino setup --check` reports issues but setup completed**
Re-run `mino setup --native` to repair. Some steps are idempotent.

### Uninstall

```bash
# Remove all mino native sandbox components
mino setup --native --uninstall

# Removes: _mino_agent user, helper binary, sudoers entry, pf anchor
```

## Container Images

Mino uses a base image (`mino-base`) with a layer composition system for language toolchains.

| Alias | Behavior | Includes |
|-------|----------|----------|
| `typescript`, `ts`, `node` | Layer composition from `mino-base` | Node.js 22 LTS, pnpm, tsx, TypeScript, biome |
| `rust`, `cargo` | Layer composition from `mino-base` | rustup, cargo, clippy, bacon, sccache |
| `python`, `py` | Layer composition from `mino-base` | Python 3.13, uv, ruff, pytest |
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
name = "go"
description = "Go toolchain + tools"
version = "1"

[env]
GOPATH = "/cache/go"
GOMODCACHE = "/cache/go/mod"
GOCACHE = "/cache/go/build"

[env.path_prepend]
dirs = ["/usr/local/go/bin", "/cache/go/bin"]

[cache]
paths = ["/cache/go"]
```

**`install.sh`** — runs as root on `mino-base`. Must be idempotent (safe to re-run):

```bash
#!/usr/bin/env bash
set -euo pipefail

# Install Go (idempotent)
if ! command -v go &>/dev/null; then
    curl -LsSf https://go.dev/dl/go1.24.1.linux-amd64.tar.gz | tar -C /usr/local -xzf -
fi

# Install tools
export GOPATH=/opt/go-tools
export PATH="/usr/local/go/bin:${GOPATH}/bin:${PATH}"
go install golang.org/x/tools/gopls@latest
go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest

# Fix permissions
chown -R developer:developer /opt/go-tools
chmod -R a+rX /opt/go-tools

# Verify
go version
gopls version
```

### Using Custom Layers

```bash
# Use by name (resolved from layer locations)
mino run --layers go

# Compose multiple layers
mino run --layers go,rust

# Set via environment for CI
export MINO_LAYERS=go
mino run -- go test ./...
```

### Overriding Built-in Layers

To customize a built-in layer, create a layer with the same name in your project or user config directory. Your version takes precedence:

```
.mino/layers/typescript/layer.toml    # overrides built-in typescript
.mino/layers/typescript/install.sh
```

## Architecture

### macOS (via OrbStack)

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

### Linux (native Podman)

```
Linux Host
    |
    +- mino CLI (Rust binary)
    |   - Validates environment (rootless Podman)
    |   - Generates temp credentials (STS, gcloud, az)
    |   - Manages session lifecycle
    |
    +-> Podman rootless container (no VM layer)
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
# User configuration (platform-specific):
#   Linux:  ~/.config/mino/config.toml
#   macOS:  ~/Library/Application Support/mino/config.toml

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

## Audit Log

Mino writes security events to `<state_dir>/mino/audit.log` in JSON Lines format. Enabled by default; disable with `general.audit_log = false` in config.

Each line is a JSON object:
```json
{"timestamp":"2026-03-09T12:00:00Z","event":"session.created","data":{...}}
```

### Events

| Event | When | Data fields |
|-------|------|-------------|
| `session.created` | Session state initialized | `name`, `project_dir`, `image`, `command` |
| `credentials.injected` | Cloud credentials passed to container | `session_name`, `providers` |
| `session.started` | Container running | `name`, `container_id` |
| `session.stopped` | Container exited | `name`, `exit_code` |
| `session.failed` | Container failed to start | `name`, `error` |

Audit logging uses silent failure mode — IO errors are logged via `tracing::warn` but never block or crash the primary workflow.

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
