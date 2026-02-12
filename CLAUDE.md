# Mino - Development Guide

## Project Overview

Mino is a secure sandbox wrapper for AI coding agents. It runs commands in rootless Podman containers with temporary cloud credentials, SSH agent forwarding, and persistent dependency caching.

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
│   ├── mod.rs                 # ConfigManager
│   └── schema.rs              # TOML config structs
├── orchestration/
│   ├── runtime.rs             # ContainerRuntime trait
│   ├── native_podman.rs       # Linux implementation
│   ├── orbstack_runtime.rs    # macOS implementation
│   ├── orbstack.rs            # OrbStack VM management
│   └── factory.rs             # Platform detection
├── session/                   # Session state (JSON files)
└── creds/                     # Cloud credential providers
```

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

### Configuration
- TOML at `~/.config/mino/config.toml`
- All structs use `#[serde(default)]` for partial configs
- State stored at `~/.local/share/mino/`

## Cache System

Content-addressed caching keyed by lockfile SHA256 hash.

**States:**
- `Miss` -> create volume, mount read-write
- `Building` -> mount read-write (resume after crash)
- `Complete` -> mount read-only (immutable)

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
2. Handle in `src/cli/commands/run.rs`

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

4. Add alias in `src/cli/commands/run.rs` `image_alias_to_layer()`:
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
