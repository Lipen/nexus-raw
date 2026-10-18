#!/bin/sh
# The already-out guard for the PyPI upload.
#
#   pypi-guard.sh
#
# The pypa upload has no skip: a re-dispatch after a partial failure would
# die on "file already exists" instead of idling through. When the version
# is already on PyPI the guard writes `skip=true` to $GITHUB_OUTPUT, which
# holds the publish step back. Otherwise the upload proceeds.
#
# Exit codes: 0 the guard decided, 1 the tree carries no version, 2 bad input.
set -eu

usage() {
  echo "usage: pypi-guard.sh"
  echo "  writes skip=true to \$GITHUB_OUTPUT when the version is already on PyPI"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

here="$(dirname "$0")"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' clients/python/pyproject.toml)"
[ -n "$version" ] || { echo "pypi-guard: no version in clients/python/pyproject.toml" >&2; exit 1; }

if "$here/registry-probe.sh" pypi nexus-raw "$version"; then
  echo "nexus-raw@$version is already on PyPI, skipping the upload"
  echo "skip=true" >> "${GITHUB_OUTPUT:-/dev/null}"
else
  echo "nexus-raw@$version is not on PyPI, the upload proceeds"
fi
