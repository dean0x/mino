class Minotaur < Formula
  desc "Secure AI agent sandbox wrapper using Podman rootless containers"
  homepage "https://github.com/dean0x/minotaur"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-x86_64-apple-darwin.tar.gz"
      sha256 "83ef56a7f0aafb282c1b1b124b147780b92751739ebc3370f0d68e6cf843c428"
    end

    on_arm do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-aarch64-apple-darwin.tar.gz"
      sha256 "59f08bbc32aec7d4bcd2365b6b8354ccdd23e7f9b46ceb00dbd4a1e5e1b35b01"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "adc6765a8d883ed571483f18abfb016962ad68e7cf818d0af7ef8485d99d8d47"
    end

    on_arm do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "97c64473225c29ad09dfe8adf99b25c1bc727364701f88673a5be8d65689dcdb"
    end
  end

  def install
    bin.install "minotaur"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/minotaur --version")
  end
end
