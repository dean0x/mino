# Mino Base Image
# Fedora 43 + Node.js 22 LTS + productivity tools + Claude Code
#
# This is the foundation for all mino language images.
# Contains shell tools, git utilities, and the Claude Code CLI.
#
# CLI tools live in /opt/mino-tools/ and are symlinked to ~/.local/bin/
# by the bootstrap script. Users can override any tool by placing their
# own binary in ~/.local/bin/ — bootstrap won't overwrite real files.

FROM fedora:43

LABEL org.opencontainers.image.source="https://github.com/dean0x/mino"
LABEL org.opencontainers.image.description="Mino base image with Claude Code and development tools"

# Avoid interactive prompts
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8

# System packages — only things that genuinely require root/dnf
RUN dnf install -y --setopt=install_weak_deps=False \
    # Core utilities
    git curl wget jq openssh-clients ca-certificates tar gzip unzip which \
    # Shell
    zsh \
    # Networking & capabilities
    procps-ng httpie libcap \
    # Build essentials for native modules
    gcc gcc-c++ make pkgconf \
    && dnf clean all \
    && rm -rf /var/cache/dnf

# CLI tool versions (ARGs declared before the single consolidated RUN)
ARG RG_VERSION=14.1.1
ARG FD_VERSION=v10.2.0
ARG BAT_VERSION=v0.24.0
ARG FZF_VERSION=v0.57.0
ARG DELTA_VERSION=0.18.2
ARG ZOXIDE_VERSION=v0.9.6
ARG GH_VERSION=2.65.0
ARG NVIM_VERSION=v0.10.3
ARG YQ_VERSION=v4.44.3
ARG TOKEI_VERSION=v12.1.2
ARG EZA_VERSION=v0.20.22
ARG SD_VERSION=v1.0.0

