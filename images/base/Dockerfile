# Mino Base Image
# Fedora 43 + Node.js 22 LTS + productivity tools + Claude Code
#
# This is the foundation for all mino language images.
# Contains shell tools, git utilities, and the Claude Code CLI.

FROM fedora:43

LABEL org.opencontainers.image.source="https://github.com/dean0x/mino"
LABEL org.opencontainers.image.description="Mino base image with Claude Code and development tools"

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
    # Networking & capabilities
    procps-ng httpie libcap \
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

# yq (YAML processor) - download pre-built binary
ARG YQ_VERSION=v4.44.3
RUN ARCH=$(uname -m) && \
    case "$ARCH" in \
        x86_64) YQ_ARCH="amd64" ;; \
        aarch64) YQ_ARCH="arm64" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/mikefarah/yq/releases/download/${YQ_VERSION}/yq_linux_${YQ_ARCH}" \
        -o /usr/local/bin/yq && \
    chmod +x /usr/local/bin/yq

# tokei (code statistics) - download pre-built binary
ARG TOKEI_VERSION=v12.1.2
RUN ARCH=$(uname -m) && \
    curl -fsSL "https://github.com/XAMPPRocky/tokei/releases/download/${TOKEI_VERSION}/tokei-${ARCH}-unknown-linux-gnu.tar.gz" | \
    tar -xz -C /usr/local/bin tokei && \
    chmod +x /usr/local/bin/tokei

# eza (modern ls/tree replacement) - download pre-built binary
ARG EZA_VERSION=v0.20.22
RUN ARCH=$(uname -m) && \
    curl -fsSL "https://github.com/eza-community/eza/releases/download/${EZA_VERSION}/eza_${ARCH}-unknown-linux-gnu.tar.gz" | \
    tar -xz -C /usr/local/bin ./eza && \
    chmod +x /usr/local/bin/eza

# sd (modern sed replacement) - download pre-built binary
ARG SD_VERSION=v1.0.0
RUN ARCH=$(uname -m) && \
    case "$ARCH" in \
        x86_64) SD_TARGET="x86_64-unknown-linux-gnu" ;; \
        aarch64) SD_TARGET="aarch64-unknown-linux-musl" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/chmln/sd/releases/download/${SD_VERSION}/sd-${SD_VERSION}-${SD_TARGET}.tar.gz" | \
    tar -xz --strip-components=1 -C /usr/local/bin "sd-${SD_VERSION}-${SD_TARGET}/sd" && \
    chmod +x /usr/local/bin/sd

# Create non-root user with pinned UID for volume stability
RUN useradd -m -s /bin/zsh -u 1000 developer \
    && mkdir -p /workspace /cache \
    && chown -R developer:developer /workspace /cache

# PATH for user-level tool installs (must be set before Claude Code install)
ENV PATH="/home/developer/.local/bin:/home/developer/.npm-global/bin:${PATH}"

# Git configuration for better diffs
RUN git config --system core.pager delta \
    && git config --system delta.navigate true \
    && git config --system delta.line-numbers true \
    && git config --system delta.side-by-side false \
    && git config --system interactive.diffFilter "delta --color-only"

# System-level shell config (aliases, PATH, tool integrations)
# Loaded BEFORE ~/.zshrc — must not depend on user config (no OMZ sourcing)
RUN mkdir -p /etc/zsh/zshrc.d \
    && cat > /etc/zsh/zshrc.d/mino.zsh << 'MINORC'
# Modern CLI replacements
alias ls="eza --color=auto"
alias ll="eza -la --color=auto"
alias la="eza -a --color=auto"
alias tree="eza --tree"
alias cat="bat --paging=never"
alias find="fd"
alias grep="rg"

# Tool integrations
eval "$(zoxide init zsh)"
source <(fzf --zsh)

# History settings
HISTSIZE=50000
SAVEHIST=50000
setopt HIST_IGNORE_DUPS
setopt HIST_IGNORE_SPACE

