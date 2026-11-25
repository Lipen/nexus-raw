# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class NexusRaw < Formula
  desc "Verified, resumable directory transfers for Sonatype Nexus raw"
  homepage "https://github.com/Lipen/nexus-raw"
  version "0.29.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"
      sha256 "1a6960dbfa4c87daccd2324a101f36d76e30cd24e1b41ea90283ae21385d9cc3"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"
      sha256 "54c4cb9701fa78be455112afd8d12e6b9309f88028888b94ee6027a5522ad08c"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"
      sha256 "eede559e8478a077d6d5c6d92dd3e6054154b34df017ea0d30bb8f4c299e6f48"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"
      sha256 "119d197cb5bb2853689877cdb98a703f2cc2679a654eeaf237e944eeb934161b"
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
