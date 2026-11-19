# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class NexusRaw < Formula
  desc "Verified, resumable directory transfers for Sonatype Nexus raw"
  homepage "https://github.com/Lipen/nexus-raw"
  version "0.28.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"
      sha256 "c00951aa592c74792a2df318358f832a97c0b16eec0e4ffd757681151d9d9570"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"
      sha256 "6eb7c5284274c057727c79f25e29d45567ecf41f842e961f4cce218a5c9ffbbe"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"
      sha256 "92a010df1c13985f1ecbd519ac4cef50c809d75ac4974ce2fbdc6307f8ab157c"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"
      sha256 "db13ae0883b968af774c83a2ad76e2c22dbbd0185c3c2fdc806e32148012e733"
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
