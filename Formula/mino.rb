class Mino < Formula
  desc "Secure AI agent sandbox wrapper using Podman rootless containers"
  homepage "https://github.com/dean0x/mino"
  version "1.2.2"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-apple-darwin.tar.gz"
      sha256 "5aa7296172bc93b3cde45d0b8dccfff916c8284bacc43a41ff53fbe4e7db9a37"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-apple-darwin.tar.gz"
      sha256 "8e1514fff695f37dbece21905939dd09952d5032cab9380323d1c977a891e254"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "acab8b9ed30e7015f3d63b47a177d27e0623ffe2ccd23b89f0b3edf74ec286a7"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "9578ae0a3161bbfdf06ee1840b5ab927f146f0b5f42dbf7db2a58fa77d9d1d8c"
    end
  end

  def install
    bin.install "mino"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/mino --version")
  end
end
