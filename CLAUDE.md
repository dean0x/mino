# Mino - Development Guide

## Project Overview

Mino is a secure sandbox wrapper for AI coding agents. It runs commands in rootless Podman
containers with temporary cloud credentials, SSH agent forwarding, and persistent dependency
caching. It also supports a native sandbox mode for macOS that skips containers entirely.

## Architecture

```
src/
├── main.rs                    # CLI entry point
├── lib.rs                     # Library exports
├── error.rs                   # MinoError enum, Result type
├── cache/                     # Dependency caching system
│   ├── lockfile.rs            # Lockfile detection + SHA256 hashing
│   └── volume.rs              # Cache volume state management
├── cli/
│   ├── args.rs                # Clap argument definitions
│   └── commands/              # Command implementations
├── config/
│   ├── mod.rs                 # ConfigManager (validates SandboxConfig on load)
│   └── schema.rs              # TOML config structs
├── orchestration/
│   ├── runtime.rs             # ContainerRuntime trait
│   ├── native_podman.rs       # Linux implementation
│   ├── orbstack_runtime.rs    # macOS implementation
│   ├── orbstack.rs            # OrbStack VM management
│   └── factory.rs             # Platform detection
├── sandbox/                   # Native sandbox subsystem
│   ├── mod.rs                 # RuntimeMode enum + resolve_runtime_mode()
│   ├── config.rs              # SandboxConfig, validate(), DEFAULT_ENV_PASSTHROUGH
│   ├── dotfiles.rs            # Dotfile sanitization + copy_claude_config_dir()
│   ├── fs_copy.rs             # Iterative copy_dir_recursive (BFS, no Box::pin)
│   ├── helper.rs              # ACL helpers, SANDBOX_PATH constant
│   ├── helper_protocol.rs     # HelperRequest/HelperResponse serde types
│   ├── macos.rs               # macOS-specific sandbox launch
│   ├── linux.rs               # Linux user-namespace sandbox
│   ├── native.rs              # SandboxPlatform trait + platform factory
│   ├── process.rs             # SandboxProcess (wait + signal forwarding)
│   ├── proxy.rs               # SOCKS5/HTTP-CONNECT filtering proxy
│   └── resource_limits.rs     # ResourceLimitsDto
├── session/                   # Session state (JSON files)
├── network.rs                 # Network isolation modes + iptables
└── creds/                     # Cloud credential providers
```

Binary: `src/bin/mino-sandbox-helper.rs` — privileged macOS helper (runs as root via sudoers).

## Key Patterns

### Error Handling
- All functions return `MinoResult<T>` (never panic in business logic)
- Use `MinoError::io()` for IO errors with context
- Errors have `.hint()` for actionable suggestions

### Async Runtime
- Tokio multi-threaded runtime
- All IO is async
- Use `async_trait` for async trait methods

### Platform Abstraction
- `ContainerRuntime` trait abstracts Podman operations
- `NativePodmanRuntime` for Linux (direct podman calls)
- `OrbStackRuntime` for macOS (via OrbStack VM)
- Factory pattern selects runtime based on `Platform::detect()`

### Native Sandbox

`RuntimeMode` enum dispatches between container and native paths early in `execute()`:

```rust
match resolve_runtime_mode(cli_runtime, &config.runtime)? {
    RuntimeMode::Container => execute_container(args, config).await,
    RuntimeMode::Native => execute_native(args, config).await,
}
```

**Config precedence** (`[sandbox]` > `[container]` > default):
- Network fields (`network`, `network_allow`, `network_preset`): `resolve_sandbox_network()` in
  `src/sandbox/config.rs` returns the effective value, falling back to `[container]` when the
  `[sandbox]` field is `None`.
- Env passthrough: `SandboxConfig.env_passthrough` (default: `DEFAULT_ENV_PASSTHROUGH`).
- Explicit env vars: `SandboxConfig.env` (falls back to `ContainerConfig.env`).

**Helper binary protocol** (`src/bin/mino-sandbox-helper.rs`):
- Runs as root via `sudoers.d/mino` (one binary, one path, no args)
- Request serialized to a temp JSON file, path passed via `--request-file`
- File is deleted immediately after load to minimize credential exposure on disk
- Responses are printed as JSON to stdout, errors to stderr

