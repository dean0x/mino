class Mino < Formula
  desc "Secure AI agent sandbox wrapper using Podman rootless containers"
  homepage "https://github.com/dean0x/mino"
  version "1.1.0"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-apple-darwin.tar.gz"
      sha256 "1f5bd6b851d355007ab193bc43b6156087cd1aef6f5d54dfb4eb23c03232e536"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-apple-darwin.tar.gz"
      sha256 "531098c36013e4bb8c15d1015fbfc00d7531daf3c4d45b16aa714529b277f518"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "f105ca8864cc5e06cfe1497d39c572ba5c897b9343fa257e934c30e836f8599c"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "278e95e59d1f049e6112b1ba6b58eaf20962fc5c4c04c90681f2965c3379d199"
    end
  end

  def install
    bin.install "mino"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/mino --version")
  end
end
