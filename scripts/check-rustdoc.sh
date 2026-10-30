#!/bin/sh
# The rustdoc gate: a broken intra-doc link or an unclosed HTML tag in a doc
# comment is an error, the same way clippy treats warnings.
# It runs in the prek hook and in CI, so a comment-only change cannot rot the
# crate docs silently.
#
#   check-rustdoc.sh [-h]
#
# Exit codes: 0 clean, 1 cargo doc reported a warning, 2 usage.
set -eu

usage() {
  echo "usage: check-rustdoc.sh"
  echo "  RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps; exits 1 on any warning"
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  "") ;;
  *)
    usage >&2
    exit 2
    ;;
esac

RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
