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
ARG NVIM_VERSION=v0.11.0
ARG YQ_VERSION=v4.44.3
ARG TOKEI_VERSION=v12.1.2
ARG EZA_VERSION=v0.20.22
ARG SD_VERSION=v1.0.0

# SHA256 checksums for CLI tool downloads (per architecture)
# Format: TOOL_SHA256_ARCH — update these when bumping tool versions
ARG RG_SHA256_X86_64=4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef28337eeae4d8e
ARG RG_SHA256_AARCH64=c827481c4ff4ea10c9dc7a4022c8de5db34a5737cb74484d62eb94a95841ab2f
ARG FD_SHA256_X86_64=d9bfa25ec28624545c222992e1b00673b7c9ca5eb15393c40369f10b28f9c932
ARG FD_SHA256_AARCH64=4e8e596646d047d904f2c5ca74b39dccc69978b6e1fb101094e534b0b59c1bb0
ARG BAT_SHA256_X86_64=d39a21e3da57fe6a3e07184b3c1dc245f8dba379af569d3668b6dcdfe75e3052
ARG BAT_SHA256_AARCH64=feccae9a0576d97609c57e32d3914c5116136eab0df74c2ab74ef397d42c5b10
ARG FZF_SHA256_X86_64=a3c087a5f40e8bb4d9bfb26faffa094643df111a469646bef53154a54af9ff92
ARG FZF_SHA256_AARCH64=e0b3fd1bb769997907d373b0511401801cd643ce939d26ad42e9fe2836bed625
ARG DELTA_SHA256_X86_64=b7ea845004762358a00ef9127dd9fd723e333c7e4b9cb1da220c3909372310ee
ARG DELTA_SHA256_AARCH64=adf7674086daa4582f598f74ce9caa6b70c1ba8f4a57d2911499b37826b014f9
ARG ZOXIDE_SHA256_X86_64=eaa18f8aca1f02e1a132a9eac21b630918f96fa1cc7486ab87e7053dad37b004
ARG ZOXIDE_SHA256_AARCH64=7da16a093b2ac16d577dc53d1f74a8199fc824c963fa3aaf77a6d9f294a895d5
ARG GH_SHA256_X86_64=762569efe785082b7d1feb06995efece1a9cecce16da8503ac6fdbcbea04085b
ARG GH_SHA256_AARCH64=8bcec9f3ee5c7c3700359a75da774c71221064a0ba017537795aa32ac8bbb481
ARG NVIM_SHA256_X86_64=fe0a5bc79e64c5e4d9f844cd96157ebd3919ef1343b329e9ebc3f455924cc7d6
ARG NVIM_SHA256_AARCH64=6959583f45042da20e0a082dc65108725629a4739ab91246c678fd084ecaf50e
ARG YQ_SHA256_X86_64=a2c097180dd884a8d50c956ee16a9cec070f30a7947cf4ebf87d5f36213e9ed7
ARG YQ_SHA256_AARCH64=0e7e1524f68d91b3ff9b089872d185940ab0fa020a5a9052046ef10547023156
ARG TOKEI_SHA256_X86_64=c8c5c4ab9e1ff47e745de70f4af3214078657399fa7a0da0b5f209d780e49978
ARG TOKEI_SHA256_AARCH64=ef514fd12cfc3ee2d1725e5ecb866ee1123163004879ec285b22f3323389ebe2
ARG EZA_SHA256_X86_64=60340f26af56cccba5e34ee04ee2917aea27e44ac960a3716e01d75f07f74ef7
ARG EZA_SHA256_AARCH64=617b76b587213058ae15cfd3d5ebac63c8981315aed36f1e7ae0d858ee522e11
ARG SD_SHA256_X86_64=297202d8cc09cb65c9d7afdfc3f23146371d0c7d7d3448cecab11685d47c709a
ARG SD_SHA256_AARCH64=9e99f8e9dd70c7015b8c20e49270bdd99e9caf966a41a11982f4d785f6a58c9e

