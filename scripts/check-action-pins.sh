#!/bin/sh
# Every remote action and reusable workflow must be pinned to a full 40-hex
# commit. A mutable tag can move under a job that carries publish credentials,
# so a tag is a supply-chain hole, not a shortcut.
#
# Called from the docs gate: the same list runs in `just check-docs`, the Docs
# workflow and the release gate.
#
# Exit codes: 0 every pin is a commit, 1 an unpinned or tag-pinned action.
set -eu

usage() {
  echo "usage: check-action-pins.sh"
  echo "  asserts every uses: in .github/ is pinned to a 40-hex commit"
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

# Local actions (`uses: ./`) and docker references carry no owner/repo@ref and
# are not covered; a remote action always has `@` after the repository path.
bad="$(grep -RhE '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]+[A-Za-z0-9._/-]+@' \
    .github/workflows .github/actions 2>/dev/null |
  grep -vE '@[0-9a-f]{40}([[:space:]]|$)' || true)"
if [ -n "$bad" ]; then
  echo "check-action-pins: an action is not pinned to a 40-hex commit:" >&2
  echo "$bad" >&2
  exit 1
fi
echo "check-action-pins: every action is pinned"
