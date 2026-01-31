# Minotaur

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

Secure sandbox wrapper for AI coding agents using OrbStack + Podman rootless containers.

Wraps **any command** in isolated containers with temporary cloud credentials and SSH agent forwarding. Works with Claude Code, Aider, Cursor, or any CLI tool.

## Why Minotaur?

AI coding agents are powerful but require significant system access. Minotaur provides defense-in-depth:

- **Filesystem Isolation**: Agent only sees your project directory, not `~/.ssh`, `~/.aws`, or system files
- **Credential Scoping**: Short-lived cloud tokens instead of permanent credentials
- **Network Boundaries**: Container-level network isolation via Podman

## Features

- **Rootless Containers**: Podman containers inside OrbStack VMs - no root required
- **Temporary Credentials**: Generates short-lived AWS/GCP/Azure tokens (1-12 hours)
- **SSH Agent Forwarding**: Git authentication without exposing private keys
- **Multi-Session**: Run multiple isolated sandboxes in parallel
- **Zero Config**: Works out of the box with sensible defaults

## Requirements

- **macOS** with [OrbStack](https://orbstack.dev) installed
- Cloud CLIs (optional, for credential generation):
  - `aws` - AWS credentials via STS
  - `gcloud` - GCP access tokens
  - `az` - Azure access tokens
  - `gh` - GitHub token

## Installation

### From Source

```bash
git clone https://github.com/yourname/minotaur.git
cd minotaur
cargo install --path .
```

### Verify Installation

```bash
minotaur status
```

## Quick Start

```bash
# Interactive shell in sandbox
minotaur run

# Run Claude Code in sandbox
minotaur run -- claude

# Run with AWS credentials
minotaur run --aws -- bash

# Run with all cloud credentials
minotaur run --all-clouds -- bash

# Named session with specific project
minotaur run -n my-feature -p ~/projects/myapp -- zsh

# Use a different container image
minotaur run --image ubuntu:22.04 -- bash
```

## CLI Reference

### Global Options

These options work with all commands:

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Enable verbose output |
| `-c, --config <PATH>` | Configuration file path (env: `MINOTAUR_CONFIG`) |

### Commands

#### `minotaur run`

Start a sandboxed session.

```bash
minotaur run [OPTIONS] [-- COMMAND...]
```

| Option | Description |
|--------|-------------|
| `-n, --name <NAME>` | Session name (auto-generated if omitted) |
| `-p, --project <PATH>` | Project directory to mount (default: current dir) |
| `--image <IMAGE>` | Container image to use (default: fedora:41) |
| `--aws` | Include AWS credentials |
| `--gcp` | Include GCP credentials |
| `--azure` | Include Azure credentials |
| `--all-clouds` | Include all cloud credentials |
| `--github` | Include GitHub token (default: true) |
| `--ssh-agent` | Forward SSH agent (default: true) |
| `-e, --env <KEY=VALUE>` | Additional environment variable |
| `-V, --volume <HOST:CONTAINER>` | Additional volume mount |
| `-d, --detach` | Run in background |

#### `minotaur list`

List sessions.

```bash
minotaur list [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-a, --all` | Show all sessions including stopped |
| `-f, --format <FORMAT>` | Output format: `table`, `json`, `plain` (default: table) |

#### `minotaur stop`

Stop a running session.

```bash
minotaur stop [OPTIONS] <SESSION>
```

| Option | Description |
|--------|-------------|
| `-f, --force` | Force stop without graceful shutdown |

#### `minotaur logs`

View session logs.

```bash
minotaur logs [OPTIONS] <SESSION>
```

| Option | Description |
|--------|-------------|
| `-f, --follow` | Follow log output (like `tail -f`) |
| `-l, --lines <N>` | Number of lines to show (default: 100, 0 = all) |

#### `minotaur status`

Check system health and dependencies.

```bash
minotaur status
```

#### `minotaur config`

Show or edit configuration.

```bash
minotaur config [SUBCOMMAND]
```

| Subcommand | Description |
|------------|-------------|
| `show` | Show current configuration (default) |
| `path` | Show configuration file path |
| `init [--force]` | Initialize default configuration |
| `set <KEY> <VALUE>` | Set a configuration value (e.g., `vm.name myvm`) |

## Configuration

Configuration is stored at `~/.config/minotaur/config.toml`:

```toml
[general]
verbose = false
log_format = "text"    # "text" or "json"

[vm]
name = "minotaur"
distro = "fedora"
# cpus = 4             # CPU cores (optional)
# memory_mb = 4096     # Memory in MB (optional)

[container]
image = "fedora:41"
workdir = "/workspace"
network = "host"
packages = ["git", "curl", "which"]  # Installed on first run
# env = { "MY_VAR" = "value" }       # Additional env vars
# volumes = ["/host/path:/container/path"]

[credentials.aws]
session_duration_secs = 3600         # Token lifetime (1-12 hours)
# role_arn = "arn:aws:iam::123456789012:role/MyRole"
# external_id = "my-external-id"
# profile = "default"
# region = "us-east-1"

[credentials.gcp]
# project = "my-project"
# service_account = "sa@project.iam.gserviceaccount.com"

[credentials.azure]
# subscription = "subscription-id"
# tenant = "tenant-id"

[credentials.github]
host = "github.com"    # For GitHub Enterprise

[session]
shell = "/bin/bash"
# default_project_dir = "/path/to/default/project"
```

### Configuration Keys

Use `minotaur config set <key> <value>` to modify:

```
general.verbose
general.log_format
vm.name
vm.distro
vm.cpus
vm.memory_mb
container.image
container.network
container.workdir
credentials.aws.session_duration_secs
credentials.aws.role_arn
credentials.aws.profile
credentials.aws.region
credentials.gcp.project
credentials.azure.subscription
credentials.azure.tenant
session.shell
```

## Architecture

```
macOS Host
    │
    ├─ minotaur CLI (Rust binary)
    │   • Validates environment (OrbStack, Podman)
    │   • Generates temp credentials (STS, gcloud, az)
    │   • Manages session lifecycle
    │
    └─► OrbStack VM (lightweight Linux, ~200MB)
        │
        └─► Podman rootless container
            • Mounted: /workspace (project dir only)
            • SSH agent socket forwarded
            • Temp credentials as env vars
            • NO access to: ~/.ssh, ~/, system dirs
```

## Credential Strategy

| Service | Method | Lifetime |
|---------|--------|----------|
| SSH/Git | Agent forwarding via socket | Session |
| GitHub | `gh auth token` | Existing token |
| AWS | STS GetSessionToken/AssumeRole | 1-12 hours |
| GCP | `gcloud auth print-access-token` | 1 hour |
| Azure | `az account get-access-token` | 1 hour |

Credentials are cached with TTL awareness - Minotaur automatically refreshes expired tokens.

## State Storage

```
~/.config/minotaur/config.toml      # User configuration
~/.local/state/minotaur/
├── sessions/*.json                  # Session state
└── credentials/*.json               # Cached credentials (600 perms)
```

## Security Considerations

Minotaur provides defense-in-depth but is not a complete security solution:

- **Trust Boundary**: The container can access anything mounted into it
- **Network Access**: Default `host` network mode allows outbound connections
- **Credential Scope**: Temporary credentials still have the permissions of the source identity
- **OrbStack Trust**: You're trusting OrbStack's VM isolation

For maximum security:
1. Use dedicated cloud roles with minimal permissions
2. Use named sessions to track activity
3. Consider network-restricted container modes for sensitive work

## Development

```bash
# Build debug
cargo build

# Build release
cargo build --release

# Run tests
cargo test

# Run with debug logging
RUST_LOG=minotaur=debug cargo run -- status

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
