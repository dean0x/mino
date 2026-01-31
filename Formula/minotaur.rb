class Minotaur < Formula
  desc "Secure AI agent sandbox wrapper using Podman rootless containers"
  homepage "https://github.com/dean0x/minotaur"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-x86_64-apple-darwin.tar.gz"
      # PLACEHOLDER: Update SHA256 after first release
      # Run: curl -sL <url> | shasum -a 256
      sha256 "PLACEHOLDER_INTEL_SHA256"
    end

    on_arm do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-aarch64-apple-darwin.tar.gz"
      # PLACEHOLDER: Update SHA256 after first release
      # Run: curl -sL <url> | shasum -a 256
      sha256 "PLACEHOLDER_ARM_SHA256"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-x86_64-unknown-linux-gnu.tar.gz"
      # PLACEHOLDER: Update SHA256 after first release
      sha256 "PLACEHOLDER_LINUX_INTEL_SHA256"
    end

    on_arm do
      url "https://github.com/dean0x/minotaur/releases/download/v#{version}/minotaur-aarch64-unknown-linux-gnu.tar.gz"
      # PLACEHOLDER: Update SHA256 after first release
      sha256 "PLACEHOLDER_LINUX_ARM_SHA256"
    end
  end

  def install
    bin.install "minotaur"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/minotaur --version")
  end
end
