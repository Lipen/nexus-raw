# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class Nxr < Formula
  desc "Verified, resumable directory transfers for Sonatype Nexus raw"
  homepage "https://github.com/Lipen/nexus-raw"
  version "0.22.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"
      sha256 "9aded399cc59faaf29ed8529ee3f54c204f6fa3fddbf65a26be2b8e4f52f12a9"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"
      sha256 "92a36c0f7543207536eafe9371370441594f12122bd8ebeacfccc8ccc055eaf1"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"
      sha256 "bd87d113d22635c71e66c5ccee4ec2ba580ad4d07122fccbf581e082ef0fba7f"
    else
      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"
      sha256 "fb4dbd1e5a10ad74f20040f558228c6c538c07ce3a47ec371fc59da5d68588ff"
    end
  end

  def install
    bin.install "nxr"
    doc.install "README.md", "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/nxr --version")
  end
end
