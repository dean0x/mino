class Mino < Formula
  desc "Secure AI agent sandbox wrapper using Podman rootless containers"
  homepage "https://github.com/dean0x/mino"
  version "1.2.1"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-apple-darwin.tar.gz"
      sha256 "ed70f21df3f67dc7905ef1158bbbd3d730a632a958e725c8e64ca6531f39aff6"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-apple-darwin.tar.gz"
      sha256 "04646b7ba5e9a02d832652f39cc3cdbc6f4f44cd3d693eece23dad5381d8cb91"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "fc8d4fe5c62ce7381e4db1c4a99c06fd3eb1dd60feb3c71f6f36a63c75a24327"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "06804c19871c989b7deb7c0dbdbc14823f1960d16d542bf9070715fd0aa5d808"
    end
  end

  def install
    bin.install "mino"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/mino --version")
  end
end