# Download all CLI tools into /opt/mino-tools/ in a single layer.
# Each tool is downloaded to a temp file, SHA256-verified, then extracted.
RUN set -e && mkdir -p /opt/mino-tools && ARCH=$(uname -m) && \
    # --- ripgrep ---
    case "$ARCH" in \
        x86_64)  RG_TARGET="x86_64-unknown-linux-musl"; RG_SHA256="${RG_SHA256_X86_64}" ;; \
        aarch64) RG_TARGET="aarch64-unknown-linux-gnu"; RG_SHA256="${RG_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/BurntSushi/ripgrep/releases/download/${RG_VERSION}/ripgrep-${RG_VERSION}-${RG_TARGET}.tar.gz" \
        -o /tmp/rg.tar.gz && \
    echo "${RG_SHA256}  /tmp/rg.tar.gz" | sha256sum -c - && \
    tar -xz --strip-components=1 -C /opt/mino-tools -f /tmp/rg.tar.gz "ripgrep-${RG_VERSION}-${RG_TARGET}/rg" && \
    rm /tmp/rg.tar.gz && \
    # --- fd ---
    curl -fsSL "https://github.com/sharkdp/fd/releases/download/${FD_VERSION}/fd-${FD_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" \
        -o /tmp/fd.tar.gz && \
    case "$ARCH" in \
        x86_64)  echo "${FD_SHA256_X86_64}  /tmp/fd.tar.gz" ;; \
        aarch64) echo "${FD_SHA256_AARCH64}  /tmp/fd.tar.gz" ;; \
    esac | sha256sum -c - && \
    tar -xz --strip-components=1 -C /opt/mino-tools -f /tmp/fd.tar.gz "fd-${FD_VERSION}-${ARCH}-unknown-linux-musl/fd" && \
    rm /tmp/fd.tar.gz && \
    # --- bat ---
    case "$ARCH" in \
        x86_64)  BAT_TARGET="x86_64-unknown-linux-musl"; BAT_SHA256="${BAT_SHA256_X86_64}" ;; \
        aarch64) BAT_TARGET="aarch64-unknown-linux-gnu"; BAT_SHA256="${BAT_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/sharkdp/bat/releases/download/${BAT_VERSION}/bat-${BAT_VERSION}-${BAT_TARGET}.tar.gz" \
        -o /tmp/bat.tar.gz && \
    echo "${BAT_SHA256}  /tmp/bat.tar.gz" | sha256sum -c - && \
    tar -xz --strip-components=1 -C /opt/mino-tools -f /tmp/bat.tar.gz "bat-${BAT_VERSION}-${BAT_TARGET}/bat" && \
    rm /tmp/bat.tar.gz && \
    # --- fzf ---
    case "$ARCH" in \
        x86_64)  FZF_ARCH="amd64"; FZF_SHA256="${FZF_SHA256_X86_64}" ;; \
        aarch64) FZF_ARCH="arm64"; FZF_SHA256="${FZF_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/junegunn/fzf/releases/download/${FZF_VERSION}/fzf-${FZF_VERSION#v}-linux_${FZF_ARCH}.tar.gz" \
        -o /tmp/fzf.tar.gz && \
    echo "${FZF_SHA256}  /tmp/fzf.tar.gz" | sha256sum -c - && \
    tar -xz -C /opt/mino-tools -f /tmp/fzf.tar.gz fzf && \
    rm /tmp/fzf.tar.gz && \
    # --- delta (git diff) ---
    case "$ARCH" in \
        x86_64)  DELTA_TARGET="x86_64-unknown-linux-musl"; DELTA_SHA256="${DELTA_SHA256_X86_64}" ;; \
        aarch64) DELTA_TARGET="aarch64-unknown-linux-gnu"; DELTA_SHA256="${DELTA_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/dandavison/delta/releases/download/${DELTA_VERSION}/delta-${DELTA_VERSION}-${DELTA_TARGET}.tar.gz" \
        -o /tmp/delta.tar.gz && \
    echo "${DELTA_SHA256}  /tmp/delta.tar.gz" | sha256sum -c - && \
    tar -xz --strip-components=1 -C /opt/mino-tools -f /tmp/delta.tar.gz "delta-${DELTA_VERSION}-${DELTA_TARGET}/delta" && \
    rm /tmp/delta.tar.gz && \
    # --- zoxide (smart cd) ---
    curl -fsSL "https://github.com/ajeetdsouza/zoxide/releases/download/${ZOXIDE_VERSION}/zoxide-${ZOXIDE_VERSION#v}-${ARCH}-unknown-linux-musl.tar.gz" \
        -o /tmp/zoxide.tar.gz && \
    case "$ARCH" in \
        x86_64)  echo "${ZOXIDE_SHA256_X86_64}  /tmp/zoxide.tar.gz" ;; \
        aarch64) echo "${ZOXIDE_SHA256_AARCH64}  /tmp/zoxide.tar.gz" ;; \
    esac | sha256sum -c - && \
    tar -xz -C /opt/mino-tools -f /tmp/zoxide.tar.gz zoxide && \
    rm /tmp/zoxide.tar.gz && \
    # --- GitHub CLI ---
    case "$ARCH" in \
        x86_64)  GH_ARCH="amd64"; GH_SHA256="${GH_SHA256_X86_64}" ;; \
        aarch64) GH_ARCH="arm64"; GH_SHA256="${GH_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${GH_ARCH}.tar.gz" \
        -o /tmp/gh.tar.gz && \
    echo "${GH_SHA256}  /tmp/gh.tar.gz" | sha256sum -c - && \
    tar -xz --strip-components=2 -C /opt/mino-tools -f /tmp/gh.tar.gz "gh_${GH_VERSION}_linux_${GH_ARCH}/bin/gh" && \
    rm /tmp/gh.tar.gz && \
    # --- neovim ---
    case "$ARCH" in \
        x86_64)  NVIM_ARCH="x86_64"; NVIM_SHA256="${NVIM_SHA256_X86_64}" ;; \
        aarch64) NVIM_ARCH="arm64"; NVIM_SHA256="${NVIM_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/neovim/neovim/releases/download/${NVIM_VERSION}/nvim-linux-${NVIM_ARCH}.tar.gz" \
        -o /tmp/nvim.tar.gz && \
    echo "${NVIM_SHA256}  /tmp/nvim.tar.gz" | sha256sum -c - && \
    tar -xz --strip-components=2 -C /opt/mino-tools -f /tmp/nvim.tar.gz "nvim-linux-${NVIM_ARCH}/bin/nvim" && \
    rm /tmp/nvim.tar.gz && \
    # --- yq (YAML processor) ---
    case "$ARCH" in \
        x86_64)  YQ_ARCH="amd64"; YQ_SHA256="${YQ_SHA256_X86_64}" ;; \
        aarch64) YQ_ARCH="arm64"; YQ_SHA256="${YQ_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/mikefarah/yq/releases/download/${YQ_VERSION}/yq_linux_${YQ_ARCH}" \
        -o /tmp/yq && \
    echo "${YQ_SHA256}  /tmp/yq" | sha256sum -c - && \
    mv /tmp/yq /opt/mino-tools/yq && \
    # --- tokei (code statistics) ---
    curl -fsSL "https://github.com/XAMPPRocky/tokei/releases/download/${TOKEI_VERSION}/tokei-${ARCH}-unknown-linux-gnu.tar.gz" \
        -o /tmp/tokei.tar.gz && \
    case "$ARCH" in \
        x86_64)  echo "${TOKEI_SHA256_X86_64}  /tmp/tokei.tar.gz" ;; \
        aarch64) echo "${TOKEI_SHA256_AARCH64}  /tmp/tokei.tar.gz" ;; \
    esac | sha256sum -c - && \
    tar -xz -C /opt/mino-tools -f /tmp/tokei.tar.gz tokei && \
    rm /tmp/tokei.tar.gz && \
    # --- eza (modern ls/tree replacement) ---
    curl -fsSL "https://github.com/eza-community/eza/releases/download/${EZA_VERSION}/eza_${ARCH}-unknown-linux-gnu.tar.gz" \
        -o /tmp/eza.tar.gz && \
    case "$ARCH" in \
        x86_64)  echo "${EZA_SHA256_X86_64}  /tmp/eza.tar.gz" ;; \
        aarch64) echo "${EZA_SHA256_AARCH64}  /tmp/eza.tar.gz" ;; \
    esac | sha256sum -c - && \
    tar -xz -C /opt/mino-tools -f /tmp/eza.tar.gz ./eza && \
    rm /tmp/eza.tar.gz && \
    # --- sd (modern sed replacement) ---
    case "$ARCH" in \
        x86_64)  SD_TARGET="x86_64-unknown-linux-gnu"; SD_SHA256="${SD_SHA256_X86_64}" ;; \
        aarch64) SD_TARGET="aarch64-unknown-linux-musl"; SD_SHA256="${SD_SHA256_AARCH64}" ;; \
        *) echo "Unsupported architecture: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/chmln/sd/releases/download/${SD_VERSION}/sd-${SD_VERSION}-${SD_TARGET}.tar.gz" \
        -o /tmp/sd.tar.gz && \
    echo "${SD_SHA256}  /tmp/sd.tar.gz" | sha256sum -c - && \
    tar -xz --strip-components=1 -C /opt/mino-tools -f /tmp/sd.tar.gz "sd-${SD_VERSION}-${SD_TARGET}/sd" && \
    rm /tmp/sd.tar.gz && \
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
