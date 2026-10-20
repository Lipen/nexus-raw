#!/bin/sh
# Publish staged npm packages, idempotent across re-runs.
#
#   npm-publish.sh <package-dir>...
#
# The platform packages (`npm/npm-*`), the staged main package (`pkg-main`)
# and the bridge (`packaging/npm-bridge`) all go out through here: the name
# and the version come from each `<dir>/package.json`.
# NPM_TAG selects the dist-tag, `latest` by default.
# A release publishes main and the bridge under `next` and promotes them to
# `latest` only after the published example passes.
# A package already on the registry is skipped, a package that appears only
# after our failed attempt lost a race to a concurrent run (not a failure),
# and a package that never appears fails the run. Every package is attempted
# before the exit code judges, so one failure does not hide the others.
#
# Exit codes: 0 every package is out, 1 at least one publish failed, 2 bad input.
set -eu

usage() {
  echo "usage: NPM_TAG=<tag> npm-publish.sh <package-dir>..."
  echo "  publishes each staged package dir, skipping versions already on the registry"
  echo "  NPM_TAG defaults to latest"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "$#" -ge 1 ] || usage_err
tag="${NPM_TAG:-latest}"
case "$tag" in
  ''|*[!A-Za-z0-9._-]*) usage_err ;;
esac
for dir in "$@"; do
  case "$dir" in
    -*) usage_err ;;
  esac
done

here="$(dirname "$0")"
fail=0
for dir in "$@"; do
  name="$(sed -n 's/^  "name": "\(.*\)",/\1/p' "$dir/package.json")"
  version="$(sed -n 's/^  "version": "\(.*\)",/\1/p' "$dir/package.json")"
  if [ -z "$name" ] || [ -z "$version" ]; then
    echo "npm-publish: no name or version in $dir/package.json" >&2
    exit 1
  fi
  if "$here/registry-probe.sh" npm "$name" "$version"; then
    echo "$name@$version is already on the registry, skipping"
  elif ( cd "$dir" && npm publish . --provenance --access public --tag "$tag" ); then
    echo "published"
  elif "$here/registry-probe.sh" npm "$name" "$version"; then
    echo "$name@$version appeared after our publish attempt: lost the race to a concurrent run, not broken"
  else
    echo "$name@$version publish failed and the version is not visible" >&2
    fail=1
  fi
done
if [ "$fail" = 1 ]; then
  # The loop over the staged set closes with one verdict line, exactly as the
  # inline loop did; a single package ends with its own per-package line.
  if [ "$#" -gt 1 ]; then
    echo "platform packages failed to publish" >&2
  fi
  exit 1
fi
