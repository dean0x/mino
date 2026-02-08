# Mino Rust Image
# rustup + stable toolchain + development tools
#
# Extends mino-base with Rust development tools.

ARG BASE_IMAGE=ghcr.io/dean0x/mino-base:latest
FROM ${BASE_IMAGE}

LABEL org.opencontainers.image.source="https://github.com/dean0x/mino"
LABEL org.opencontainers.image.description="Mino Rust development image"

USER root

# Install rustup and toolchain in shared location
ENV RUSTUP_HOME=/opt/rustup
ENV CARGO_HOME=/opt/cargo
ENV PATH="${CARGO_HOME}/bin:${PATH}"

COPY install.sh /tmp/install-rust.sh
RUN chmod +x /tmp/install-rust.sh && /tmp/install-rust.sh && rm /tmp/install-rust.sh

USER developer

# Configure cargo home for caching
# When mino mounts /cache, cargo will use /cache/cargo
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
