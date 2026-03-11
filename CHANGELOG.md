# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.3.0] - 2026-03-11

### Added

- `--read-only` flag for immutable container filesystems — enhances sandbox security by preventing writes outside mounted volumes (#38).
- Python language layer with uv, ruff, and pytest toolchain support (#26).
- `mino completions <shell>` command for bash, zsh, fish, elvish, and PowerShell (#28).
- Parallelized volume queries for faster `cache list` and `cache clear` operations (#39).

### Changed

- Decomposed monolithic `run.rs` into focused submodules for maintainability (#23).
- Deduplicated volume JSON parsing between NativePodmanRuntime and OrbStackRuntime into shared helpers (#40).

### Removed

- Local Homebrew formula copy (now maintained in `dean0x/homebrew-tap`).

## [1.2.2] - 2026-03-04

### Fixed

- Cache finalization now writes sidecar state files, fixing the no-op where Podman volume labels were immutable after creation so caches never transitioned from `building` to `complete`. Adds detached-mode background monitor for automatic finalization (#20).
- `config set --local` now preserves TOML comments using `toml_edit` round-trip instead of stripping them (#6).
- Added `--no-ssh-agent` and `--no-github` negatable flags so users can disable these default-on features from CLI (#21).
- Credential provider failures now surface visibly between spinner phases instead of silently failing; added `--strict-credentials` flag for CI use (#25).

## [1.2.1] - 2026-03-03

### Security

- Remove `session.default_project_dir` config option to eliminate trust gate bypass vulnerability. Malicious `.mino.toml` could redirect project mount via symlink without triggering trust prompt. Now only `--project` flag or current working directory are used.
- Harden trust gate to include `container.workdir` and `vm.*` fields in sensitive key enumeration.
- Redact credential values from debug log output to prevent accidental exposure in CI logs or crash reports.

## [1.2.0] - 2026-02-21

### Breaking Changes

- Default network mode changed from `host` to `bridge`. Containers are now isolated from host localhost by default. Use `--network host` or set `container.network = "host"` in config to restore previous behavior.
- All containers now run with `--cap-drop ALL`. Custom images requiring specific Linux capabilities may fail. Allowlist mode (`--network-allow`) automatically adds `CAP_NET_ADMIN`.
- Container processes limited to 4096 PIDs (`--pids-limit 4096`).

### Added

- `--network-preset dev|registries` flag with built-in allowlists for common services (GitHub, npm, crates.io, PyPI, AI APIs).
- Interactive network mode prompt on first run — saves choice to config so it never prompts again.
- `--security-opt no-new-privileges` on all containers to prevent privilege escalation.
- Container removal after all sessions (interactive and detached) to prevent credential persistence via `podman inspect`. Detached containers use `--rm` for automatic cleanup on process exit.
- `capsh --drop=cap_net_admin` after iptables setup in allowlist mode — irrecoverably drops the capability before running user commands.
- `libcap` added to base Dockerfile for `capsh` binary.

### Fixed

- Detached containers (`mino run -d`) now auto-removed on exit via `--rm`, closing credential leakage gap where stopped containers exposed env vars via `podman inspect`.
- `mino stop` now tolerates already-removed containers gracefully.

### Security

- Defense-in-depth: capability dropping, privilege escalation prevention, PID limits.
- Allowlist mode now irrecoverably drops `CAP_NET_ADMIN` before executing user commands.
- All containers cleaned up after exit to prevent credential leakage (interactive via explicit removal, detached via `--rm`).

## [1.1.0] - 2025-02-13

### Added

- Interactive layer selection prompt when no image or layers configured.
- `MINO_LAYERS` environment variable for non-interactive layer selection (CI, IDE plugins).
- Progress bar UX for layer composition builds.

### Fixed

- Rootless Podman auto-configuration on fresh OrbStack VMs.
- `cache clear` error handling when stopped containers reference images.
- Layer build UX improvements for long-running installs.

## [1.0.0] - 2025-02-13

### Added

- Network isolation modes: `host`, `none`, `bridge`, and allowlist with iptables egress filtering.
- Composable layer system for multi-toolchain containers (`--layers typescript,rust`).
- Project-local `.mino.toml` configuration with `mino init`.
- Persistent dependency caching with content-addressed volumes.
- Temporary cloud credentials (AWS STS, GCP, Azure).
- SSH agent forwarding.
- Session management (list, stop, logs).
- Homebrew formula, npm package, and crates.io distribution.

### Removed

- Pre-built language images — replaced by layer composition system.
- Dead `src/credentials/` directory (compiler used `src/creds/` via `#[path]`).

## [0.1.0] - 2025-01-31

### Added

- Initial release.
- OrbStack VM management with rootless Podman.
- Container image builds with pre-built binaries.
- Audit logging and session cleanup.
- Basic CLI: `run`, `list`, `stop`, `logs`, `status`, `setup`.

[1.3.0]: https://github.com/dean0x/mino/compare/v1.2.2...v1.3.0
[1.2.2]: https://github.com/dean0x/mino/compare/v1.2.1...v1.2.2
[1.2.1]: https://github.com/dean0x/mino/compare/v1.2.0...v1.2.1
[1.2.0]: https://github.com/dean0x/mino/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/dean0x/mino/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/dean0x/mino/compare/v0.1.0...v1.0.0
[0.1.0]: https://github.com/dean0x/mino/releases/tag/v0.1.0
