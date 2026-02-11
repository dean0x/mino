# Mino TypeScript Image
# Node.js 22 LTS + pnpm + TypeScript toolchain
#
# Extends mino-base with TypeScript/Node development tools.

ARG BASE_IMAGE=ghcr.io/dean0x/mino-base:latest
FROM ${BASE_IMAGE}

LABEL org.opencontainers.image.source="https://github.com/dean0x/mino"
LABEL org.opencontainers.image.description="Mino TypeScript development image"

USER root

COPY install.sh /tmp/install-typescript.sh
RUN chmod +x /tmp/install-typescript.sh && /tmp/install-typescript.sh && rm /tmp/install-typescript.sh

USER developer

# Configure cache directories
# These will be overridden when mino mounts /cache
ENV PNPM_HOME=/cache/pnpm
ENV npm_config_cache=/cache/npm
ENV NODE_ENV=development

# Add pnpm to PATH
ENV PATH="${PNPM_HOME}:${PATH}"

# Verify installations
RUN pnpm --version \
    && tsx --version \
    && tsc --version \
    && ncu --version \
    && biome --version \
    && turbo --version \
    && vite --version

WORKDIR /workspace
CMD ["/bin/zsh"]
