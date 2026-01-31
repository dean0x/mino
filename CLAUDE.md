# Minotaur - Development Guide

## Project Overview

Minotaur is a secure sandbox wrapper for AI coding agents. It runs commands in rootless Podman containers with temporary cloud credentials, SSH agent forwarding, and persistent dependency caching.

## Architecture

```
src/
├── main.rs                    # CLI entry point
├── lib.rs                     # Library exports
├── error.rs                   # MinotaurError enum, Result type
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
- All functions return `MinotaurResult<T>` (never panic in business logic)
- Use `MinotaurError::io()` for IO errors with context
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
- TOML at `~/.config/minotaur/config.toml`
- All structs use `#[serde(default)]` for partial configs
- State stored at `~/.local/share/minotaur/`

## Cache System

Content-addressed caching keyed by lockfile SHA256 hash.

**States:**
- `Miss` → create volume, mount read-write
- `Building` → mount read-write (resume after crash)
- `Complete` → mount read-only (immutable)

**Volume naming:** `minotaur-cache-{ecosystem}-{hash12}`

**Labels:** Stored on Podman volumes via `--label`:
- `io.minotaur.cache=true`
- `io.minotaur.cache.ecosystem={npm,cargo,...}`
- `io.minotaur.cache.hash={hash}`
- `io.minotaur.cache.state={building,complete}`

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
