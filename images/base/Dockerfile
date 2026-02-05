# Minotaur Base Image
# Fedora 43 + Node.js 22 LTS + productivity tools + Claude Code
#
# This is the foundation for all minotaur language images.
# Contains shell tools, git utilities, and the Claude Code CLI.

FROM fedora:43

LABEL org.opencontainers.image.source="https://github.com/dean0x/minotaur"
LABEL org.opencontainers.image.description="Minotaur base image with Claude Code and development tools"

# Avoid interactive prompts
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

# System packages (single layer for efficiency)
RUN dnf install -y --setopt=install_weak_deps=False \
    # Core utilities
    git curl wget jq openssh-clients ca-certificates tar gzip unzip which \
    # Modern replacements for grep/find/cat
    ripgrep fd-find bat fzf \
    # Git diff and smart cd (from Fedora repos)
    git-delta zoxide \
    # Editor
    neovim \
    # Shell
    zsh \
    # Networking
    procps-ng httpie \
    # Build essentials for native modules
    gcc gcc-c++ make pkgconf \
    && dnf clean all \
    && rm -rf /var/cache/dnf

# GitHub CLI
RUN dnf install -y 'dnf-command(config-manager)' \
    && dnf config-manager addrepo --from-repofile=https://cli.github.com/packages/rpm/gh-cli.repo \
    && dnf install -y gh \
    && dnf clean all \
    && rm -rf /var/cache/dnf

# Node.js 22 LTS (required for Claude Code)
RUN curl -fsSL https://rpm.nodesource.com/setup_22.x | bash - \
    && dnf install -y nodejs \
    && dnf clean all \
    && rm -rf /var/cache/dnf \
    && node --version && npm --version

# mcfly (shell history search) - download pre-built binary
# git-delta and zoxide are installed via dnf above
ARG MCFLY_VERSION=v0.9.2
RUN ARCH=$(uname -m) && \
    curl -fsSL "https://github.com/cantino/mcfly/releases/download/${MCFLY_VERSION}/mcfly-${MCFLY_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" | \
    tar -xz -C /usr/local/bin mcfly && \
    chmod +x /usr/local/bin/mcfly

# Claude Code CLI
RUN npm install -g @anthropic-ai/claude-code \
    && npm cache clean --force \
    && claude --version

# Create non-root user
RUN useradd -m -s /bin/zsh developer \
    && mkdir -p /workspace /cache \
    && chown -R developer:developer /workspace /cache

# Git configuration for better diffs
RUN git config --system core.pager delta \
    && git config --system delta.navigate true \
    && git config --system delta.line-numbers true \
    && git config --system delta.side-by-side false \
    && git config --system interactive.diffFilter "delta --color-only"

# Shell configuration for developer user
USER developer
WORKDIR /home/developer

# Configure zsh with zoxide and mcfly
RUN echo 'eval "$(zoxide init zsh)"' >> ~/.zshrc \
    && echo 'eval "$(mcfly init zsh)"' >> ~/.zshrc \
    && echo 'export MCFLY_FUZZY=2' >> ~/.zshrc \
    && echo 'export MCFLY_RESULTS=50' >> ~/.zshrc \
    && echo 'alias ls="ls --color=auto"' >> ~/.zshrc \
    && echo 'alias cat="bat --paging=never"' >> ~/.zshrc \
    && echo 'alias find="fd"' >> ~/.zshrc \
    && echo 'alias grep="rg"' >> ~/.zshrc

WORKDIR /workspace

# Verify installations
RUN claude --version \
    && git --version \
    && gh --version \
    && delta --version \
    && node --version \
    && npm --version \
    && rg --version \
    && fd --version \
    && bat --version \
    && fzf --version \
    && nvim --version | head -1 \
    && zoxide --version \
    && mcfly --version

CMD ["/bin/zsh"]
