#!/bin/sh
# Render Formula/nexus-raw.rb from a release's SHA256SUMS.
#
#   brew-formula.sh [VERSION] [--sums FILE] [--out FILE] [--check]
#
# VERSION defaults to [workspace.package] version in Cargo.toml.
# --sums defaults to the release's SHA256SUMS downloaded from GitHub.
# --out defaults to Formula/nexus-raw.rb, and "-" prints to stdout.
# --check diffs the rendered formula against --out and exits 1 on drift.
#
# Exit codes: 0 ok, 1 drift or a missing archive, 2 usage or fetch failure.
set -eu

usage() {
  echo "usage: brew-formula.sh [VERSION] [--sums FILE] [--out FILE] [--check]"
  echo "  renders Formula/nexus-raw.rb from a release's SHA256SUMS"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
esac

here=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$here"

version=""
sums=""
out="Formula/nexus-raw.rb"
check=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --sums)
      [ "$#" -ge 2 ] || usage_err
      sums=$2
      shift 2
      ;;
    --out)
      [ "$#" -ge 2 ] || usage_err
      out=$2
      shift 2
      ;;
    --check)
      check=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage_err
      ;;
    *)
      [ -z "$version" ] || usage_err
      version=$1
      shift
      ;;
  esac
done

if [ -z "$version" ]; then
  version=$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)
fi
case "$version" in
  '' | *[!0-9A-Za-z._-]*) usage_err ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

if [ -z "$sums" ]; then
  sums="$tmp/SHA256SUMS"
  curl -fsSL "https://github.com/Lipen/nexus-raw/releases/download/v$version/SHA256SUMS" -o "$sums" || {
    echo "brew-formula: cannot fetch SHA256SUMS for v$version" >&2
    exit 2
  }
fi
[ -f "$sums" ] || {
  echo "brew-formula: no SHA256SUMS at $sums" >&2
  exit 2
}

sum_for() {
  awk -v f="$1" '$2 == f || $2 == "*" f { print $1; exit }' "$sums"
}

darwin_arm=$(sum_for nxr-darwin-arm64.tar.gz)
darwin_x64=$(sum_for nxr-darwin-x64.tar.gz)
linux_arm=$(sum_for nxr-linux-aarch64.tar.gz)
linux_x64=$(sum_for nxr-linux-x64.tar.gz)
for pair in "darwin-arm64:$darwin_arm" "darwin-x64:$darwin_x64" "linux-aarch64:$linux_arm" "linux-x64:$linux_x64"; do
  case "$pair" in
    *:)
      echo "brew-formula: SHA256SUMS has no ${pair%%:*} archive" >&2
      exit 1
      ;;
  esac
done

rendered="$tmp/nexus-raw.rb"
{
  cat <<'HEAD'
# typed: false
# frozen_string_literal: true

# The tap is this repository itself:
#   brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
#   brew install lipen/nexus-raw/nexus-raw
# Regenerate with `just brew-formula <version>` after a release.
class NexusRaw < Formula
HEAD
  printf '  desc "Verified, resumable directory transfers for Sonatype Nexus raw"\n'
  printf '  homepage "https://github.com/Lipen/nexus-raw"\n'
  printf '  version "%s"\n' "$version"
  printf '  license "MIT"\n\n'
  printf '  on_macos do\n'
  printf '    if Hardware::CPU.arm?\n'
  printf '      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-arm64.tar.gz"\n'
  printf '      sha256 "%s"\n' "$darwin_arm"
  printf '    else\n'
  printf '      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-darwin-x64.tar.gz"\n'
  printf '      sha256 "%s"\n' "$darwin_x64"
  printf '    end\n'
  printf '  end\n\n'
  printf '  on_linux do\n'
  printf '    if Hardware::CPU.arm?\n'
  printf '      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-aarch64.tar.gz"\n'
  printf '      sha256 "%s"\n' "$linux_arm"
  printf '    else\n'
  printf '      url "https://github.com/Lipen/nexus-raw/releases/download/v#{version}/nxr-linux-x64.tar.gz"\n'
  printf '      sha256 "%s"\n' "$linux_x64"
  printf '    end\n'
  printf '  end\n\n'
  cat <<'TAIL'
  def install
    bin.install "nxr"
    doc.install "README.md", "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/nxr --version")
  end
end
TAIL
} > "$rendered"

if [ "$check" -eq 1 ]; then
  if [ "$out" = "-" ]; then
    cat "$rendered"
    exit 0
  fi
  if ! diff -u "$out" "$rendered"; then
    echo "brew-formula: $out is out of date (run without --check to rewrite it)" >&2
    exit 1
  fi
  echo "brew-formula: $out matches v$version"
  exit 0
fi

if [ "$out" = "-" ]; then
  cat "$rendered"
else
  cp "$rendered" "$out"
  echo "brew-formula: wrote $out for v$version"
fi
