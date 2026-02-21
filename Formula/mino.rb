class Mino < Formula
  desc "Secure AI agent sandbox wrapper using Podman rootless containers"
  homepage "https://github.com/dean0x/mino"
  version "1.2.0"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-apple-darwin.tar.gz"
      sha256 "0b0ec7654d975c420cf367b705d85b2d6475a55386975a3812c5dbff2c866aa6"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-apple-darwin.tar.gz"
      sha256 "5f382007e9d4b7fe0746fb11ef788418636413e9e12e7e5785a99ed0cbbe47b9"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "c73517a994b9ed4b5ad4457f2c8c95a6912faa7c8be2d5e925bde682a89b0c39"
    end

    on_arm do
      url "https://github.com/dean0x/mino/releases/download/v#{version}/mino-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "2d4c3ca0ca00494df463c7a6118f4ec94cc5cccc829c0bf03920d9ff482909c9"
    end
  end

  def install
    bin.install "mino"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/mino --version")
  end
end
