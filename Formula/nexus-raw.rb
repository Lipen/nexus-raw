# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class NexusRaw < Formula
  desc "Verified, resumable directory transfers for Sonatype Nexus raw"
  homepage "https://github.com/Lipen/nexus-raw"
  version "0.25.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"
      sha256 "4aba31a70bc1ad6a4a5dc69f90aa1070ee4926cf85ff765bdcabe83f9969c616"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"
      sha256 "69839fefed820c61f66542fbd2e3f7a41bb9f9f4810e0f057853c0701bea1eb6"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"
      sha256 "e7396aa7d90cf91d2a0a69fbb4567ba1e937146e5bcd8c07f67be6845502850a"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"
      sha256 "b2efeaa8b9ceefc4fe23d9e106d3b681acd984de014c83eb4d430b3e74ac2567"
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
