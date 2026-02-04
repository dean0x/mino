# Minotaur Rust Image
# rustup + stable toolchain + development tools
#
# Extends minotaur-base with Rust development tools.

ARG BASE_IMAGE=ghcr.io/dean0x/minotaur-base:latest
FROM ${BASE_IMAGE}

LABEL org.opencontainers.image.source="https://github.com/dean0x/minotaur"
LABEL org.opencontainers.image.description="Minotaur Rust development image"

USER root

# Install OpenSSL dev headers (needed by cargo-outdated)
RUN dnf install -y --setopt=install_weak_deps=False openssl-devel && dnf clean all

# Install rustup and toolchain in shared location
ENV RUSTUP_HOME=/opt/rustup
ENV CARGO_HOME=/opt/cargo
ENV PATH="${CARGO_HOME}/bin:${PATH}"

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --profile minimal \
    && rustup component add rustfmt clippy \
    # bacon: TUI file watcher (replaces archived cargo-watch)
    # cargo-edit: add/rm/upgrade commands
    # cargo-outdated: check for outdated dependencies
    && cargo install bacon cargo-edit cargo-outdated --locked \
    && chmod -R a+rX ${RUSTUP_HOME} ${CARGO_HOME} \
    && chown -R developer:developer ${CARGO_HOME} ${RUSTUP_HOME}

USER developer

# Configure cargo home for caching
# When minotaur mounts /cache, cargo will use /cache/cargo
ENV CARGO_HOME=/cache/cargo

# Keep rustup and toolchain paths accessible
ENV RUSTUP_HOME=/opt/rustup
ENV PATH="/opt/cargo/bin:${PATH}"

# Verify installations
RUN rustc --version \
    && cargo --version \
    && rustfmt --version \
    && cargo clippy --version \
    && bacon --version \
    && cargo add --help > /dev/null \
    && cargo outdated --version

WORKDIR /workspace
CMD ["/bin/zsh"]
