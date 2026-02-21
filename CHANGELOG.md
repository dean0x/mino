# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0] - Unreleased

### Breaking Changes

- Default network mode changed from `host` to `bridge`. Containers are now isolated from host localhost by default. Use `--network host` or set `container.network = "host"` in config to restore previous behavior.
- All containers now run with `--cap-drop ALL`. Custom images requiring specific Linux capabilities may fail. Allowlist mode (`--network-allow`) automatically adds `CAP_NET_ADMIN`.
- Container processes limited to 4096 PIDs (`--pids-limit 4096`).

### Added

- `--network-preset dev|registries` flag with built-in allowlists for common services (GitHub, npm, crates.io, PyPI, AI APIs).
- Interactive network mode prompt on first run — saves choice to config so it never prompts again.
- `--security-opt no-new-privileges` on all containers to prevent privilege escalation.
- Container removal after interactive sessions to prevent credential persistence via `podman inspect`.
- `capsh --drop=cap_net_admin` after iptables setup in allowlist mode — irrecoverably drops the capability before running user commands.
- `libcap` added to base Dockerfile for `capsh` binary.

### Security

- Defense-in-depth: capability dropping, privilege escalation prevention, PID limits.
- Allowlist mode now irrecoverably drops `CAP_NET_ADMIN` before executing user commands.
- Stopped containers cleaned up to prevent credential leakage.

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

[1.2.0]: https://github.com/dean0x/mino/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/dean0x/mino/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/dean0x/mino/compare/v0.1.0...v1.0.0
[0.1.0]: https://github.com/dean0x/mino/releases/tag/v0.1.0