# Download all CLI tools into /opt/mino-tools/ in a single layer
RUN set -e && mkdir -p /opt/mino-tools && ARCH=$(uname -m) && \
    # --- ripgrep ---
    curl -fsSL "https://github.com/BurntSushi/ripgrep/releases/download/${RG_VERSION}/ripgrep-${RG_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" | \
    tar -xz --strip-components=1 -C /opt/mino-tools "ripgrep-${RG_VERSION}-${ARCH}-unknown-linux-musl/rg" && \
    # --- fd ---
    curl -fsSL "https://github.com/sharkdp/fd/releases/download/${FD_VERSION}/fd-${FD_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" | \
    tar -xz --strip-components=1 -C /opt/mino-tools "fd-${FD_VERSION}-${ARCH}-unknown-linux-musl/fd" && \
    # --- bat ---
    curl -fsSL "https://github.com/sharkdp/bat/releases/download/${BAT_VERSION}/bat-${BAT_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" | \
    tar -xz --strip-components=1 -C /opt/mino-tools "bat-${BAT_VERSION}-${ARCH}-unknown-linux-musl/bat" && \
    # --- fzf ---
    case "$ARCH" in \
        x86_64) FZF_ARCH="amd64" ;; \
        aarch64) FZF_ARCH="arm64" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/junegunn/fzf/releases/download/${FZF_VERSION}/fzf-${FZF_VERSION#v}-linux_${FZF_ARCH}.tar.gz" | \
    tar -xz -C /opt/mino-tools fzf && \
    # --- delta (git diff) ---
    curl -fsSL "https://github.com/dandavison/delta/releases/download/${DELTA_VERSION}/delta-${DELTA_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" | \
    tar -xz --strip-components=1 -C /opt/mino-tools "delta-${DELTA_VERSION}-${ARCH}-unknown-linux-musl/delta" && \
    # --- zoxide (smart cd) ---
    curl -fsSL "https://github.com/ajeetdsouza/zoxide/releases/download/${ZOXIDE_VERSION}/zoxide-${ZOXIDE_VERSION#v}-${ARCH}-unknown-linux-musl.tar.gz" | \
    tar -xz -C /opt/mino-tools zoxide && \
    # --- GitHub CLI ---
    case "$ARCH" in \
        x86_64) GH_ARCH="amd64" ;; \
        aarch64) GH_ARCH="arm64" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${GH_ARCH}.tar.gz" | \
    tar -xz --strip-components=2 -C /opt/mino-tools "gh_${GH_VERSION}_linux_${GH_ARCH}/bin/gh" && \
    # --- neovim ---
    case "$ARCH" in \
        x86_64) NVIM_ARCH="x86_64" ;; \
        aarch64) NVIM_ARCH="aarch64" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/neovim/neovim/releases/download/${NVIM_VERSION}/nvim-linux-${NVIM_ARCH}.tar.gz" | \
    tar -xz --strip-components=2 -C /opt/mino-tools "nvim-linux-${NVIM_ARCH}/bin/nvim" && \
    # --- yq (YAML processor) ---
    case "$ARCH" in \
        x86_64) YQ_ARCH="amd64" ;; \
        aarch64) YQ_ARCH="arm64" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/mikefarah/yq/releases/download/${YQ_VERSION}/yq_linux_${YQ_ARCH}" \
        -o /opt/mino-tools/yq && \
    # --- tokei (code statistics) ---
    curl -fsSL "https://github.com/XAMPPRocky/tokei/releases/download/${TOKEI_VERSION}/tokei-${ARCH}-unknown-linux-gnu.tar.gz" | \
    tar -xz -C /opt/mino-tools tokei && \
    # --- eza (modern ls/tree replacement) ---
    curl -fsSL "https://github.com/eza-community/eza/releases/download/${EZA_VERSION}/eza_${ARCH}-unknown-linux-gnu.tar.gz" | \
    tar -xz -C /opt/mino-tools ./eza && \
    # --- sd (modern sed replacement) ---
    case "$ARCH" in \
        x86_64) SD_TARGET="x86_64-unknown-linux-gnu" ;; \
        aarch64) SD_TARGET="aarch64-unknown-linux-musl" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/chmln/sd/releases/download/${SD_VERSION}/sd-${SD_VERSION}-${SD_TARGET}.tar.gz" | \
    tar -xz --strip-components=1 -C /opt/mino-tools "sd-${SD_VERSION}-${SD_TARGET}/sd" && \
    # --- set permissions on all tools ---
    chmod +x /opt/mino-tools/*

# Create non-root user with pinned UID for volume stability
RUN useradd -m -s /bin/zsh -u 1000 developer \
    && mkdir -p /workspace /cache \
    && chown -R developer:developer /workspace /cache

# PATH: ~/.local/bin (symlinks) > ~/.npm-global/bin > /opt/mino-tools (fallback)
ENV PATH="/home/developer/.local/bin:/home/developer/.npm-global/bin:/opt/mino-tools:${PATH}"

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
# Modern CLI replacements (resolve via PATH — works from ~/.local/bin or /opt/mino-tools)
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

# Layer-injected PATH prepend (set by mino for user-install layers)
if [ -n "$MINO_PATH_PREPEND" ]; then
    export PATH="$MINO_PATH_PREPEND:$PATH"
fi
MINORC

# Source system zshrc.d files from /etc/zsh/zshrc (before user's ~/.zshrc)
RUN echo 'for f in /etc/zsh/zshrc.d/*.zsh(N); do source "$f"; done' >> /etc/zsh/zshrc

# Bootstrap script + entrypoint
COPY mino-bootstrap mino-entrypoint /usr/local/bin/
RUN chmod +x /usr/local/bin/mino-bootstrap /usr/local/bin/mino-entrypoint

# Write image version for bootstrap staleness detection
ARG MINO_BASE_VERSION=dev
RUN echo "$MINO_BASE_VERSION" > /etc/mino-base-version

# Healthcheck script (uses nvm node via PATH — sourced in bootstrap)
RUN printf '#!/bin/bash\nset -e\ncommand -v claude >/dev/null 2>&1\ncommand -v git >/dev/null 2>&1\n# Check node via nvm or PATH\nif [ -d "$HOME/.nvm" ]; then\n  export NVM_DIR="$HOME/.nvm"\n  [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"\nfi\ncommand -v node >/dev/null 2>&1\ntest -d /workspace\ntest -d /cache\ntest "$(whoami)" = "developer"\necho "ok"\n' \
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

# nvm (Node Version Manager) — install nvm + Node.js 22 LTS
ARG NVM_VERSION=v0.40.1
RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/${NVM_VERSION}/install.sh | bash \
    && bash -c 'source ~/.nvm/nvm.sh && nvm install 22 && nvm alias default 22'

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

# Verify CLI tools in /opt/mino-tools/
RUN /opt/mino-tools/rg --version \
    && /opt/mino-tools/fd --version \
    && /opt/mino-tools/bat --version \
    && /opt/mino-tools/fzf --version \
    && /opt/mino-tools/delta --version \
    && /opt/mino-tools/zoxide --version \
    && /opt/mino-tools/gh --version \
    && /opt/mino-tools/nvim --version | head -1 \
    && /opt/mino-tools/yq --version \
    && /opt/mino-tools/tokei --version \
    && /opt/mino-tools/eza --version \
    && /opt/mino-tools/sd --version

# Verify user-level installs
RUN claude --version \
    && git --version \
    && bash -c 'source ~/.nvm/nvm.sh && node --version && npm --version'

HEALTHCHECK CMD mino-healthcheck

ENTRYPOINT ["/usr/local/bin/mino-entrypoint"]
CMD ["/bin/zsh"]
