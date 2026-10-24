# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class NexusRaw < Formula
  desc "Verified, resumable directory transfers for Sonatype Nexus raw"
  homepage "https://github.com/Lipen/nexus-raw"
  version "0.24.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"
      sha256 "06dd0c25613f6544486deb514d798bab0c4a17ed2436b4bc0dc636d016ea0992"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"
      sha256 "aea67093bf786c3bf7af8709d14471af4a1f1c71d5385570d62e62b6463221f6"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"
      sha256 "6c3d36919cd689e0b473409b667bfbe13f5c1e8e08cf39bb60a091da835f9dfb"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"
      sha256 "4ff9872fdf6b795a5ce609778fdbfb46176ea548c7e6dd3242487a68c6ed7933"
    end
  end

  def install
    bin.install "nxr"
    # The mount is Linux-only; older Linux archives predate it.
    bin.install "nxr-mount" if OS.linux? && File.exist?("nxr-mount")
    doc.install "README.md", "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/nxr --version")
    if OS.linux? && (bin/"nxr-mount").exist?
      assert_match version.to_s, shell_output("#{bin}/nxr-mount --version")
    end
  end
end
