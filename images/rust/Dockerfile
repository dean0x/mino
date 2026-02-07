# Minotaur Rust Image
# rustup + stable toolchain + development tools
#
# Extends minotaur-base with Rust development tools.

ARG BASE_IMAGE=ghcr.io/dean0x/minotaur-base:latest
FROM ${BASE_IMAGE}

LABEL org.opencontainers.image.source="https://github.com/dean0x/minotaur"
LABEL org.opencontainers.image.description="Minotaur Rust development image"

USER root

# Install rustup and toolchain in shared location
ENV RUSTUP_HOME=/opt/rustup
ENV CARGO_HOME=/opt/cargo
ENV PATH="${CARGO_HOME}/bin:${PATH}"

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --profile minimal \
    && rustup component add rustfmt clippy \
    # Install cargo-binstall for fast binary downloads
    && curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    # bacon: TUI file watcher (replaces archived cargo-watch)
    # cargo-edit: add/rm/upgrade commands
    # cargo-outdated: check for outdated dependencies
    # cargo-nextest: structured test output with per-test timing
    # sccache: shared compilation cache
    && cargo binstall -y bacon cargo-edit cargo-outdated cargo-nextest sccache \
    && chmod -R a+rX ${RUSTUP_HOME} ${CARGO_HOME} \
    && chown -R developer:developer ${CARGO_HOME} ${RUSTUP_HOME}

USER developer

# Configure cargo home for caching
# When minotaur mounts /cache, cargo will use /cache/cargo
ENV CARGO_HOME=/cache/cargo

# Keep rustup and toolchain paths accessible
ENV RUSTUP_HOME=/opt/rustup
ENV PATH="/opt/cargo/bin:${PATH}"

# sccache configuration
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/cache/sccache

# Verify installations
RUN rustc --version \
    && cargo --version \
    && rustfmt --version \
    && cargo clippy --version \
    && bacon --version \
    && cargo add --help > /dev/null \
    && cargo outdated --version \
    && cargo nextest --version \
    && sccache --version

WORKDIR /workspace
CMD ["/bin/zsh"]
