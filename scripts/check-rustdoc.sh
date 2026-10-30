#!/bin/sh
# The rustdoc gate: a broken intra-doc link or an unclosed HTML tag in a doc
# comment is an error, the same way clippy treats warnings.
# It runs in the prek hook and in CI, so a comment-only change cannot rot the
# crate docs silently.
set -eu
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