# Up/down arrow history substring search keybindings
bindkey '^[[A' history-substring-search-up
bindkey '^[[B' history-substring-search-down

# PATH for user-local tool installs
export PATH="$HOME/.local/bin:$HOME/.npm-global/bin:$PATH"
MINORC

# Source system zshrc.d files from /etc/zsh/zshrc (before user's ~/.zshrc)
RUN echo 'for f in /etc/zsh/zshrc.d/*.zsh(N); do source "$f"; done' >> /etc/zsh/zshrc

# Bootstrap script + entrypoint
COPY mino-bootstrap mino-entrypoint /usr/local/bin/
RUN chmod +x /usr/local/bin/mino-bootstrap /usr/local/bin/mino-entrypoint

# Write image version for bootstrap staleness detection
ARG MINO_BASE_VERSION=dev
RUN echo "$MINO_BASE_VERSION" > /etc/mino-base-version

# Healthcheck script
RUN printf '#!/bin/bash\nset -e\ncommand -v claude >/dev/null 2>&1\ncommand -v git >/dev/null 2>&1\ncommand -v node >/dev/null 2>&1\ntest -d /workspace\ntest -d /cache\ntest "$(whoami)" = "developer"\necho "ok"\n' \
    > /usr/local/bin/mino-healthcheck \
    && chmod +x /usr/local/bin/mino-healthcheck

# Developer user setup
USER developer
WORKDIR /home/developer

# Oh My Zsh (unattended, no shell change prompt)
RUN sh -c "$(curl -fsSL https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)" "" --unattended

# zsh-autosuggestions plugin (grey ghost text as you type)
RUN git clone --depth=1 https://github.com/zsh-users/zsh-autosuggestions \
    ${ZSH_CUSTOM:-~/.oh-my-zsh/custom}/plugins/zsh-autosuggestions

# zsh-history-substring-search (up/down arrows filter by typed prefix)
RUN git clone --depth=1 https://github.com/zsh-users/zsh-history-substring-search \
    ${ZSH_CUSTOM:-~/.oh-my-zsh/custom}/plugins/zsh-history-substring-search

# nvm (Node Version Manager) - system Node remains as fallback
ARG NVM_VERSION=v0.40.1
RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/${NVM_VERSION}/install.sh | bash

# User's ~/.zshrc (OMZ sourcing + user customizable)
# System aliases/tools are in /etc/zsh/zshrc.d/mino.zsh (loaded first)
RUN cat > ~/.zshrc << 'ZSHRC'
# Oh My Zsh configuration
export ZSH="$HOME/.oh-my-zsh"
ZSH_THEME="robbyrussell"
DISABLE_AUTO_UPDATE=true
DISABLE_UPDATE_PROMPT=true
plugins=(git zsh-autosuggestions history-substring-search)
source $ZSH/oh-my-zsh.sh

# nvm
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"
[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"
ZSHRC

# Claude Code CLI (user-level install, updatable)
RUN curl -fsSL https://cli.anthropic.com/install.sh | sh

# Copy developer home to /etc/skel/ for bootstrap on new home volumes
USER root
RUN cp -a /home/developer/.oh-my-zsh /etc/skel/.oh-my-zsh \
    && cp -a /home/developer/.nvm /etc/skel/.nvm \
    && cp -a /home/developer/.local /etc/skel/.local \
    && cp /home/developer/.zshrc /etc/skel/.zshrc \
    && touch /etc/skel/.zsh_history

# Write bootstrap marker so --no-home (no volume mount) skips bootstrap
USER developer
RUN cat /etc/mino-base-version > /home/developer/.mino-bootstrapped

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
    && eza --version \
    && sd --version \
    && yq --version \
    && tokei --version

HEALTHCHECK CMD mino-healthcheck

ENTRYPOINT ["/usr/local/bin/mino-entrypoint"]
CMD ["/bin/zsh"]
