# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class NexusRaw < Formula
  desc "Verified, resumable directory transfers for Sonatype Nexus raw"
  homepage "https://github.com/Lipen/nexus-raw"
  version "0.29.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"
      sha256 "ecf912d9cbd4b7cc3212fa213827c20f09721e534d7a20610fe837056d730c6f"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"
      sha256 "4aa249a81cfbe86f92d875e16b56e4702a923a00d3b0aa445b9b4b5b1bd135e5"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"
      sha256 "f50402bb08a162317da3a8d792873d5658b1cc74cce62abc1ebba9ae04df597d"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"
      sha256 "cae80dc8b54da5958520467081d3e16f54e1a23d75cc50f3f879d549449ceaa9"
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