**SpawnGuard RAII pattern**:
- Constructed after home dir is created; Drop removes home dir + ACLs on error
- `std::mem::forget(guard)` on the success path transfers cleanup to the parent process
- The guard is installed before ACLs are set (in `InstallGuard` stage) so file_inherit
  applies to any dotfiles copied in the `CopyDotfiles` stage

**SPAWN_STAGES dispatch ordering**:
- `SpawnStage` enum + `SPAWN_STAGES: &[SpawnStage]` const define canonical pipeline order
- Tests assert `InstallGuard < CopyDotfiles`, `ResolveIds < ChownHome`, `ExecChild` is last
- To reorder steps, update `SPAWN_STAGES` — the test suite will catch missing entries

**Dotfile preparation** (`prepare_dotfiles` in `src/cli/commands/run/native.rs`):
- Splits into three independent helpers: `write_sanitized_dotfiles`, `create_passthrough_symlinks`,
  `copy_auto_dirs`
- All three run concurrently via `tokio::try_join!`
- Safe because `SandboxConfig::validate()` (called at config load time) rejects overlapping
  entries in `auto_passthrough_dirs` / `auto_copy_dirs` / `DEFAULT_DOTFILES`

### Network Isolation
- Three modes: `Host`, `None`, `Bridge`, `Allow(rules)`
- `--network-allow` implies bridge + `CAP_NET_ADMIN` + iptables wrapper
- `resolve_network_mode()` handles CLI/config precedence and conflict detection
- iptables rules: DROP all → ACCEPT loopback → ACCEPT established → ACCEPT DNS → per-rule ACCEPT → exec command

### Configuration
- TOML at `~/.config/mino/config.toml`
- All structs use `#[serde(default)]` for partial configs
- State stored at `~/.local/share/mino/`
- `SandboxConfig::validate()` is called by `ConfigManager::load_from_file` and rejects
  configs where `auto_passthrough_dirs` and `auto_copy_dirs` overlap each other or
  `DEFAULT_DOTFILES`

## Cache System

Content-addressed caching keyed by lockfile SHA256 hash.

**States:**
- `Miss` -> create volume, mount read-write
- `Building` -> mount read-write (resume after crash)
- `Complete` -> skip re-finalization, eligible for GC

**Volume naming:** `mino-cache-{ecosystem}-{hash12}`

**Labels:** Stored on Podman volumes via `--label`:
- `io.mino.cache=true`
- `io.mino.cache.ecosystem={npm,cargo,...}`
- `io.mino.cache.hash={hash}`
- `io.mino.cache.state={building,complete}`

## Testing

```bash
cargo test              # All tests
cargo test cache        # Cache module tests only
cargo clippy            # Lints
```

## Common Tasks

### Adding a new lockfile type
1. Add variant to `Ecosystem` enum in `src/cache/lockfile.rs`
2. Add pattern in `lockfile_patterns()`
3. Add env vars in `cache_env_vars()`
4. Add display/parse in `fmt::Display` and `parse_ecosystem()`

### Adding a CLI flag
1. Add to `RunArgs` in `src/cli/args.rs`
2. Handle in `src/cli/commands/run/mod.rs` (or relevant submodule)

### Adding a config option
1. Add field to struct in `src/config/schema.rs`
2. Update `Default` impl
3. Document in README.md

## Container Images & Layers

Only `mino-base` is a pre-built GHCR image. Language toolchains use the layer composition system.

### Adding a new language layer

1. Create `images/{language}/layer.toml` with metadata, env vars, and cache paths

2. Create `images/{language}/install.sh`:
   - Must be idempotent, runs as root
   - End with `--version` verification checks
   - Mark executable: `chmod +x`

3. Add `include_str!` in `src/layer/resolve.rs` for the new layer files

4. Add alias in `src/cli/commands/run/image.rs` `image_alias_to_layer()`:
   ```rust
   "{language}" | "{alias}" => Some("{language}"),
   ```

5. Update `images/README.md` with tools inventory

### Layer design principles

- Layers compose on top of `mino-base` (shared tools, Node for Claude Code)
- Install scripts run as root, layer.toml configures env vars
- Configure cache paths via env vars (CARGO_HOME, npm_config_cache, etc.)
- Use LTS/stable versions
- End install.sh with `--version` verification commands
- Keep layers minimal - don't add tools that aren't commonly needed
