# Minotaur TypeScript Image
# Node.js 22 LTS + pnpm + TypeScript toolchain
#
# Extends minotaur-base with TypeScript/Node development tools.

ARG BASE_IMAGE=ghcr.io/dean0x/minotaur-base:latest
FROM ${BASE_IMAGE}

LABEL org.opencontainers.image.source="https://github.com/dean0x/minotaur"
LABEL org.opencontainers.image.description="Minotaur TypeScript development image"

USER root

# Install pnpm, tsx, typescript, npm-check-updates, biome, turbo, vite
RUN npm install -g pnpm tsx typescript npm-check-updates @biomejs/biome turbo vite \
    && npm cache clean --force

USER developer

# Configure cache directories
# These will be overridden when minotaur mounts /cache
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
